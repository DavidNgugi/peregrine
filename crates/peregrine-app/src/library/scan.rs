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

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use peregrine_domain::{Freshness, ScanPolicy, ScanVerdict, SkipReason, Stamp};
use thiserror::Error;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
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

impl CandidateFile {
    /// What an incremental pass records about this file.
    #[must_use]
    pub const fn stamp(&self) -> Stamp {
        Stamp {
            size_bytes: self.size_bytes,
            mtime_ms: self.mtime_ms,
        }
    }

    /// What an incremental pass should do about this file, given what the library recorded.
    ///
    /// `known` is keyed by path and holds one stamp per file the library already has under this
    /// root. Building it is the caller's job because it is a database read, and where it comes
    /// from is the scan job's business (PG-E14-04); what this method owns is the comparison and
    /// the answer, which is [`Freshness`]'s rule and not a second copy of it.
    #[must_use]
    pub fn freshness(&self, known: &HashMap<PathBuf, Stamp>) -> Freshness {
        Freshness::of(known.get(&self.path).copied(), self.stamp())
    }
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

impl ScanOutcome {
    /// Folds one directory's findings in and answers with the directories left to visit.
    ///
    /// The queue is the *walk's* state, not the outcome's: what a caller reads is a finished
    /// answer, and an outcome that carried a work list would make two walks of one tree differ by
    /// the list each of them had not finished yet.
    fn absorb(&mut self, collected: DirectoryOutcome) -> Vec<PathBuf> {
        self.candidates.extend(collected.candidates);
        self.skipped.extend(collected.skipped);
        self.unreadable.extend(collected.unreadable);
        collected.directories
    }

    /// Sorts every list by path.
    ///
    /// Two walks of one tree — the sequential one and the bounded one — have to produce the same
    /// outcome, and the order they *found* things in is the one thing about them that differs by
    /// construction. Sorting is what makes that difference invisible, and it is also what makes a
    /// scan log diffable between two runs (PG-E14-02).
    fn sort(&mut self) {
        self.candidates
            .sort_by(|left, right| left.path.cmp(&right.path));
        self.skipped
            .sort_by(|left, right| left.path.cmp(&right.path));
        self.unreadable
            .sort_by(|left, right| left.path.cmp(&right.path));
    }
}

/// One directory's own entries, classified.
///
/// The unit both walks are built from. `directories` is what is left to visit rather than what has
/// been visited, so the driver decides the order — depth first for the sequential walk, whatever
/// finishes first for the bounded one.
#[derive(Debug, Default)]
struct DirectoryOutcome {
    candidates: Vec<CandidateFile>,
    skipped: Vec<SkippedPath>,
    unreadable: Vec<UnreadableDirectory>,
    directories: Vec<PathBuf>,
}

/// Why reading the *root* did not answer, which the driver needs to tell apart.
enum RootRead {
    /// The filesystem refused.
    Io(io::Error),
    /// The blocking task did not finish, which only a panic inside it can cause.
    Worker,
}

/// Reads one directory on the blocking pool.
async fn read_on_the_pool(
    scanner: Scanner,
    root: &Path,
    directory: &Path,
) -> Result<DirectoryOutcome, RootRead> {
    let root = root.to_path_buf();
    let directory = directory.to_path_buf();
    tokio::task::spawn_blocking(move || scanner.collect_directory(&root, &directory))
        .await
        .map_err(|_| RootRead::Worker)?
        .map_err(RootRead::Io)
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

    /// The walk was cancelled, so its outcome says nothing about the tree.
    ///
    /// This is an error rather than a partial outcome on purpose: a caller that reconciled from a
    /// partial walk would mark every file it had not reached yet as missing (PG-E14-04's
    /// acceptance, and the reason `PG-E14-13`'s reconciliation is only ever called with a scan that
    /// finished).
    #[error("the scan was cancelled")]
    Cancelled,

    /// A blocking task did not finish, which only a panic inside it can cause.
    #[error("the scan's worker for one directory did not finish")]
    Worker,
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
        let collected = self
            .collect_directory(root, root)
            .map_err(|source| ScanError::Root {
                path: root.to_path_buf(),
                source,
            })?;
        let mut queue = outcome.absorb(collected);

        while let Some(directory) = queue.pop() {
            match self.collect_directory(root, &directory) {
                Ok(collected) => queue.extend(outcome.absorb(collected)),
                Err(error) => outcome.unreadable.push(UnreadableDirectory {
                    path: directory,
                    kind: error.kind(),
                }),
            }
        }

        outcome.sort();
        Ok(outcome)
    }

    /// Walks `root` with at most `limit` directories being read at once, and stops when `cancel` is
    /// requested.
    ///
    /// The same traversal as [`Scanner::scan`], driven differently: every directory is read on the
    /// blocking pool, up to `limit` of them in flight, because a filesystem walk must never block
    /// the runtime and a library on a network volume makes that concrete. The outcome is
    /// **identical** to the sequential walk's — same candidates, same skips, same unreadable
    /// directories, in the same order — which is asserted rather than assumed, because two walks
    /// that disagree would make a scan's result depend on which one ran it.
    ///
    /// # Errors
    ///
    /// [`ScanError::Root`] when the root itself cannot be read, which is the one failure that stops
    /// the scan because then there is nothing to report; [`ScanError::Cancelled`] when cancellation
    /// was requested; [`ScanError::Worker`] when a blocking task did not finish, which only a panic
    /// inside it can cause.
    pub async fn scan_bounded(
        &self,
        root: &Path,
        limit: usize,
        cancel: &CancellationToken,
    ) -> Result<ScanOutcome, ScanError> {
        let limit = limit.max(1);
        let mut outcome = ScanOutcome::default();
        let mut pending: JoinSet<(PathBuf, io::Result<DirectoryOutcome>)> = JoinSet::new();

        let mut queue: Vec<PathBuf> = match read_on_the_pool(*self, root, root).await {
            Ok(collected) => outcome.absorb(collected),
            Err(RootRead::Io(source)) => {
                return Err(ScanError::Root {
                    path: root.to_path_buf(),
                    source,
                });
            }
            Err(RootRead::Worker) => return Err(ScanError::Worker),
        };

        while !queue.is_empty() || !pending.is_empty() {
            if cancel.is_cancelled() {
                // The tasks still reading are asked to stop rather than awaited: what they were
                // going to produce is not needed, and a cancelled scan should stop promptly.
                pending.abort_all();
                return Err(ScanError::Cancelled);
            }

            while pending.len() < limit {
                let Some(directory) = queue.pop() else {
                    break;
                };
                let scanner = *self;
                let root = root.to_path_buf();
                pending.spawn_blocking(move || {
                    let collected = scanner.collect_directory(&root, &directory);
                    (directory, collected)
                });
            }

            let Some(joined) = pending.join_next().await else {
                break;
            };
            match joined {
                Ok((_visited, Ok(collected))) => queue.extend(outcome.absorb(collected)),
                Ok((directory, Err(error))) => outcome.unreadable.push(UnreadableDirectory {
                    path: directory,
                    kind: error.kind(),
                }),
                Err(_) => return Err(ScanError::Worker),
            }
        }

        outcome.sort();
        Ok(outcome)
    }

    /// One directory's own entries, classified, with the directories below it to visit.
    ///
    /// This is the whole of the scan's IO for one level — the listing, each entry's type, and the
    /// stat that stamps a candidate — which is why it is the unit both walks are built from: the
    /// sequential one calls it while descending, and the bounded one runs several at once on the
    /// blocking pool. It is synchronous on purpose. `std::fs` blocks, and the repo's rule is that a
    /// filesystem walk never blocks the runtime, so the blocking happens here and the awaits happen
    /// in the driver.
    fn collect_directory(self, root: &Path, directory: &Path) -> io::Result<DirectoryOutcome> {
        let mut collected = DirectoryOutcome::default();
        for entry in read_sorted(directory)? {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                // The entry vanished between the listing and the stat, which a download finishing
                // beside a scan does routinely. It is not a candidate and not an error.
                collected.unreadable.push(UnreadableDirectory {
                    path,
                    kind: io::ErrorKind::NotFound,
                });
                continue;
            };

            if file_type.is_symlink() {
                skip(&mut collected, path, SkipReason::Symlink);
                continue;
            }

            if file_type.is_dir() {
                match self.policy.directory(&name_of(&path)) {
                    // The directory is reported once, with its reason, rather than every file
                    // inside it: the user skipped the folder, so the folder is the answer.
                    ScanVerdict::Skipped(reason) => skip(&mut collected, path, reason),
                    ScanVerdict::Candidate => collected.directories.push(path),
                }
                continue;
            }

            if !file_type.is_file() {
                // A socket, a fifo, or a device node in a media folder is not a title. Nothing is
                // read from it, so a fifo cannot block the walk.
                skip(&mut collected, path, SkipReason::Unsupported);
                continue;
            }

            // Relative to the root, for the reason in the module doc: the components of the root
            // are the user's choice, and only what is below it is the policy's business.
            match self
                .policy
                .file(path.strip_prefix(root).unwrap_or(&path), false)
            {
                ScanVerdict::Skipped(reason) => skip(&mut collected, path, reason),
                ScanVerdict::Candidate => match entry.metadata() {
                    Ok(metadata) => collected.candidates.push(CandidateFile {
                        path,
                        size_bytes: metadata.len(),
                        mtime_ms: modified_millis(&metadata),
                    }),
                    Err(error) => collected.unreadable.push(UnreadableDirectory {
                        path,
                        kind: error.kind(),
                    }),
                },
            }
        }
        Ok(collected)
    }
}

/// Records a skip and logs it.
///
/// The log is `debug` because a large library has thousands of skips and every one of them is
/// expected; the summary a user or a support report reads is the job's to write (PG-E14-04), and
/// what this line exists for is answering "why is my film not in the library" without a rebuild.
fn skip(outcome: &mut DirectoryOutcome, path: PathBuf, reason: SkipReason) {
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
pub(crate) fn modified_millis(metadata: &fs::Metadata) -> i64 {
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

        /// Used only by the unix permission tests; dead code on Windows is an error there.
        #[cfg(unix)]
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
        // The scan canonicalises what it walks, and on Windows a temporary directory's short name
        // (RUNNER~1) is not the long one canonicalisation returns; compare under one spelling.
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        canonical_path
            .strip_prefix(&canonical_root)
            .unwrap_or(&canonical_path)
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
    fn a_root_under_a_hidden_or_extras_directory_is_still_scanned() {
        // Found by PG-E14-13's reconciler tests, which build their corpus with `TempDir` — whose
        // own name starts with a dot. The policy was being applied to the root's own path, so a
        // library at `/Users/me/.media/Movies` was skipped in its entirety, every file reported as
        // hidden. The rules describe what is *below* the root, because the root is a path the user
        // chose and can defend; `/media/Extras/Alien.mkv` is not the same claim as a root the user
        // named `Extras`.
        for root_name in [".media-library", ".hidden", "Extras", "Sample"] {
            let corpus = Corpus::new(&format!("dotted-root-{root_name}"));
            let film = corpus.file("Movies/Alien (1979).mkv", b"a film");
            let scan_root = corpus.root.join(root_name);
            fs::create_dir_all(&scan_root).unwrap();
            let inside = scan_root.join("Movies/Alien (1979).mkv");
            fs::create_dir_all(inside.parent().unwrap()).unwrap();
            fs::rename(&film, &inside).unwrap();

            let outcome = Scanner::new(ScanPolicy::new()).scan(&scan_root).unwrap();

            assert_eq!(
                outcome.candidates.len(),
                1,
                "a root named {root_name} holds one film"
            );
            assert_eq!(outcome.candidates[0].path, inside);
            assert!(
                outcome.skipped.is_empty(),
                "nothing below a chosen root is skipped for the root's own name: {:?}",
                outcome.skipped
            );
        }
    }

    #[test]
    fn a_hidden_directory_below_the_root_is_still_skipped() {
        // The other half of the same rule, so the fix above cannot be read as "hidden names stop
        // mattering".
        let corpus = Corpus::new("still-hidden");

        corpus.file("Movies/.Trash/old.mkv", b"a hidden folder");

        let outcome = Scanner::new(ScanPolicy::new()).scan(&corpus.root).unwrap();

        assert_eq!(outcome.candidates, Vec::new());
        assert_eq!(outcome.skipped[0].reason, SkipReason::Hidden);
    }

    #[test]
    fn a_file_that_changed_size_without_changing_its_mtime_is_detected() {
        // PG-E14-03's acceptance, staged against a real filesystem rather than against the rule
        // alone. `File::set_modified` puts the recorded mtime back after the rewrite, which is what
        // a filesystem with a coarse timestamp granularity does by itself: FAT and NTFS round to
        // two seconds, so a file re-fetched inside that window keeps its timestamp and changes its
        // size. The final assertion checks the staging worked, because a test that failed to move
        // the mtime back would pass for the wrong reason.
        let (corpus, scanner) = corpus("incremental-size");
        let film = corpus.root.join("Movies/Alien (1979).mkv");
        let first = scanner.scan(&corpus.root).unwrap();
        let known: HashMap<PathBuf, Stamp> = first
            .candidates
            .iter()
            .map(|file| (file.path.clone(), file.stamp()))
            .collect();

        let unchanged = scanner
            .scan(&corpus.root)
            .unwrap()
            .candidates
            .into_iter()
            .find(|file| file.path == film)
            .expect("the film is still there");
        assert_eq!(unchanged.freshness(&known), Freshness::Unchanged);

        let mtime = fs::metadata(&film).unwrap().modified().unwrap();
        fs::write(&film, b"a film, but longer now").unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&film)
            .unwrap()
            .set_modified(mtime)
            .unwrap();

        let second = scanner.scan(&corpus.root).unwrap();
        let rewritten = second
            .candidates
            .iter()
            .find(|file| file.path == film)
            .expect("a rewritten file is still a candidate");

        assert_eq!(fs::metadata(&film).unwrap().modified().unwrap(), mtime);
        assert!(rewritten.size_bytes > unchanged.size_bytes);
        assert_eq!(rewritten.freshness(&known), Freshness::Changed);
    }

    #[test]
    fn a_file_the_library_has_never_seen_is_new() {
        let (corpus, scanner) = corpus("incremental-new");
        let newcomer = corpus.file("Movies/Arrival (2016).mkv", b"a newer film");

        let outcome = scanner.scan(&corpus.root).unwrap();
        let known = HashMap::new();
        let found = outcome
            .candidates
            .iter()
            .find(|file| file.path == newcomer)
            .expect("the new film is a candidate");

        assert_eq!(found.freshness(&known), Freshness::New);
        assert!(found.freshness(&known).needs_work());
    }

    #[test]
    fn an_unchanged_scan_needs_no_work_at_all() {
        // The point of the pass: a library that has not changed is a scan that does nothing
        // expensive, whatever the size of the tree.
        let (corpus, scanner) = corpus("incremental-none");
        let first = scanner.scan(&corpus.root).unwrap();
        let known: HashMap<PathBuf, Stamp> = first
            .candidates
            .iter()
            .map(|file| (file.path.clone(), file.stamp()))
            .collect();

        let second = scanner.scan(&corpus.root).unwrap();

        assert!(
            second
                .candidates
                .iter()
                .all(|file| file.freshness(&known) == Freshness::Unchanged),
            "nothing changed between the two scans"
        );
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

    #[tokio::test]
    async fn the_bounded_walk_and_the_sequential_walk_agree_exactly() {
        // The property that makes two walks safe to have: whichever one ran, a scan's outcome is
        // the same. They differ by construction — one recurses while the other runs up to four
        // directories at once on the blocking pool — so the assertion is about the *outcome*, which
        // is why the outcome is sorted rather than left in whatever order the walk found things.
        let (corpus, scanner) = corpus("bounded-agrees");
        let cancel = CancellationToken::new();

        let sequential = scanner.scan(&corpus.root).unwrap();
        let bounded = scanner
            .scan_bounded(&corpus.root, 4, &cancel)
            .await
            .unwrap();

        assert_eq!(bounded, sequential);
        assert!(
            !bounded.candidates.is_empty(),
            "and the corpus has something to find"
        );
    }

    #[tokio::test]
    async fn a_bounded_walk_with_one_worker_at_a_time_is_still_the_same_walk() {
        // The bound is a bound, not a behaviour: `limit` of one has to be the sequential walk.
        let (corpus, scanner) = corpus("bounded-one");
        let cancel = CancellationToken::new();

        let sequential = scanner.scan(&corpus.root).unwrap();
        let bounded = scanner
            .scan_bounded(&corpus.root, 1, &cancel)
            .await
            .unwrap();

        assert_eq!(bounded, sequential);
    }

    #[tokio::test]
    async fn a_cancelled_walk_reports_cancellation_rather_than_a_partial_outcome() {
        // PG-E14-04's acceptance at its smallest: cancelled means "no answer", not "here is what I
        // got so far". A caller that reconciled from a partial answer would mark every file it had
        // not reached yet as missing.
        let (corpus, scanner) = corpus("bounded-cancelled");
        let cancel = CancellationToken::new();
        cancel.cancel();

        let outcome = scanner.scan_bounded(&corpus.root, 4, &cancel).await;

        assert!(matches!(outcome, Err(ScanError::Cancelled)), "{outcome:?}");
    }

    #[tokio::test]
    async fn a_walk_cancelled_after_the_first_level_still_refuses_to_answer() {
        // The same rule with work already done: the root was read, some files were found, and the
        // cancellation arrives before the tree is finished. What is asserted is that none of it is
        // handed back — the partial outcome does not escape.
        let (corpus, scanner) = corpus("bounded-cancelled-late");
        let cancel = CancellationToken::new();
        let cancelled = cancel.clone();
        let root = corpus.root.clone();
        let walker = tokio::spawn(async move { scanner.scan_bounded(&root, 4, &cancel).await });

        cancelled.cancel();
        let outcome = walker.await.expect("the walk task itself does not panic");

        assert!(matches!(outcome, Err(ScanError::Cancelled)), "{outcome:?}");
    }

    #[tokio::test]
    async fn a_bounded_walk_that_finishes_before_cancellation_still_answers() {
        // The other side of the same rule, so the cancellation check cannot be read as "a walk
        // where cancellation was ever requested refuses": one that finished first has an answer.
        let (corpus, scanner) = corpus("bounded-finished");
        let cancel = CancellationToken::new();

        let outcome = scanner
            .scan_bounded(&corpus.root, 4, &cancel)
            .await
            .unwrap();
        cancel.cancel();

        assert_eq!(outcome.candidates.len(), 2, "{outcome:?}");
    }

    #[tokio::test]
    async fn a_bounded_walk_of_something_that_is_not_there_still_reports_the_root() {
        // The one failure that stops a scan rather than being reported and stepped over.
        let (corpus, scanner) = corpus("bounded-missing");
        let cancel = CancellationToken::new();

        let outcome = scanner
            .scan_bounded(&corpus.root.join("NotPluggedIn"), 4, &cancel)
            .await;

        assert!(
            matches!(outcome, Err(ScanError::Root { .. })),
            "{outcome:?}"
        );
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
