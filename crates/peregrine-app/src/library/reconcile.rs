//! Marking the files that went away, and clearing the ones that came back (PG-E14-13).
//!
//! A scan that finds a file the library does not know creates a row. A scan that *does not* find a
//! file the library knows is the harder case, and this module is the answer to it: the row is kept
//! and stamped with [`missing_since`](peregrine_domain::MediaFile), so nothing the user watched is
//! forgotten, and a file that comes back — a drive plugged in again, a download re-fetched — has
//! its stamp cleared instead of being indexed a second time as a stranger.
//!
//! # What this never does
//!
//! **It never deletes.** Not the row, not the file, not the progress. `docs/FLOWS.md` F8 states the
//! rule for the reconciliation step ("never deletes watch progress or catalogue rows when a file
//! disappears") and this is where that sentence lives in code. Deleting a `media_file` row would
//! take its `subtitle_track` rows with it (`ON DELETE CASCADE`) and would orphan the watch progress
//! that hangs off the *item* the file belonged to, which is the one thing a user cannot recreate.
//!
//! **It is only ever called with a scan that ran.** This is the module's most important precondition
//! and it is why [`Scanner::scan`](crate::library::Scanner::scan) returns an error for a root it
//! cannot read instead of an empty outcome: an unreadable root is not evidence that every file in
//! it is gone, and reconciling against an empty scan would stamp a whole library missing the moment
//! a drive was unplugged. PG-E14-16 owns the unreachable-root state; what this module owes it is
//! never being reachable from a failed walk, which the types already make true — an error is not an
//! outcome.

use std::collections::HashSet;

use jiff::Timestamp;
use tracing::{debug, info};

use crate::library::roots::{LibraryError, LibraryService};
use crate::library::scan::CandidateFile;

/// What one reconciliation changed.
///
/// Paths rather than row ids, because every caller of this is either a log line or a scan report,
/// and a path is what the user searches for when their film is not there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reconciliation {
    /// The files that were there last time and were not seen this time.
    pub marked_missing: Vec<String>,
    /// The files that were marked missing and were seen again.
    pub restored: Vec<String>,
}

impl Reconciliation {
    /// Whether the scan saw exactly what the library already knew.
    #[must_use]
    pub const fn is_unchanged(&self) -> bool {
        self.marked_missing.is_empty() && self.restored.is_empty()
    }
}

impl LibraryService {
    /// Marks the files of `root_id` that `seen` did not contain, and restores the ones it did.
    ///
    /// `seen` is a scan's candidate list — what a walk of the root found — and `at` is the instant
    /// the scan ran, which is the caller's because [`MediaFile::mark_missing`] keeps the *first*
    /// instant it is given: a file that stays gone keeps saying when the library first noticed
    /// (`DATA_MODEL` I11), while a file that comes back has nothing to remember.
    ///
    /// # Errors
    ///
    /// [`LibraryError::Storage`] when either side cannot be read or written. A single row that
    /// cannot be written fails the reconciliation rather than being skipped: a half-applied
    /// reconciliation is a library whose report disagrees with its rows, and the next scan will
    /// retry the whole thing.
    ///
    /// [`MediaFile::mark_missing`]: peregrine_domain::MediaFile::mark_missing
    pub async fn reconcile(
        &self,
        root_id: i64,
        seen: &[CandidateFile],
        at: Timestamp,
    ) -> Result<Reconciliation, LibraryError> {
        let database = self.database();
        // Lossy is not loss here: `ScanPolicy` refuses a path whose own name or any component is
        // not UTF-8, so every candidate that reaches this point has text on both sides.
        let seen: HashSet<String> = seen
            .iter()
            .map(|file| file.path.to_string_lossy().into_owned())
            .collect();

        let mut report = Reconciliation::default();

        for recorded in database.present_files(root_id).await? {
            if seen.contains(recorded.path.as_str()) {
                continue;
            }
            // The row stays; the stamp is the whole change. `updated_at` is the scan's instant
            // rather than the file's, because nothing about the file changed — what changed is
            // what the library knows.
            if database
                .set_media_file_missing(recorded.id, Some(at.as_millisecond()), at.as_millisecond())
                .await?
            {
                debug!(path = %recorded.path, "a file the library had is gone");
                report.marked_missing.push(recorded.path);
            }
        }

        for recorded in database.missing_files(root_id).await? {
            if !seen.contains(recorded.path.as_str()) {
                continue;
            }
            if database
                .set_media_file_missing(recorded.id, None, at.as_millisecond())
                .await?
            {
                debug!(path = %recorded.path, "a file the library had lost is back");
                report.restored.push(recorded.path);
            }
        }

        if !report.is_unchanged() {
            info!(
                root_id,
                missing = report.marked_missing.len(),
                restored = report.restored.len(),
                "reconciled the library against a scan"
            );
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;

    use peregrine_domain::{
        AccountId, AccountScope, LibraryRoot, LibraryRootKind, MediaFile, MediaItem,
        MediaItemState, MediaOrigin, NonEmptyText, ScanMode, TitleKind, TitleSubtype,
    };
    use peregrine_storage::{Database, StubTitle};
    use tempfile::TempDir;

    use peregrine_domain::ScanPolicy;

    use crate::library::scan::Scanner;

    use super::*;

    const NOW: i64 = 1_700_000_000_000;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millisecond(millis).expect("the tests stay inside jiff's range")
    }

    fn text(value: &str) -> NonEmptyText {
        NonEmptyText::new(value).expect("fixture text is not empty")
    }

    /// A library of one root and one film.
    ///
    /// The watch-progress half of PG-E14-13's acceptance is proven in `peregrine-storage`, where a
    /// test can create an account and a title with the crate's own helpers. What lives here is the
    /// decision — which rows a scan stamps and which it clears — which needs a root, an item, and a
    /// file and nothing else.
    struct Library {
        directory: TempDir,
        service: LibraryService,
        database: Arc<Database>,
        root_id: i64,
        item_id: i64,
        file_id: i64,
        film: PathBuf,
    }

    async fn library(name: &str) -> Library {
        let directory = TempDir::new().unwrap();
        let media = directory.path().join(format!("Movies-{name}"));
        fs::create_dir_all(&media).unwrap();
        let film = media.join("Alien (1979).mkv");
        fs::write(&film, b"a film").unwrap();

        let database = Arc::new(
            Database::open(directory.path().join("peregrine.sqlite"))
                .await
                .unwrap(),
        );
        let root_id = database
            .insert_library_root(LibraryRoot {
                id: 0,
                path: text(media.to_str().unwrap()),
                kind: LibraryRootKind::Movies,
                scan_mode: ScanMode::Incremental,
                last_scan_at: None,
                created_at: at(NOW),
            })
            .await
            .unwrap();

        // A stub title is what the catalogue holds before a provider sync fills it in, and it is
        // the row a media item hangs off; without one the item's foreign key has nothing to point
        // at.
        let title_id = database
            .insert_stub_title(StubTitle {
                kind: TitleKind::Movie,
                subtype: TitleSubtype::Standard,
                name: "Alien".to_owned(),
                original_name: None,
                sort_name: "alien".to_owned(),
                at: NOW,
            })
            .await
            .unwrap();
        let item_id = database
            .insert_media_item(MediaItem {
                id: 0,
                title_id,
                episode_id: None,
                state: MediaItemState::Available,
                origin: MediaOrigin::Scan,
                preferred_file_id: None,
                quality_label: None,
                created_at: at(NOW),
                updated_at: at(NOW),
            })
            .await
            .unwrap();
        let size = fs::metadata(&film).unwrap().len();
        let file_id = database
            .insert_media_file(MediaFile {
                id: 0,
                media_item_id: item_id,
                path: text(film.to_str().unwrap()),
                root_id: Some(root_id),
                size_bytes: size,
                mtime_ms: at(NOW),
                added_at: at(NOW),
                missing_since: None,
                container: None,
                duration_ms: None,
                video_codec: None,
                video_profile: None,
                width: None,
                height: None,
                bit_depth: None,
                hdr_format: None,
                frame_rate: None,
                bitrate_kbps: None,
                audio_codec: None,
                audio_channels: None,
                audio_layout: None,
                audio_languages: None,
                subtitle_format: None,
                probe_version: None,
                probe_json: None,
                quality_label: None,
                updated_at: at(NOW),
            })
            .await
            .unwrap();

        let service = LibraryService::new(Arc::clone(&database));
        Library {
            directory,
            service,
            database,
            root_id,
            item_id,
            file_id,
            film,
        }
    }

    /// The scope the fixtures are read with: an account with no ceiling and no curation.
    fn any_account() -> AccountScope {
        AccountScope::unrestricted(AccountId::new(1).expect("account one"))
    }

    impl Library {
        /// What a scan of the whole directory sees, which for these fixtures is the film.
        fn scan(&self) -> Vec<CandidateFile> {
            Scanner::new(ScanPolicy::new())
                .scan(self.directory.path())
                .unwrap()
                .candidates
        }

        async fn row(&self) -> MediaFile {
            self.database
                .media_file(any_account(), self.file_id)
                .await
                .unwrap()
                .expect("the row is never deleted")
        }
    }

    #[tokio::test]
    async fn a_file_that_disappears_is_stamped_missing_rather_than_deleted() {
        // The direction that loses data if it is wrong. The row survives with an instant on it, and
        // the item it belonged to is untouched.
        let library = library("disappears").await;
        assert_eq!(library.scan().len(), 1, "the film is found to begin with");

        fs::remove_file(&library.film).unwrap();
        let report = library
            .service
            .reconcile(library.root_id, &[], at(NOW + 1_000))
            .await
            .unwrap();

        assert_eq!(report.marked_missing.len(), 1, "{report:?}");
        assert_eq!(report.restored, Vec::<String>::new());
        let row = library.row().await;
        assert_eq!(row.missing_since, Some(at(NOW + 1_000)));
        assert_eq!(
            row.media_item_id, library.item_id,
            "the item still holds it"
        );
        assert!(
            library
                .database
                .media_item(any_account(), library.item_id)
                .await
                .unwrap()
                .is_some(),
            "and the item is still there for progress to hang off"
        );
    }

    #[tokio::test]
    async fn a_file_that_comes_back_is_the_same_row() {
        // The reason the row is kept: a restore must not create a second item, because the user's
        // position is attached to the first one.
        let library = library("comes-back").await;

        fs::remove_file(&library.film).unwrap();
        library
            .service
            .reconcile(library.root_id, &[], at(NOW + 1_000))
            .await
            .unwrap();
        assert_eq!(library.row().await.missing_since, Some(at(NOW + 1_000)));

        fs::write(&library.film, b"a film").unwrap();
        let report = library
            .service
            .reconcile(library.root_id, &library.scan(), at(NOW + 2_000))
            .await
            .unwrap();

        assert_eq!(report.restored.len(), 1, "{report:?}");
        let row = library.row().await;
        assert_eq!(row.id, library.file_id, "the same row, not a new one");
        assert_eq!(row.media_item_id, library.item_id, "the same item");
        assert_eq!(row.missing_since, None);
    }

    #[tokio::test]
    async fn a_scan_that_sees_everything_changes_nothing() {
        let library = library("unchanged").await;
        let seen = library.scan();

        let report = library
            .service
            .reconcile(library.root_id, &seen, at(NOW + 1_000))
            .await
            .unwrap();

        assert!(report.is_unchanged(), "{report:?}");
        assert_eq!(library.row().await.missing_since, None);
    }

    #[tokio::test]
    async fn a_file_that_stays_gone_keeps_the_first_instant_it_was_noticed() {
        // `DATA_MODEL` I11: a library missing a file for a month says so, rather than saying "a
        // minute ago" on every scan.
        let library = library("stays-gone").await;
        library
            .service
            .reconcile(library.root_id, &[], at(NOW + 1_000))
            .await
            .unwrap();

        let second = library
            .service
            .reconcile(library.root_id, &[], at(NOW + 500_000))
            .await
            .unwrap();

        assert!(
            second.marked_missing.is_empty(),
            "a file already stamped is not stamped again: {second:?}"
        );
        assert_eq!(library.row().await.missing_since, Some(at(NOW + 1_000)));
    }

    #[tokio::test]
    async fn reconciling_one_root_says_nothing_about_another() {
        // A scan walks one root. If a reconciliation swept the whole table, adding a second root
        // would mark every file of the first one missing the first time it was scanned.
        let library = library("other-root").await;
        let other = library.directory.path().join("Shows");
        fs::create_dir_all(&other).unwrap();
        let other_id = library
            .database
            .insert_library_root(LibraryRoot {
                id: 0,
                path: text(other.to_str().unwrap()),
                kind: LibraryRootKind::Shows,
                scan_mode: ScanMode::Incremental,
                last_scan_at: None,
                created_at: at(NOW),
            })
            .await
            .unwrap();

        let report = library
            .service
            .reconcile(other_id, &[], at(NOW + 1_000))
            .await
            .unwrap();

        assert!(report.is_unchanged(), "{report:?}");
        assert_eq!(
            library.row().await.missing_since,
            None,
            "the film belongs to the other root and is still there"
        );
    }

    #[tokio::test]
    async fn the_report_names_the_paths_a_user_would_search_for() {
        let library = library("report").await;

        let report = library
            .service
            .reconcile(library.root_id, &[], at(NOW + 1_000))
            .await
            .unwrap();

        assert_eq!(
            report.marked_missing,
            vec![library.film.to_string_lossy().into_owned()]
        );
    }

    #[tokio::test]
    async fn a_root_with_nothing_indexed_reconciles_to_nothing() {
        // The empty case: a reconciliation must not invent rows, and a root the library has never
        // indexed has nothing to mark and nothing to restore.
        let library = library("empty-root").await;
        let empty = library.directory.path().join("Empty");
        fs::create_dir_all(&empty).unwrap();
        let empty_id = library
            .database
            .insert_library_root(LibraryRoot {
                id: 0,
                path: text(empty.to_str().unwrap()),
                kind: LibraryRootKind::Movies,
                scan_mode: ScanMode::Incremental,
                last_scan_at: None,
                created_at: at(NOW),
            })
            .await
            .unwrap();

        let report = library
            .service
            .reconcile(empty_id, &[], at(NOW + 1_000))
            .await
            .unwrap();

        assert!(report.is_unchanged(), "{report:?}");
    }
}
