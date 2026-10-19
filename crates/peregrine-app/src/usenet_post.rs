//! Usenet post-processing: repair a par2 set, list an archive, extract it, and record the files.
//!
//! Both tools are child processes resolved by the caller. The listing is inspected with the same
//! payload inspector the pre-download gate uses, before a byte is extracted; the extracted tree is
//! walked again for links, traversal, and caps. A tool that is absent is a named refusal, not a
//! silent success.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use peregrine_domain::{
    EntryKind, InspectionLimits, OptIn, PathGuard, PayloadDescription, PayloadEntry,
    PayloadInspector, Verdict,
};
use peregrine_storage::{Database, DownloadFileRow, DownloadJobRow, DownloadSource};
use peregrine_usenet::NzbDocument;
use tokio::process::Command;

/// The two external tools a post-process needs.
#[derive(Debug, Clone)]
pub struct PostTools {
    /// The 7-Zip executable.
    pub seven_zip: PathBuf,
    /// The par2 executable, when repair is available.
    pub par2: Option<PathBuf>,
}

impl PostTools {
    /// Finds the tools this build may use: the explicit development variables, then the
    /// application's own sidecar directory. PATH is never consulted.
    #[must_use]
    pub fn resolve() -> Option<Self> {
        let seven_zip = locate("PEREGRINE_7ZZ", &["7zz", "7zz.exe", "7z", "7z.exe"])?;
        let par2 = locate(
            "PEREGRINE_PAR2",
            &["par2", "par2.exe", "par2cmdline", "par2cmdline.exe"],
        );
        Some(Self { seven_zip, par2 })
    }
}

fn locate(variable: &str, names: &[&str]) -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var(variable) {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Some(path);
        }
    }
    let executable = std::env::current_exe().ok()?;
    let directory = executable.parent()?;
    for name in names {
        for candidate in [
            directory.join(name),
            directory.join("sidecars").join(name),
            directory.join("sidecars").join(name).join(name),
            // Tauri copies `resources: ["sidecars"]` into the bundle's resources
            // directory, which is beside `Contents/MacOS` on macOS and beside the
            // executable on Windows and the common Linux layouts.
            directory.join("../Resources/sidecars").join(name),
            directory.join("../Resources").join(name),
            directory.join("resources/sidecars").join(name),
            directory.join("../share/peregrine/sidecars").join(name),
        ] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Why post-processing could not finish.
#[derive(Debug, thiserror::Error)]
pub enum PostError {
    /// No archive tool is configured.
    #[error("the archive extractor is not installed")]
    ExtractorMissing,
    /// The archive listing could not be read.
    #[error("the archive could not be listed")]
    Listing,
    /// The listing was refused by the gate.
    #[error("the archive's contents are not safe to extract")]
    Unsafe,
    /// Extraction failed.
    #[error("the archive could not be extracted")]
    Extract,
    /// A repaired set could not be repaired.
    #[error("the par2 set could not be repaired")]
    Repair,
    /// The output tree escaped the job directory or held a link.
    #[error("the extracted tree is not safe")]
    Tree,
    /// A filesystem operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The database refused the file rows.
    #[error(transparent)]
    Storage(#[from] peregrine_storage::StorageError),
}

/// Repairs, lists, extracts, and records the media a job's archives carry.
///
/// Returns `true` when at least one media file was recorded.
///
/// # Errors
///
/// `PostError` when a tool is missing, the listing is refused, extraction fails, or the output tree
/// is unsafe.
pub async fn process(
    tools: &PostTools,
    database: &Database,
    destination: &PathGuard,
    row: &DownloadJobRow,
    files: &[DownloadFileRow],
    max_bytes: u64,
) -> Result<bool, PostError> {
    let job_dir = Path::new(row.save_path.as_str());
    let password = match row.source.as_ref() {
        Some(DownloadSource::NzbFile(path)) => tokio::fs::read(path.as_str())
            .await
            .ok()
            .and_then(|bytes| NzbDocument::parse(&bytes).ok())
            .and_then(|document| document.password().map(str::to_owned)),
        _ => None,
    };
    let selected: Vec<&DownloadFileRow> = files.iter().filter(|file| file.selected).collect();
    let archives: Vec<&DownloadFileRow> = selected
        .iter()
        .copied()
        .filter(|file| is_archive(file.path.as_str()))
        .collect();
    if archives.is_empty() {
        return Ok(false);
    }

    if let Some(par2) = &tools.par2
        && let Some(set) = selected.iter().find(|file| {
            std::path::Path::new(file.path.as_str())
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("par2"))
        })
    {
        let path = job_dir.join(set.path.as_str());
        repair(par2, &path).await?;
    }

    let archive = job_dir.join(archives[0].path.as_str());
    let mut list = Command::new(&tools.seven_zip);
    list.args(["l", "-slt"]);
    if let Some(password) = &password {
        list.arg(format!("-p{password}"));
    }
    let listing = list
        .arg("--")
        .arg(&archive)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await?;
    if !listing.status.success() {
        return Err(PostError::Listing);
    }
    let text = String::from_utf8_lossy(&listing.stdout);
    let description = listing_description(&text)?;
    let limits = InspectionLimits::new(InspectionLimits::DEFAULT_MAX_FILES, max_bytes);
    let report =
        PayloadInspector::new(limits).inspect(&description, destination, OptIn::NotGranted);
    if report.verdict() != Verdict::Allowed {
        return Err(PostError::Unsafe);
    }

    let extracted = job_dir.join("extracted");
    tokio::fs::create_dir_all(&extracted).await?;
    let mut extract = Command::new(&tools.seven_zip);
    extract
        .args(["x", "-y"])
        .arg(format!("-o{}", extracted.display()));
    if let Some(password) = &password {
        extract.arg(format!("-p{password}"));
    }
    let status = extract
        .arg("--")
        .arg(&archive)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    if !status.success() {
        return Err(PostError::Extract);
    }

    let mut entries = Vec::new();
    collect(&extracted, &extracted, destination, &mut entries).await?;
    let total = entries
        .iter()
        .fold(0u64, |sum, entry| sum.saturating_add(entry.bytes()));
    if entries.is_empty() || total > max_bytes {
        return Err(PostError::Extract);
    }
    let mut all: Vec<PayloadEntry> = files
        .iter()
        .map(|file| PayloadEntry::new(file.path.as_str(), file.size_bytes, EntryKind::File))
        .collect();
    all.extend(entries);
    let combined = PayloadDescription::new(all);
    let selection: Vec<usize> = (0..combined.entries().len()).collect();
    database
        .select_download_files(row.id, combined, &selection)
        .await?;
    Ok(true)
}

async fn repair(par2: &Path, set: &Path) -> Result<(), PostError> {
    if run_ok(par2, &["verify", "-q"], set).await || run_ok(par2, &["repair", "-q"], set).await {
        Ok(())
    } else {
        Err(PostError::Repair)
    }
}

pub(crate) fn is_archive(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|extension| {
            ["rar", "zip", "7z"]
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

pub(crate) fn is_par2(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("par2"))
}

async fn run_ok(tool: &Path, arguments: &[&str], path: &Path) -> bool {
    Command::new(tool)
        .args(arguments)
        .arg("--")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|status| status.success())
}

fn listing_description(text: &str) -> Result<PayloadDescription, PostError> {
    let mut entries = Vec::new();
    let mut path: Option<String> = None;
    let mut size = 0u64;
    let mut first = true;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("Path = ") {
            if first {
                // `7zz l -slt` starts with the archive itself, not a member; judging it would make
                // every archive look nested.
                first = false;
                continue;
            }
            if let Some(previous) = path.take() {
                push_entry(&mut entries, &previous, size);
            }
            path = Some(value.trim().to_owned());
            size = 0;
        } else if let Some(value) = line.strip_prefix("Size = ") {
            size = value.trim().parse().unwrap_or(0);
        } else if let Some(value) = line.strip_prefix("Attributes = ")
            && value.contains('D')
        {
            path = None;
        }
    }
    if let Some(previous) = path {
        push_entry(&mut entries, &previous, size);
    }
    if entries.is_empty() {
        return Err(PostError::Listing);
    }
    Ok(PayloadDescription::new(entries))
}

fn push_entry(entries: &mut Vec<PayloadEntry>, path: &str, size: u64) {
    let path = path.trim().trim_start_matches("./").replace('\\', "/");
    if path.is_empty() || path.ends_with('/') {
        return;
    }
    entries.push(PayloadEntry::new(path, size, EntryKind::File));
}

async fn collect(
    root: &Path,
    current: &Path,
    destination: &PathGuard,
    entries: &mut Vec<PayloadEntry>,
) -> Result<(), PostError> {
    let mut directory = tokio::fs::read_dir(current).await?;
    while let Some(entry) = directory.next_entry().await? {
        let metadata = entry.metadata().await?;
        if metadata.file_type().is_symlink() {
            return Err(PostError::Tree);
        }
        if metadata.is_dir() {
            Box::pin(collect(root, entry.path().as_path(), destination, entries)).await?;
            continue;
        }
        if !metadata.is_file() {
            return Err(PostError::Tree);
        }
        let path = entry.path();
        let Ok(safe) = destination.resolve(&path) else {
            return Err(PostError::Tree);
        };
        let relative = safe
            .as_path()
            .strip_prefix(root)
            .unwrap_or(safe.as_path())
            .to_string_lossy()
            .replace('\\', "/");
        entries.push(PayloadEntry::new(relative, metadata.len(), EntryKind::File));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slt_listing_becomes_payload_entries() {
        let listing = "Path = archive.rar\nSize = 999\nType = Rar\n\nPath = movie.mkv\nSize = 123\nAttributes = A_ -rw-r--r--\n\nPath = extras/\nSize = 0\nAttributes = D_ drwxr-xr-x\n\nPath = extras/note.txt\nSize = 2\nAttributes = A_ -rw-r--r--\n";
        let description = listing_description(listing).expect("a listing");
        assert_eq!(description.file_count(), 2);
        assert_eq!(description.entries()[0].path(), "movie.mkv");
        assert_eq!(description.entries()[0].bytes(), 123);
        assert_eq!(description.entries()[1].path(), "extras/note.txt");
    }

    #[test]
    fn archives_are_recognised_without_a_case_trap() {
        assert!(is_archive("Movie.RAR"));
        assert!(is_archive("parts/movie.part1.rar"));
        assert!(is_archive("extras.zip"));
        assert!(is_archive("backup.7z"));
        assert!(!is_archive("movie.mkv"));
        assert!(!is_archive("rar"));
    }

    #[test]
    fn an_empty_listing_is_refused() {
        assert!(matches!(
            listing_description("Path = \nSize = 0\n"),
            Err(PostError::Listing)
        ));
    }
}
#[test]
fn a_traversing_listing_is_blocked_by_the_inspector() {
    let listing = "Path = archive.rar\nSize = 9\nType = Rar\n\nPath = ../escape.mkv\nSize = 1\nAttributes = A_ -rw-r--r--\n";
    let description = listing_description(listing).expect("a listing");
    let guard = PathGuard::new(std::env::temp_dir()).expect("a root");
    let report = PayloadInspector::new(InspectionLimits::new(10, u64::MAX)).inspect(
        &description,
        &guard,
        OptIn::NotGranted,
    );
    assert_eq!(report.verdict(), Verdict::Blocked);
}
#[tokio::test]
async fn a_symlink_in_the_extracted_tree_is_refused() {
    let root = tempfile::TempDir::new().expect("a temporary directory");
    let extracted = root.path().join("extracted");
    tokio::fs::create_dir_all(&extracted).await.expect("dir");
    tokio::fs::write(extracted.join("movie.mkv"), b"hello")
        .await
        .expect("file");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc/passwd", extracted.join("link.mkv")).expect("link");
        let guard = PathGuard::new(root.path()).expect("a root");
        let mut entries = Vec::new();
        let error = collect(&extracted, &extracted, &guard, &mut entries)
            .await
            .expect_err("a link is refused");
        assert!(matches!(error, PostError::Tree));
    }
}

#[tokio::test]
async fn a_regular_tree_is_collected_with_relative_paths() {
    let root = tempfile::TempDir::new().expect("a temporary directory");
    let extracted = root.path().join("extracted");
    tokio::fs::create_dir_all(extracted.join("extras"))
        .await
        .expect("dir");
    tokio::fs::write(extracted.join("movie.mkv"), b"hello")
        .await
        .expect("file");
    tokio::fs::write(extracted.join("extras/note.txt"), b"note")
        .await
        .expect("file");
    let guard = PathGuard::new(root.path()).expect("a root");
    let mut entries = Vec::new();
    collect(&extracted, &extracted, &guard, &mut entries)
        .await
        .expect("a tree");
    assert_eq!(entries.len(), 2);
    assert!(
        entries
            .iter()
            .any(|entry| entry.path() == "extras/note.txt")
    );
}
#[cfg(unix)]
#[tokio::test]
async fn par2_answers_healthy_repairable_and_unrepairable_distinctly() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::TempDir::new().expect("a temporary directory");
    let set = root.path().join("set.par2");
    tokio::fs::write(&set, b"set").await.expect("set");
    let script = root.path().join("par2");
    tokio::fs::write(
        &script,
        "#!/bin/sh\ncase \"$1\" in verify) exit 0;; repair) exit 0;; esac\n",
    )
    .await
    .expect("script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("executable");
    assert!(repair(&script, &set).await.is_ok(), "healthy");

    tokio::fs::write(
        &script,
        "#!/bin/sh\ncase \"$1\" in verify) exit 1;; repair) exit 0;; esac\n",
    )
    .await
    .expect("script");
    assert!(repair(&script, &set).await.is_ok(), "repaired");

    tokio::fs::write(
        &script,
        "#!/bin/sh\ncase \"$1\" in verify) exit 1;; repair) exit 1;; esac\n",
    )
    .await
    .expect("script");
    assert!(matches!(
        repair(&script, &set).await,
        Err(PostError::Repair)
    ));
}
#[test]
fn absolute_nested_and_oversized_listings_are_refused() {
    let root = tempfile::TempDir::new().expect("a temporary directory");
    let guard = PathGuard::new(root.path()).expect("a root");
    for (path, size) in [
        ("/etc/passwd", 1),
        ("nested.rar", 1),
        ("movie.mkv", 10_000_000),
    ] {
        let listing = format!(
            "Path = archive.rar\nSize = 9\nType = Rar\n\nPath = {path}\nSize = {size}\nAttributes = A_ -rw-r--r--\n"
        );
        let description = listing_description(&listing).expect("a listing");
        let report = PayloadInspector::new(InspectionLimits::new(10, 1_000)).inspect(
            &description,
            &guard,
            OptIn::NotGranted,
        );
        assert_ne!(report.verdict(), Verdict::Allowed, "{path} was allowed");
    }
}
