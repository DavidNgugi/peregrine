//! Walking a root and keeping the candidates (PG-E14-02).
//!
//! `docs/FLOWS.md` F8 has the library service hand a root to a scanner and get back "candidate
//! files with mtime and size": the decision about *what* is a candidate is the domain's
//! ([`peregrine_domain::ScanPolicy`]), and everything filesystem-shaped is here — reading the
//! directories, refusing to follow a link, and reporting what it could not read rather than
//! failing the whole scan for one unreadable folder.
//!
//! # Three things this walk does that a `read_dir` recursion does not
//!
//! **It does not follow symbolic links.** A link inside a root that points outside it is how an
//! index ends up holding files the user never pointed at, and the guard that decides containment
//! is syntactic — it cannot see where a link resolves. Skipping them is the safe half of the
//! trade, and the cost is a user whose library is assembled from links, who can add the link's
//! target as its own root. The reason is recorded either way, so a scan says what it ignored
//! rather than quietly indexing less than the user expected.
//!
//! **It sorts each directory by name.** [`std::fs::read_dir`] returns entries in whatever order
//! the filesystem holds them, which differs between machines and between two runs on one machine.
//! A scan whose log is in a different order every time is a log nobody can diff, and the two
//! passes of an incremental scan (PG-E14-03) would report changes in an order that means nothing.
//!
//! **It reports an unreadable directory instead of failing.** A library with one folder whose
//! permissions were changed is a library that still scans; the outcome carries the directory and
//! the kind of failure, and only an unreadable *root* stops the scan, because then there is
//! nothing to report.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use peregrine_domain::{ScanPolicy, ScanVerdict, SkipReason};
use thiserror::Error;
use tracing::debug;

/// One file the library could hold, with what an incremental pass compares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateFile {
    /// The file's path, as the walk found it.
    pub path: PathBuf,
    /// Its size in bytes.
    pub size_bytes: u64,
    /// Its modification time, in milliseconds since the Unix epoch.
    pub mtime_ms: i64,
}

/// One path the policy refused, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedPath {
    /// What was skipped: a file, or a directory whose contents were never enumerated.
    pub path: PathBuf,
    /// The reason, which is what makes a scan log answer "why is my film not there".
    pub reason: SkipReason,
}

/// One directory the walk could not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadableDirectory {
    /// The directory.
    pub path: PathBuf,
    /// What the filesystem said, as a kind rather than a message: the message carries the path
    /// again, and the kind is what a caller can branch on.
    pub kind: io::ErrorKind,
}

/// What one pass over a root found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanOutcome {
    /// The files a later pass will parse, probe, and match.
    pub candidates: Vec<CandidateFile>,
    /// The paths the policy refused, each with its reason.
    pub skipped: Vec<SkippedPath>,
    /// The directories that could not be read, which do not stop the scan.
    pub unreadable: Vec<UnreadableDirectory>,
}

/// Why a root could not be walked at all.
#[derive(Debug, Error)]
pub enum ScanError {
    /// The root itself could not be read, so there is nothing to report about its contents.
    #[error("the library root {} could not be read", .path.display())]
    Root {
        /// The root.
        path: PathBuf,
        /// What the filesystem said.
        #[source]
        source: io::Error,
    },
}

/// Walks roots and reports what they hold.
#[derive(Debug, Clone, Copy, Default)]
pub struct Scanner {
    policy: ScanPolicy,
}

impl Scanner {
    /// A scanner applying `policy`.
    #[must_use]
    pub const fn new(policy: ScanPolicy) -> Self {
        Self { policy }
    }

    /// The policy this scanner applies, so a caller can classify a path the same way.
    #[must_use]
    pub const fn policy(&self) -> ScanPolicy {
        self.policy
    }

    /// Walks `root` and reports its candidates, its skips, and what it could not read.
    ///
    /// # Errors
    ///
    /// [`ScanError::Root`] when the root cannot be read. A directory *inside* the root that cannot
    /// be read is reported in the outcome instead, because one unreadable folder is not a reason
    /// to abandon a library.
    pub fn scan(&self, root: &Path) -> Result<ScanOutcome, ScanError> {
        let mut outcome = ScanOutcome::default();
        let entries = read_sorted(root).map_err(|source| ScanError::Root {
            path: root.to_path_buf(),
            source,
        })?;

        self.walk_entries(entries, &mut outcome);
        Ok(outcome)
    }

    /// Classifies and descends into one directory's already-sorted entries.
    fn walk_entries(self, entries: Vec<fs::DirEntry>, outcome: &mut ScanOutcome) {
        for entry in entries {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                // The entry vanished between the listing and the stat, which a download finishing
                // beside a scan does routinely. It is not a candidate and not an error.
                outcome.unreadable.push(UnreadableDirectory {
                    path,
                    kind: io::ErrorKind::NotFound,
                });
                continue;
            };

            if file_type.is_symlink() {
                skip(outcome, path, SkipReason::Symlink);
                continue;
            }

            if file_type.is_dir() {
                match self.policy.directory(&name_of(&path)) {
                    // The directory is reported once, with its reason, rather than every file
                    // inside it: the user skipped the folder, so the folder is the answer.
                    ScanVerdict::Skipped(reason) => skip(outcome, path, reason),
                    ScanVerdict::Candidate => match read_sorted(&path) {
                        Ok(children) => self.walk_entries(children, outcome),
                        Err(error) => outcome.unreadable.push(UnreadableDirectory {
                            path,
                            kind: error.kind(),
                        }),
                    },
                }
                continue;
            }

            if !file_type.is_file() {
                // A socket, a fifo, or a device node in a media folder is not a title. Nothing is
                // read from it, so a fifo cannot block the walk.
                skip(outcome, path, SkipReason::Unsupported);
                continue;
            }

            match self.policy.file(&path, false) {
                ScanVerdict::Skipped(reason) => skip(outcome, path, reason),
                ScanVerdict::Candidate => match entry.metadata() {
                    Ok(metadata) => outcome.candidates.push(CandidateFile {
                        path,
                        size_bytes: metadata.len(),
                        mtime_ms: modified_millis(&metadata),
                    }),
                    Err(error) => outcome.unreadable.push(UnreadableDirectory {
                        path,
                        kind: error.kind(),
                    }),
                },
            }
        }
    }
}

/// Records a skip and logs it.
///
/// The log is `debug` because a large library has thousands of skips and every one of them is
/// expected; the summary a user or a support report reads is the job's to write (PG-E14-04), and
/// what this line exists for is answering "why is my film not in the library" without a rebuild.
fn skip(outcome: &mut ScanOutcome, path: PathBuf, reason: SkipReason) {
    debug!(
        path = %path.display(),
        reason = reason.as_str(),
        "a library path is not a candidate"
    );
    outcome.skipped.push(SkippedPath { path, reason });
}

/// One directory's entries, sorted by name.
///
/// Sorted because the order a filesystem returns entries in is arbitrary: see the module doc.
fn read_sorted(directory: &Path) -> io::Result<Vec<fs::DirEntry>> {
    let mut entries: Vec<fs::DirEntry> = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    Ok(entries)
}

/// A path's own name, or the empty string for a path that has none.
fn name_of(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_owned()
}

/// A file's modification time in milliseconds since the Unix epoch.
///
/// A file older than the epoch — which a copied archive or a badly set clock produces — has no
/// representable value here. It is given `i64::MIN` rather than the epoch, because the instant is
/// what an incremental pass (PG-E14-03) compares: a value no real mtime can equal makes such a
/// file look changed on every scan, which is the safe direction, while `0` would make it look
/// unchanged forever.
fn modified_millis(metadata: &fs::Metadata) -> i64 {
    let Ok(modified) = metadata.modified() else {
        return i64::MIN;
    };
    match modified.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(elapsed) => i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX),
        Err(_) => i64::MIN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tree built once per test, removed when the guard drops.
    struct Corpus {
        root: PathBuf,
    }

    impl Corpus {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("peregrine-scan-{name}"));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn file(&self, relative: &str, bytes: &[u8]) -> PathBuf {
            let path = self.root.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, bytes).unwrap();
            path
        }

        fn directory(&self, relative: &str) -> PathBuf {
            let path = self.root.join(relative);
            fs::create_dir_all(&path).unwrap();
            path
        }
    }

    impl Drop for Corpus {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    /// The corpus PG-E14-02's acceptance names, as a tree of real files.
    fn corpus(name: &str) -> (Corpus, Scanner) {
        let corpus = Corpus::new(name);
        // Two things a library wants.
        corpus.file("Movies/Alien (1979).mkv", b"a film");
        corpus.file("Shows/Breaking Bad/Season 1/S01E01.mkv", b"an episode");
        // And everything it does not.
        corpus.file("Movies/Alien (1979).srt", b"subtitles");
        corpus.file("Movies/Alien (1979).nfo", b"metadata");
        corpus.file("Movies/.hidden.mkv", b"hidden");
        corpus.file("Movies/._Alien.mkv", b"an AppleDouble twin");
        corpus.file("Movies/Alien.mkv.part", b"a download in progress");
        corpus.file("Movies/Alien-sample.mkv", b"a release sample");
        corpus.file("Movies/Trailer - Alien.mkv", b"a trailer");
        corpus.file("Movies/Sample/clip.mkv", b"a sample folder");
        corpus.file("Movies/Extras/interview.mkv", b"an interview");
        corpus.file("Movies/.Trash/old.mkv", b"a hidden folder");
        (corpus, Scanner::new(ScanPolicy::new()))
    }

    fn relative(root: &Path, path: &Path) -> String {
        path.strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn the_corpus_scan_keeps_the_titles_and_skips_the_rest() {
        // The acceptance: "corpus scan skips the right files with reasons logged". Both halves are
        // asserted, because a scanner that skipped everything would satisfy the second alone.
        let (corpus, scanner) = corpus("acceptance");

        let outcome = scanner.scan(&corpus.root).unwrap();

        let kept: Vec<String> = outcome
            .candidates
            .iter()
            .map(|file| relative(&corpus.root, &file.path))
            .collect();
        assert_eq!(
            kept,
            [
                "Movies/Alien (1979).mkv",
                "Shows/Breaking Bad/Season 1/S01E01.mkv"
            ],
            "exactly the two titles, in a stable order"
        );
    }

    #[test]
    fn every_skip_carries_the_reason_a_user_would_give() {
        let (corpus, scanner) = corpus("reasons");

        let outcome = scanner.scan(&corpus.root).unwrap();
        let reasons: Vec<(String, SkipReason)> = outcome
            .skipped
            .iter()
            .map(|skip| (relative(&corpus.root, &skip.path), skip.reason))
            .collect();

        assert!(reasons.contains(&(
            "Movies/Alien (1979).srt".to_owned(),
            SkipReason::Unsupported
        )));
        assert!(reasons.contains(&(
            "Movies/Alien (1979).nfo".to_owned(),
            SkipReason::Unsupported
        )));
        assert!(reasons.contains(&("Movies/.hidden.mkv".to_owned(), SkipReason::Hidden)));
        assert!(reasons.contains(&("Movies/._Alien.mkv".to_owned(), SkipReason::Hidden)));
        assert!(reasons.contains(&("Movies/Alien.mkv.part".to_owned(), SkipReason::Temporary)));
        assert!(reasons.contains(&("Movies/Alien-sample.mkv".to_owned(), SkipReason::Sample)));
        assert!(reasons.contains(&("Movies/Trailer - Alien.mkv".to_owned(), SkipReason::Trailer)));
        assert!(
            reasons.contains(&("Movies/Sample".to_owned(), SkipReason::Sample)),
            "the folder is reported, not each file inside it"
        );
        assert!(reasons.contains(&("Movies/Extras".to_owned(), SkipReason::Extras)));
        assert!(reasons.contains(&("Movies/.Trash".to_owned(), SkipReason::Hidden)));
    }

    #[test]
    fn an_ignored_directory_is_reported_once_and_never_descended() {
        // One line per ignored folder is what a user can act on; a hundred lines about the files
        // inside a folder they chose to ignore is noise, and descending it would also be work
        // spent to throw the answer away.
        let (corpus, scanner) = corpus("ignored-folder");

        let outcome = scanner.scan(&corpus.root).unwrap();

        let inside_extras = outcome
            .skipped
            .iter()
            .filter(|skip| skip.path.starts_with(corpus.root.join("Movies/Extras")))
            .count();
        assert_eq!(inside_extras, 1, "the folder, and nothing under it");
        assert!(
            outcome
                .candidates
                .iter()
                .all(|file| !file.path.starts_with(corpus.root.join("Movies/Extras")))
        );
    }

    #[test]
    fn a_directory_that_is_not_ignored_is_walked_to_any_depth() {
        let (corpus, scanner) = corpus("depth");

        let outcome = scanner.scan(&corpus.root).unwrap();

        assert!(
            outcome
                .candidates
                .iter()
                .any(|file| file.path.ends_with("Season 1/S01E01.mkv")),
            "three levels down is still the library"
        );
    }

    #[test]
    fn a_symlink_is_reported_rather_than_followed() {
        // A link inside a root that points outside it is how an index ends up holding files the
        // user never pointed at. The link is also not descended, so its target is never walked.
        #[cfg(unix)]
        {
            let (corpus, scanner) = corpus("symlink");
            let target = corpus.root.join("Movies/Alien (1979).mkv");
            let link = corpus.root.join("Movies/Alien-link.mkv");
            std::os::unix::fs::symlink(&target, &link).unwrap();

            let outcome = scanner.scan(&corpus.root).unwrap();

            assert!(
                outcome
                    .skipped
                    .iter()
                    .any(|skip| skip.path == link && skip.reason == SkipReason::Symlink),
                "the link is reported as a link"
            );
            assert!(
                !outcome.candidates.iter().any(|file| file.path == link),
                "and it is not a candidate"
            );
        }
    }

    #[test]
    fn a_linked_directory_is_not_walked_out_of_the_root() {
        #[cfg(unix)]
        {
            let (corpus, scanner) = corpus("symlink-dir");
            let outside = Corpus::new("symlink-outside");
            outside.file("Elsewhere/Other Film.mkv", b"not in this library");
            std::os::unix::fs::symlink(&outside.root, corpus.root.join("Linked")).unwrap();

            let outcome = scanner.scan(&corpus.root).unwrap();

            assert!(
                outcome
                    .candidates
                    .iter()
                    .all(|file| !file.path.to_string_lossy().contains("Other Film")),
                "a link out of the root brings nothing in with it"
            );
            assert!(
                outcome
                    .skipped
                    .iter()
                    .any(|skip| skip.path.ends_with("Linked") && skip.reason == SkipReason::Symlink)
            );
        }
    }

    #[test]
    fn the_walk_reports_the_size_and_mtime_a_later_pass_needs() {
        let (corpus, scanner) = corpus("metadata");

        let outcome = scanner.scan(&corpus.root).unwrap();

        let film = outcome
            .candidates
            .iter()
            .find(|file| file.path.ends_with("Alien (1979).mkv"))
            .expect("the film is a candidate");
        assert_eq!(film.size_bytes, b"a film".len() as u64);
        assert!(film.mtime_ms > 0, "a file written now has a real mtime");
    }

    #[test]
    fn an_empty_root_has_nothing_to_report() {
        // The empty case, and the boundary between "nothing here" and "could not read this".
        let corpus = Corpus::new("empty");
        let scanner = Scanner::new(ScanPolicy::new());

        let outcome = scanner.scan(&corpus.root).unwrap();

        assert_eq!(outcome, ScanOutcome::default());
    }

    #[test]
    fn a_root_that_is_not_there_is_an_error_rather_than_an_empty_scan() {
        // "The folder has no media" and "I could not look" are different answers, and a scan that
        // reported the second as the first would mark every file in an unmounted library missing.
        let corpus = Corpus::new("missing");
        let scanner = Scanner::new(ScanPolicy::new());

        let outcome = scanner.scan(&corpus.root.join("NotPluggedIn"));

        assert!(
            matches!(outcome, Err(ScanError::Root { .. })),
            "{outcome:?}"
        );
    }

    #[test]
    fn a_root_that_is_a_file_is_an_error_too() {
        let corpus = Corpus::new("file-root");
        let file = corpus.file("not-a-folder.mkv", b"bytes");
        let scanner = Scanner::new(ScanPolicy::new());

        assert!(matches!(scanner.scan(&file), Err(ScanError::Root { .. })));
    }

    #[test]
    fn two_scans_of_one_tree_agree_exactly() {
        // The walk sorts, so an unchanged tree produces an identical outcome. Without it the two
        // passes of an incremental scan would report differences that are only entry order.
        let (corpus, scanner) = corpus("determinism");

        let first = scanner.scan(&corpus.root).unwrap();
        let second = scanner.scan(&corpus.root).unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn an_unreadable_directory_is_reported_and_the_scan_goes_on() {
        // One folder whose permissions changed is not a reason to abandon a library.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            let (corpus, scanner) = corpus("unreadable");
            let locked = corpus.directory("Movies/Locked");
            corpus.file("Movies/Locked/inside.mkv", b"cannot be listed");
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

            // A test running as root can read it anyway, and the case cannot be staged.
            if fs::read_dir(&locked).is_ok() {
                fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
                return;
            }

            let outcome = scanner.scan(&corpus.root).unwrap();

            // Restore before the guard drops, or the removal fails on some platforms.
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(outcome.unreadable.len(), 1, "{outcome:?}");
            assert!(outcome.unreadable[0].path.ends_with("Locked"));
            assert!(
                outcome
                    .candidates
                    .iter()
                    .any(|file| file.path.ends_with("Alien (1979).mkv")),
                "the rest of the library still scanned"
            );
        }
    }

    #[test]
    fn the_scanner_applies_the_policy_a_caller_can_read_back() {
        // The scanner and the caller classify the same path the same way, because it is one
        // policy rather than two copies of one decision.
        let scanner = Scanner::new(ScanPolicy::new());

        assert_eq!(
            scanner.policy().directory("Extras"),
            ScanVerdict::Skipped(SkipReason::Extras)
        );
    }
}
