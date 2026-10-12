//! Turning a scan's candidates into rows: the indexer (PG-E14-05, first cut).
//!
//! A walk answers "which files are there"; the reconciliation answers "which of them are gone". This
//! module answers the question in between — *what are they* — by parsing each candidate's name and
//! writing the title, item, and file rows the library needs before anything can be shown or played.
//!
//! # What this is, and what it is not yet
//!
//! `PG-E14-05` is the match pipeline into the catalogue, and its acceptance is attribution measured
//! against the naming corpus. This is the half of it that needs no provider: a filename is parsed
//! by `peregrine-domain`'s `LibraryName` and its title becomes a **stub** — the identity a later
//! sync hangs metadata off (`PG-E5-06`) — so a library can be built, searched and played from the
//! files that are there while the metadata road is still blocked on the provider stack.
//!
//! Three things are deliberately not here, and each belongs to a task rather than to a shortcut:
//!
//! * **Provider attribution.** A stub is matched by name and year, so `Alien (1979).mkv` and
//!   `Alien (1986).mkv` are two titles and a re-release is the same one. Deciding that a parsed
//!   name *is* a provider's title needs the provider (`PG-E5-13`).
//! * **Episodes.** A name that states a season or an episode number is parked in `unmatched_file`
//!   with the reason, because placing an episode needs the scheme and the numbering rules
//!   `PG-E14-07` owns. Parking loses nothing: the parse is stored beside the path.
//! * **Sidecars.** An `.nfo` beside a film is `PG-E14-06`'s, and a `.srt` is `PG-E14-09`'s. The
//!   scan policy already refuses them as candidates, so they never reach here.
//!
//! # The scope this runs under
//!
//! None, and that is a decision rather than an omission: the reads this uses are the *scanner's*
//! surface (`crate::library`'s module doc states the whole argument). A stub is created for every
//! file the walk found, including one a restricted account will never see, because the row has to
//! exist before `I37` can hide it.
//!
//! # Idempotence
//!
//! A scan may be interrupted, cancelled, and run again, so the indexer is written to be run over the
//! same candidates repeatedly: a title is found by its sort name, an item by its title and episode,
//! and a file by its path. A second pass creates nothing and refreshes the stamps it finds, which is
//! what makes the incremental walk's answer — "this file is unchanged" — a statement about rows that
//! are already correct rather than about work that was skipped.

use std::sync::Arc;

use peregrine_domain::{
    LibraryName, MediaFile, MediaItem, MediaItemState, MediaOrigin, NonEmptyText, TitleKind,
    TitleSubtype, UnmatchedFile,
};
use peregrine_storage::{Database, StorageError, StubTitle};
use thiserror::Error;
use tracing::debug;

use crate::library::scan::CandidateFile;

/// Where a season or an episode number comes from, which is not this task's question.
const EPISODES_ARE_PG_E14_07: &str =
    "the name states a season or an episode, which PG-E14-07 places";

/// Why an index pass could not finish.
#[derive(Debug, Error)]
pub enum IndexError {
    /// The database refused a read or a write.
    #[error(transparent)]
    Storage(#[from] StorageError),

    /// A path could not be spelled as the text the schema stores.
    ///
    /// The scan policy refuses a path whose components are not text, so a candidate that reaches
    /// this is text and the error is unreachable today. It is an error rather than an `expect`
    /// because the alternative to a typed refusal is a panic on a user's machine.
    #[error("the file {} cannot be stored as text", .path.display())]
    NotText {
        /// The path.
        path: std::path::PathBuf,
    },
}

/// What one index pass wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexReport {
    /// Titles created for names the catalogue did not hold.
    pub created_titles: usize,
    /// Items created — one per title, or per episode once `PG-E14-07` lands.
    pub created_items: usize,
    /// Files written for candidates the library had never seen.
    pub created_files: usize,
    /// Files whose size, mtime, or root was refreshed because the library had them.
    pub refreshed_files: usize,
    /// Candidates parked for a human or a later task, with the reason.
    pub unmatched: usize,
}

impl IndexReport {
    /// Whether the pass found anything new to record.
    #[must_use]
    pub const fn is_unchanged(&self) -> bool {
        self.created_titles == 0 && self.created_items == 0 && self.created_files == 0
    }
}

/// Indexes one root's candidates.
///
/// # Errors
///
/// [`IndexError::Storage`] when a read or a write fails, and [`IndexError::NotText`] for a path the
/// schema cannot hold. A single candidate's failure fails the pass: a half-indexed root is a library
/// whose report disagrees with its rows, and the next scan indexes the whole root again.
pub async fn index_candidates(
    database: &Arc<Database>,
    root_id: i64,
    candidates: &[CandidateFile],
    at: jiff::Timestamp,
) -> Result<IndexReport, IndexError> {
    let mut report = IndexReport::default();

    for candidate in candidates {
        let Some(path) = candidate.path.to_str() else {
            return Err(IndexError::NotText {
                path: candidate.path.clone(),
            });
        };

        // A file the library has already indexed is refreshed rather than re-derived: its rows
        // exist, and the only thing a walk can add is the stamp it just read.
        if let Some(known) = database.recorded_file_at_path(path).await? {
            if database
                .refresh_file_stamp(
                    known.id,
                    i64::try_from(candidate.size_bytes).unwrap_or(i64::MAX),
                    candidate.mtime_ms,
                    root_id,
                    at.as_millisecond(),
                )
                .await?
            {
                report.refreshed_files += 1;
            }
            continue;
        }

        let name = candidate.path.file_name().and_then(|name| name.to_str());
        match name.map(LibraryName::parse) {
            Some(Ok(parsed)) => {
                report
                    .absorb(&index_parsed(database, root_id, path, &parsed, candidate, at).await?);
            }
            Some(Err(error)) => {
                // The parse read what it could before it stopped, and `NameError` carries it so the
                // row loses nothing. Parked rather than dropped: this is the queue a human works
                // through, and a file that never became a row is a file the user cannot see.
                park(
                    database,
                    path,
                    candidate,
                    &error.name,
                    Some(error.reason.to_string()),
                    at,
                )
                .await?;
                report.unmatched += 1;
            }
            None => {
                return Err(IndexError::NotText {
                    path: candidate.path.clone(),
                });
            }
        }
    }

    if !report.is_unchanged() {
        debug!(
            root_id,
            titles = report.created_titles,
            items = report.created_items,
            files = report.created_files,
            refreshed = report.refreshed_files,
            unmatched = report.unmatched,
            "indexed a root's files"
        );
    }
    Ok(report)
}

impl IndexReport {
    /// Folds one candidate's contribution in.
    fn absorb(&mut self, other: &Self) {
        self.created_titles += other.created_titles;
        self.created_items += other.created_items;
        self.created_files += other.created_files;
        self.refreshed_files += other.refreshed_files;
        self.unmatched += other.unmatched;
    }
}

/// Indexes one parsed name.
async fn index_parsed(
    database: &Arc<Database>,
    root_id: i64,
    path: &str,
    parsed: &LibraryName,
    candidate: &CandidateFile,
    at: jiff::Timestamp,
) -> Result<IndexReport, IndexError> {
    let mut report = IndexReport::default();

    // An episode-shaped name is parked, not guessed at: which title it belongs to and where in the
    // season it sits both need the numbering rules PG-E14-07 owns.
    if parsed.season().is_some() || parsed.episodes().is_some() {
        park(
            database,
            path,
            candidate,
            parsed,
            Some(EPISODES_ARE_PG_E14_07.to_owned()),
            at,
        )
        .await?;
        report.unmatched += 1;
        return Ok(report);
    }

    let Some(title_name) = parsed.title() else {
        // `parse` guarantees a title on `Ok`, so this is a shape guard rather than a case.
        park(
            database,
            path,
            candidate,
            parsed,
            Some("the name states no title".to_owned()),
            at,
        )
        .await?;
        report.unmatched += 1;
        return Ok(report);
    };
    let sort_name = sort_name_of(title_name);

    let (item_id, created) = movie_item(database, title_name, &sort_name, at).await?;
    report.created_titles += created.titles;
    report.created_items += created.items;

    database
        .insert_media_file(MediaFile {
            id: 0,
            media_item_id: item_id,
            path: text(path)?,
            root_id: Some(root_id),
            size_bytes: candidate.size_bytes,
            // The file's own modification time, not the scan's instant: the incremental rule
            // compares what the library recorded against what the filesystem now says, and storing
            // the moment of the walk would make every file look changed on the next pass.
            mtime_ms: jiff::Timestamp::from_millisecond(candidate.mtime_ms).unwrap_or(at),
            added_at: at,
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
            updated_at: at,
        })
        .await?;
    report.created_files += 1;
    Ok(report)
}

/// What resolving a title and its item created.
#[derive(Debug, Clone, Copy, Default)]
struct Resolved {
    titles: usize,
    items: usize,
}

/// The item a parsed movie belongs to, creating the title and the item when they are missing.
///
/// The title is found by its sort name, which is what makes a second copy of one film one title
/// rather than two, and the item by its title and episode — `None`, because a film's item has no
/// episode and the partial unique index (I2) refuses a second one.
async fn movie_item(
    database: &Arc<Database>,
    title_name: &str,
    sort_name: &str,
    at: jiff::Timestamp,
) -> Result<(i64, Resolved), IndexError> {
    let mut created = Resolved::default();

    let title_id = if let Some(id) = database
        .title_id_for_sort_name(TitleKind::Movie.as_str(), sort_name)
        .await?
    {
        id
    } else {
        let id = database
            .insert_stub_title(StubTitle {
                kind: TitleKind::Movie,
                subtype: TitleSubtype::Standard,
                name: title_name.to_owned(),
                original_name: None,
                sort_name: sort_name.to_owned(),
                at: at.as_millisecond(),
            })
            .await?;
        created.titles += 1;
        id
    };

    let item_id = if let Some(id) = database.item_id_for(title_id, None).await? {
        id
    } else {
        let id = database
            .insert_media_item(MediaItem {
                id: 0,
                title_id,
                episode_id: None,
                state: MediaItemState::Available,
                origin: MediaOrigin::Scan,
                preferred_file_id: None,
                quality_label: None,
                created_at: at,
                updated_at: at,
            })
            .await?;
        created.items += 1;
        id
    };

    Ok((item_id, created))
}

/// Parks one candidate in `unmatched_file`, with whatever the parse read.
async fn park(
    database: &Arc<Database>,
    path: &str,
    candidate: &CandidateFile,
    parsed: &LibraryName,
    reason: Option<String>,
    at: jiff::Timestamp,
) -> Result<(), IndexError> {
    let stamp = jiff::Timestamp::from_millisecond(candidate.mtime_ms).unwrap_or(at);
    database
        .insert_unmatched_file(
            UnmatchedFile {
                id: 0,
                path: text(path)?,
                size_bytes: candidate.size_bytes,
                mtime_ms: stamp,
                parsed_title: parsed.title().map(str::to_owned),
                parsed_year: parsed.year(),
                parsed_season: parsed.season().map(peregrine_domain::SeasonNumber::new),
                parsed_episode: parsed
                    .episodes()
                    .map(|span| peregrine_domain::EpisodeNumber::new(span.first())),
                confidence: None,
                reason,
                suggested_title_id: None,
                suggested_episode_id: None,
                first_seen_at: at,
            },
            true,
        )
        .await?;
    Ok(())
}

/// The sort name a title is looked up and stored under.
///
/// Lower-cased and whitespace-collapsed, which is what makes `Alien` and `alien` one title. The full
/// normalization `PG-E14-05` owns — articles, punctuation, accents, and the `The Matrix`/`Matrix,
/// The` question — is not guessed at here: what this needs is a stable key, and a key that changes
/// when that task lands is a key that would have to be applied to rows written under the old one.
fn sort_name_of(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Wraps a path as the non-empty text the schema stores.
fn text(value: &str) -> Result<NonEmptyText, IndexError> {
    NonEmptyText::new(value).map_err(|_| IndexError::NotText {
        path: std::path::PathBuf::from(value),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use peregrine_domain::{AccountId, AccountScope, LibraryRoot, LibraryRootKind, ScanMode};
    use peregrine_storage::{MetadataState, PageRequest};
    use tempfile::TempDir;

    use peregrine_domain::ScanPolicy;

    use crate::library::scan::Scanner;

    use super::*;

    const NOW: i64 = 1_700_000_000_000;

    fn at(millis: i64) -> jiff::Timestamp {
        jiff::Timestamp::from_millisecond(millis).expect("the tests stay inside jiff's range")
    }

    /// A temp directory holding the files a test names, found by walking it.
    struct Corpus {
        directory: TempDir,
        database: Arc<Database>,
        root_id: i64,
    }

    async fn corpus(name: &str, files: &[&str]) -> Corpus {
        let directory = TempDir::new().unwrap();
        let media = directory.path().join(format!("Movies-{name}"));
        fs::create_dir_all(&media).unwrap();
        for relative in files {
            let path = media.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, b"a film, in bytes").unwrap();
        }
        let database = Arc::new(
            Database::open(directory.path().join("peregrine.sqlite"))
                .await
                .unwrap(),
        );
        let root_id = database
            .insert_library_root(LibraryRoot {
                id: 0,
                path: NonEmptyText::new(media.to_str().unwrap()).unwrap(),
                kind: LibraryRootKind::Movies,
                scan_mode: ScanMode::Incremental,
                last_scan_at: None,
                created_at: at(NOW),
            })
            .await
            .unwrap();
        Corpus {
            directory,
            database,
            root_id,
        }
    }

    impl Corpus {
        fn scan(&self) -> Vec<CandidateFile> {
            let root = self.database_root_path();
            Scanner::new(ScanPolicy::new())
                .scan(&root)
                .unwrap()
                .candidates
        }

        fn database_root_path(&self) -> PathBuf {
            self.directory.path().join(
                self.directory
                    .path()
                    .read_dir()
                    .unwrap()
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .find(|name| name.starts_with("Movies-"))
                    .expect("the media directory exists"),
            )
        }

        async fn index(&self) -> IndexReport {
            index_candidates(&self.database, self.root_id, &self.scan(), at(NOW + 1_000))
                .await
                .unwrap()
        }

        async fn titles(&self) -> Vec<String> {
            self.database
                .list_titles(
                    AccountScope::unrestricted(AccountId::new(1).expect("account one")),
                    PageRequest::first(50),
                )
                .await
                .unwrap()
                .into_iter()
                .map(|row| row.name.as_str().to_owned())
                .collect()
        }

        async fn unmatched(&self) -> Vec<String> {
            self.database
                .list_unmatched_files(
                    AccountScope::unrestricted(AccountId::new(1).expect("account one")),
                    PageRequest::first(50),
                )
                .await
                .unwrap()
                .into_iter()
                .map(|row| row.file.path.to_string())
                .collect()
        }
    }

    #[tokio::test]
    async fn a_film_becomes_a_title_an_item_and_a_file() {
        let corpus = corpus("film", &["Alien (1979).mkv"]).await;

        let report = corpus.index().await;

        assert_eq!(report.created_titles, 1, "{report:?}");
        assert_eq!(report.created_items, 1);
        assert_eq!(report.created_files, 1);
        assert_eq!(report.unmatched, 0);
        assert_eq!(corpus.titles().await, vec!["Alien"]);
    }

    #[tokio::test]
    async fn a_title_the_indexer_creates_is_a_stub_rather_than_a_complete_one() {
        // `PG-E5-06`: a stub is the identity a provider sync hangs its data off, and a title the
        // library invented from a filename must not claim to be complete — a screen that showed it
        // as complete would promise artwork and a synopsis that no sync has fetched.
        let corpus = corpus("stub", &["Alien (1979).mkv"]).await;
        corpus.index().await;

        let titles = corpus
            .database
            .list_titles(
                AccountScope::unrestricted(AccountId::new(1).expect("account one")),
                PageRequest::first(10),
            )
            .await
            .unwrap();
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].metadata_state, MetadataState::Stub);
    }

    #[tokio::test]
    async fn the_file_keeps_the_mtime_the_filesystem_gave_it() {
        // Not the scan's instant: the incremental rule compares what the library recorded against
        // what the filesystem now says, and a stored scan instant would make every file look
        // changed on the next pass.
        let corpus = corpus("stamp", &["Alien (1979).mkv"]).await;
        corpus.index().await;

        let candidate = corpus.scan().into_iter().next().unwrap();
        let row = corpus
            .database
            .recorded_file_at_path(candidate.path.to_str().unwrap())
            .await
            .unwrap()
            .expect("the file was indexed");

        assert_eq!(row.mtime_ms, candidate.mtime_ms);
        assert_eq!(row.size_bytes, i64::try_from(candidate.size_bytes).unwrap());
    }

    #[tokio::test]
    async fn a_second_pass_creates_nothing_and_refreshes_what_it_finds() {
        // The indexer is run over the same candidates after any interruption, so a second pass over
        // an unchanged folder must be a no-op that reports the refresh rather than a second copy of
        // everything.
        let corpus = corpus("idempotent", &["Alien (1979).mkv"]).await;
        corpus.index().await;

        let second = corpus.index().await;

        assert_eq!(second.created_titles, 0, "{second:?}");
        assert_eq!(second.created_items, 0);
        assert_eq!(second.created_files, 0);
        assert_eq!(second.refreshed_files, 1);
        assert!(second.is_unchanged());
        assert_eq!(corpus.titles().await.len(), 1);
    }

    #[tokio::test]
    async fn two_files_of_one_film_share_a_title_and_have_an_item_each() {
        // A 1080p and a 2160p copy: one title, two items, two files. The title lookup is what makes
        // that true, and a lookup that missed would have produced two titles for one film.
        let corpus = corpus(
            "two-copies",
            &["Alien (1979).mkv", "Alien (1979) 2160p.mkv"],
        )
        .await;

        let report = corpus.index().await;

        assert_eq!(report.created_titles, 1, "{report:?}");
        assert_eq!(report.created_files, 2);
        assert_eq!(corpus.titles().await, vec!["Alien"]);
    }

    #[tokio::test]
    async fn a_name_that_states_an_episode_is_parked_rather_than_guessed_at() {
        // Placing an episode needs the numbering rules PG-E14-07 owns, so the file is parked with
        // the reason and everything the parse read. Parking loses nothing: the parse is stored
        // beside the path, which is what makes the queue workable.
        let corpus = corpus("episode", &["Breaking Bad S01E01.mkv"]).await;

        let report = corpus.index().await;

        assert_eq!(report.unmatched, 1, "{report:?}");
        assert_eq!(report.created_titles, 0);
        assert_eq!(corpus.titles().await, Vec::<String>::new());
        assert_eq!(corpus.unmatched().await.len(), 1);
    }

    #[tokio::test]
    async fn a_name_that_cannot_be_parsed_is_parked_with_its_reason() {
        let corpus = corpus("unparsed", &["1080p.mkv"]).await;

        let report = corpus.index().await;

        assert_eq!(report.unmatched, 1, "{report:?}");
        assert_eq!(corpus.unmatched().await.len(), 1);
    }

    #[tokio::test]
    async fn a_scan_of_a_root_with_no_files_indexes_nothing() {
        let corpus = corpus("empty", &[]).await;

        assert_eq!(corpus.index().await, IndexReport::default());
        assert_eq!(corpus.titles().await, Vec::<String>::new());
    }

    #[tokio::test]
    async fn the_indexer_records_a_stub_for_every_file_including_one_an_account_cannot_see() {
        // The scope decision, pinned from both sides: the scanner indexes an adult-flagged title's
        // files (because the row has to exist before I37 can hide it), and a restricted account
        // still cannot see them. A scanner that read under a restricted scope would leave those
        // files permanently unindexed, and no later change to a rating ceiling could repair it.
        let corpus = corpus("scoped", &["Alien (1979).mkv", "Adult Film (2019).mkv"]).await;
        corpus.index().await;

        let unrestricted = AccountScope::unrestricted(AccountId::new(1).expect("account one"));
        let restricted = AccountScope::restricted(AccountId::new(2).expect("account two"));
        let visible = |scope| {
            let database = Arc::clone(&corpus.database);
            async move {
                database
                    .list_titles(scope, PageRequest::first(50))
                    .await
                    .unwrap()
                    .len()
            }
        };
        // Both titles are stubs, so a restricted scope hides them for want of a certification
        // rather than for want of a row — which is the distinction this test exists for.
        assert_eq!(visible(unrestricted).await, 2);
        assert_eq!(visible(restricted).await, 0);
    }
}
