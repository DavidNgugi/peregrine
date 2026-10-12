//! Managing the directories the scanner is allowed to walk (PG-E14-01).
//!
//! A library root is a row and a promise: the row says which directory, what it is expected to
//! hold, and when it is scanned; the promise is that removing it undoes nothing else. That second
//! half is the acceptance this module exists to keep, and it is enforced by *not writing* the
//! statements that would break it — the storage layer's [removal](peregrine_storage::Database::remove_library_root)
//! deletes one row of `library_root` and touches no other table.
//!
//! # What is checked here and what is not
//!
//! The path goes through [`PathGuard`] before it is stored, which is what `LibraryRoot`'s own doc
//! requires: it must be absolute and must not contain a `.` or `..` component, and the components
//! are checked for the reserved names and empty entries that make a path ambiguous. `PathGuard`
//! is a syntactic check by design — the domain has no filesystem — so nothing here resolves a
//! symlink or asks the disk anything.
//!
//! **Existence is deliberately not required.** A root whose drive is not mounted is a state this
//! application models rather than a mistake to refuse (PG-E14-16), and the scanner is what
//! reports what it cannot reach. Refusing an unmounted path here would make a library that lives
//! on a removable drive impossible to configure before plugging it in.
//!
//! **Overlapping roots are allowed.** Adding `/media` and `/media/Movies` scans some files twice,
//! which is what PG-E14-12's duplicate detection is for: it reports duplicates and never
//! deletes them. Refusing the second root here would silently take that decision away from the
//! user, who may well have a reason for two roots over one directory.
//!
//! **Classification is the user's.** `kind` says what the root is expected to hold, and guessing
//! it would mean walking the directory — which is the scanner's job (PG-E14-02) and would make
//! adding a root as slow as scanning one.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use jiff::Timestamp;
use peregrine_domain::{LibraryRoot, LibraryRootKind, PathGuard, PathGuardError, ScanMode};
use peregrine_storage::{Database, PageRequest, StorageError};
use thiserror::Error;

/// Why a root could not be added, changed, or removed.
#[derive(Debug, Error)]
pub enum RootError {
    /// The path is not one a root may be.
    #[error(transparent)]
    Path(#[from] PathGuardError),

    /// The path cannot be spelled as text, and the schema stores text.
    ///
    /// `PathGuard` refuses a component it cannot read as UTF-8, so a path that got this far is
    /// text and this is unreachable today. It is an error rather than an `expect` because the
    /// alternative to a typed refusal is a panic on a user's machine, and the cost of the variant
    /// is one line.
    #[error("the folder `{}` is not a path this build can store", .path.display())]
    NotText {
        /// The folder as the user chose it.
        path: PathBuf,
    },

    /// That folder is already a library root.
    #[error("`{}` is already one of your libraries", .path.display())]
    AlreadyARoot {
        /// The folder.
        path: PathBuf,
        /// The row that already holds it, so a caller can point at it rather than just refuse.
        id: i64,
    },

    /// The database refused.
    #[error(transparent)]
    Storage(#[from] StorageError),
}

/// A root as a page of them is listed, re-exported so a caller of this module needs no second edge
/// to storage to name what it was handed (AGENTS R6).
pub use peregrine_storage::LibraryRootRow;

/// The library's roots, and the operations that change them.
#[derive(Debug, Clone)]
pub struct LibraryService {
    database: Arc<Database>,
}

impl LibraryService {
    /// A service over the open database.
    #[must_use]
    pub fn new(database: Arc<Database>) -> Self {
        Self { database }
    }

    /// Adds `path` as a root, or explains why it cannot be one.
    ///
    /// # Errors
    ///
    /// [`RootError::Path`] when the path is relative, contains a `.` or `..`, or fails the
    /// component policy; [`RootError::AlreadyARoot`] when the folder is already a root, in which
    /// case the error carries the existing row's id; [`RootError::NotText`] when the path cannot
    /// be stored; and [`RootError::Storage`] when the row cannot be written.
    pub async fn add_root(
        &self,
        path: &Path,
        kind: LibraryRootKind,
        scan_mode: ScanMode,
        at: Timestamp,
    ) -> Result<LibraryRoot, RootError> {
        let guard = PathGuard::new(path)?;
        let root = guard.root();
        let stored = root.to_str().ok_or_else(|| RootError::NotText {
            path: root.to_path_buf(),
        })?;

        // The lookup gives the common case a message naming the row that already holds the folder;
        // the schema's `UNIQUE (path)` is what actually stops a duplicate, so a lost race against
        // another caller arrives as `RootError::Storage` rather than as a silent second root.
        if let Some(existing) = self.database.library_root_by_path(stored).await? {
            return Err(RootError::AlreadyARoot {
                path: root.to_path_buf(),
                id: existing.id,
            });
        }

        let draft = LibraryRoot {
            id: 0,
            path: text(stored)?,
            kind,
            scan_mode,
            last_scan_at: None,
            created_at: at,
        };
        let id = self.database.insert_library_root(draft).await?;

        self.database
            .library_root(id)
            .await?
            .ok_or(RootError::Storage(StorageError::WriterStopped))
    }

    /// Removes a root, reporting whether it was there.
    ///
    /// What this does *not* do is the point: no media item, file, subtitle, or watch-progress row
    /// is touched, and no file on disk is deleted. A user who removes a folder has said "stop
    /// scanning this", not "forget what I watched" — and the second would be unrecoverable.
    ///
    /// # Errors
    ///
    /// [`RootError::Storage`] when the row cannot be removed.
    pub async fn remove_root(&self, id: i64) -> Result<bool, RootError> {
        Ok(self.database.remove_library_root(id).await?)
    }

    /// Changes when a root is scanned, reporting whether it was there.
    ///
    /// # Errors
    ///
    /// [`RootError::Storage`] when the row cannot be changed.
    pub async fn set_scan_mode(&self, id: i64, mode: ScanMode) -> Result<bool, RootError> {
        Ok(self.database.set_root_scan_mode(id, mode).await?)
    }

    /// A page of roots, newest first.
    ///
    /// # Errors
    ///
    /// [`RootError::Storage`] when the page cannot be read.
    pub async fn roots(&self, request: PageRequest) -> Result<Vec<LibraryRootRow>, RootError> {
        Ok(self.database.list_library_roots(request).await?)
    }
}

/// Wraps a path as the non-empty text the schema stores.
///
/// `PathGuard` has already refused an empty root, so this cannot fail for a path that reached it;
/// it returns the storage error rather than unwrapping so the call site stays free of panics.
fn text(value: &str) -> Result<peregrine_domain::NonEmptyText, RootError> {
    peregrine_domain::NonEmptyText::new(value).map_err(|_| RootError::NotText {
        path: PathBuf::from(value),
    })
}

#[cfg(test)]
mod tests {
    use peregrine_domain::AccountId;
    use peregrine_storage::PageRequest;
    use tempfile::TempDir;

    use super::*;

    const NOW: i64 = 1_700_000_000_000;

    async fn service(directory: &TempDir) -> LibraryService {
        let database = Database::open(directory.path().join("peregrine.sqlite"))
            .await
            .expect("the scratch directory is writable");
        LibraryService::new(Arc::new(database))
    }

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millisecond(millis).expect("the tests stay inside jiff's range")
    }

    #[tokio::test]
    async fn a_folder_can_be_added_and_listed() {
        let directory = TempDir::new().unwrap();
        let service = service(&directory).await;
        let movies = directory.path().join("Movies");
        std::fs::create_dir_all(&movies).unwrap();

        let added = service
            .add_root(
                &movies,
                LibraryRootKind::Movies,
                ScanMode::Incremental,
                at(NOW),
            )
            .await
            .expect("a real folder under a temp directory is a root");

        assert!(added.id > 0);
        assert_eq!(added.kind, LibraryRootKind::Movies);
        assert_eq!(added.scan_mode, ScanMode::Incremental);
        assert_eq!(added.path.as_str(), movies.to_str().unwrap());

        let listed = service.roots(PageRequest::first(10)).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].root.id, added.id);
    }

    #[tokio::test]
    async fn a_folder_that_is_already_a_root_is_refused_by_name() {
        // The error carries the row that already holds it, so a UI can point at it rather than
        // only saying no.
        let directory = TempDir::new().unwrap();
        let service = service(&directory).await;
        let movies = directory.path().join("Movies");
        std::fs::create_dir_all(&movies).unwrap();
        let first = service
            .add_root(
                &movies,
                LibraryRootKind::Movies,
                ScanMode::Incremental,
                at(NOW),
            )
            .await
            .unwrap();

        let refused = service
            .add_root(&movies, LibraryRootKind::Shows, ScanMode::Manual, at(NOW))
            .await;

        match refused {
            Err(RootError::AlreadyARoot { id, path }) => {
                assert_eq!(id, first.id);
                assert_eq!(path, movies);
            }
            other => panic!("expected a refusal by name, got {other:?}"),
        }
        assert_eq!(
            service.roots(PageRequest::first(10)).await.unwrap().len(),
            1,
            "the refused add wrote nothing"
        );
    }

    #[tokio::test]
    async fn a_relative_folder_is_refused() {
        // The scanner walks whatever it is given, so a relative root would resolve against
        // whatever the process's working directory happened to be.
        let directory = TempDir::new().unwrap();
        let service = service(&directory).await;

        let refused = service
            .add_root(
                Path::new("Movies"),
                LibraryRootKind::Movies,
                ScanMode::Incremental,
                at(NOW),
            )
            .await;

        assert!(matches!(refused, Err(RootError::Path(_))), "{refused:?}");
        assert_eq!(
            service.roots(PageRequest::first(10)).await.unwrap(),
            Vec::new()
        );
    }

    #[tokio::test]
    async fn a_folder_with_a_parent_component_is_refused() {
        // `/media/Movies/../Movies` names the same directory as `/media/Movies` but is a different
        // string, and two spellings of one root are two UNIQUE paths and two scans.
        let directory = TempDir::new().unwrap();
        let service = service(&directory).await;
        let wandering = directory.path().join("Movies").join("..").join("Movies");

        let refused = service
            .add_root(
                &wandering,
                LibraryRootKind::Movies,
                ScanMode::Incremental,
                at(NOW),
            )
            .await;

        assert!(matches!(refused, Err(RootError::Path(_))), "{refused:?}");
    }

    #[tokio::test]
    async fn a_folder_that_is_not_there_can_still_be_added() {
        // A library on a drive that is not mounted is a state this build models (PG-E14-16), and
        // the scanner is what reports it. Refusing here would make a removable library impossible
        // to configure before the drive is plugged in.
        let directory = TempDir::new().unwrap();
        let service = service(&directory).await;
        let absent = directory.path().join("NotPluggedIn");

        let added = service
            .add_root(&absent, LibraryRootKind::Movies, ScanMode::Manual, at(NOW))
            .await;

        assert!(added.is_ok(), "{added:?}");
    }

    #[tokio::test]
    async fn two_roots_over_one_directory_are_both_allowed() {
        // PG-E14-12 reports duplicates rather than the schema refusing them: a user may keep
        // `/media` and `/media/Movies` for reasons of their own, and the cost is a second scan of
        // the same files, not a wrong answer.
        let directory = TempDir::new().unwrap();
        let service = service(&directory).await;
        let media = directory.path().join("media");
        let movies = media.join("Movies");
        std::fs::create_dir_all(&movies).unwrap();

        service
            .add_root(
                &media,
                LibraryRootKind::Mixed,
                ScanMode::Incremental,
                at(NOW),
            )
            .await
            .expect("the parent is a root");
        let nested = service
            .add_root(
                &movies,
                LibraryRootKind::Movies,
                ScanMode::Incremental,
                at(NOW),
            )
            .await;

        assert!(nested.is_ok(), "a nested root is allowed: {nested:?}");
        assert_eq!(
            service.roots(PageRequest::first(10)).await.unwrap().len(),
            2
        );
    }

    #[tokio::test]
    async fn removing_a_root_reports_whether_it_was_there() {
        let directory = TempDir::new().unwrap();
        let service = service(&directory).await;
        let movies = directory.path().join("Movies");
        std::fs::create_dir_all(&movies).unwrap();
        let added = service
            .add_root(
                &movies,
                LibraryRootKind::Movies,
                ScanMode::Incremental,
                at(NOW),
            )
            .await
            .unwrap();

        assert!(service.remove_root(added.id).await.unwrap());
        assert!(!service.remove_root(added.id).await.unwrap());
        assert_eq!(
            service.roots(PageRequest::first(10)).await.unwrap(),
            Vec::new()
        );
    }

    #[tokio::test]
    async fn removing_a_root_leaves_the_folder_on_disk() {
        // The service half of PG-E14-01's acceptance. The storage half proves the rows; this
        // proves the thing a user would notice, at the layer that has a filesystem to notice it on.
        let directory = TempDir::new().unwrap();
        let service = service(&directory).await;
        let movies = directory.path().join("Movies");
        std::fs::create_dir_all(&movies).unwrap();
        let inside = movies.join("Alien (1979).mkv");
        std::fs::write(&inside, b"not really a matroska file").unwrap();
        let added = service
            .add_root(
                &movies,
                LibraryRootKind::Movies,
                ScanMode::Incremental,
                at(NOW),
            )
            .await
            .unwrap();

        service.remove_root(added.id).await.unwrap();

        assert!(inside.is_file(), "the user's files are the user's");
    }

    #[tokio::test]
    async fn a_roots_scan_mode_can_be_changed_and_a_missing_root_is_reported() {
        let directory = TempDir::new().unwrap();
        let service = service(&directory).await;
        let movies = directory.path().join("Movies");
        std::fs::create_dir_all(&movies).unwrap();
        let added = service
            .add_root(
                &movies,
                LibraryRootKind::Movies,
                ScanMode::Incremental,
                at(NOW),
            )
            .await
            .unwrap();

        assert!(
            service
                .set_scan_mode(added.id, ScanMode::Manual)
                .await
                .unwrap()
        );
        assert!(
            !service
                .set_scan_mode(added.id + 1, ScanMode::Manual)
                .await
                .unwrap()
        );
        assert_eq!(
            service.roots(PageRequest::first(10)).await.unwrap()[0]
                .root
                .scan_mode,
            ScanMode::Manual
        );
    }

    #[tokio::test]
    async fn the_added_root_records_when_it_was_added() {
        let directory = TempDir::new().unwrap();
        let service = service(&directory).await;
        let movies = directory.path().join("Movies");
        std::fs::create_dir_all(&movies).unwrap();

        let added = service
            .add_root(
                &movies,
                LibraryRootKind::Shows,
                ScanMode::Scheduled,
                at(NOW),
            )
            .await
            .unwrap();

        assert_eq!(added.created_at, at(NOW));
        assert_eq!(
            added.last_scan_at, None,
            "a root that was never scanned says so"
        );
    }

    #[tokio::test]
    async fn a_root_is_not_scoped_to_an_account() {
        // A root is a directory the user configured, not content: two accounts see the same
        // libraries, and the scope applies to what is inside them.
        let directory = TempDir::new().unwrap();
        let service = service(&directory).await;
        let movies = directory.path().join("Movies");
        std::fs::create_dir_all(&movies).unwrap();
        service
            .add_root(
                &movies,
                LibraryRootKind::Movies,
                ScanMode::Incremental,
                at(NOW),
            )
            .await
            .unwrap();

        let _ = AccountId::new(1).expect("account one exists");
        assert_eq!(
            service.roots(PageRequest::first(10)).await.unwrap().len(),
            1
        );
    }
}
