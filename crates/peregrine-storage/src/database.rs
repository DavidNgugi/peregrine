//! The database handle: one writer task, a reader pool, and the open path.

use std::path::Path;

use sqlx::ConnectOptions;
use sqlx::Connection as _;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection, SqlitePool, SqlitePoolOptions};
use tracing::{info, warn};

use peregrine_domain::{
    AccountScope, LibraryRoot, MediaFile, MediaItem, SubtitleTrack, UnmatchedFile, WatchProgress,
    WatchlistEntry,
};

use crate::catalogue::{StubTitle, TitleExternalId, TitleIdentity};
use crate::error::StorageError;
use crate::integrity::Recovery;
use crate::jobs::{JobChange, JobRecord, NewJob};
use crate::page::PageRequest;
use crate::pragmas;
use crate::retention::{RetentionOutcome, RetentionPlan};
use crate::usage::{DiskUsage, LibraryUsageRow};
use crate::writer::{NewEvent, WriteRequest, WriteResponse, WriterHandle};

/// ARCHITECTURE §10: readers use a pool sized to `min(4, cores)`.
const READER_POOL_MAX: u32 = 4;

/// A handle to the one `SQLite` database.
///
/// Writes are applied by a task that owns the only write connection; reads are served
/// by a small pool, because WAL lets readers run while a writer holds the write lock.
#[derive(Debug)]
pub struct Database {
    writer: WriterHandle,
    readers: SqlitePool,
    recovery: Recovery,
}

impl Database {
    /// Opens the database at `path`, creating it when it does not exist, checks the file
    /// for damage, applies the embedded migrations on the write connection, and starts the
    /// writer task.
    ///
    /// The order is the recovery path of ARCHITECTURE §15: a damaged file is moved aside and
    /// replaced with a clean schema before the writer opens it, and an upgrade copies an
    /// existing database before it applies the first pending migration.
    ///
    /// # Errors
    ///
    /// [`StorageError::Pragma`] means the write connection could not be brought to the
    /// documented PRAGMA set, so the database was left untouched and the caller should
    /// report the pragma rather than retry blindly. [`StorageError::MigrationFailed`] and
    /// [`StorageError::Backup`] mean the upgrade did not run; [`StorageError::Quarantine`]
    /// means a damaged file is still in the way and nothing was deleted.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let path = path.as_ref();
        // The path is the first thing a support question needs and is not a secret; this
        // crate never logs a setting or a credential.
        info!(path = %path.display(), "opening the database");

        let recovery = recover(path).await?;
        let mut connection = open_writer_connection(path).await?;
        crate::migrations::apply(&mut connection, path).await?;

        let writer = WriterHandle::spawn(connection);
        let readers = open_reader_pool(path)
            .await
            .map_err(|error| StorageError::open(path, error))?;

        Ok(Self {
            writer,
            readers,
            recovery,
        })
    }

    /// What the boot-time integrity check found.
    ///
    /// [`Recovery::Rebuilt`] means the rows that were in the file are not in the database
    /// any more, so the library has to be rebuilt by scanning (ARCHITECTURE §15).
    #[must_use]
    pub fn recovery(&self) -> &Recovery {
        &self.recovery
    }

    /// The single-writer handle, for a repository that lives in its own module.
    ///
    /// A repository needs the writer and a reader, not the handle's whole surface; handing
    /// it the two ports is what keeps the writer the only write path (`ARCHITECTURE` §5).
    pub(crate) const fn writer(&self) -> &WriterHandle {
        &self.writer
    }

    /// The reader pool, for a repository that lives in its own module.
    pub(crate) const fn readers(&self) -> &SqlitePool {
        &self.readers
    }

    /// Appends one row to `event_log` through the single writer.
    ///
    /// Returns the rowid `SQLite` assigned: only the writer task ever holds the write
    /// connection, so it is the only place that can observe it.
    pub async fn append_event(&self, event: NewEvent) -> Result<i64, StorageError> {
        match self.writer.send(WriteRequest::AppendEvent(event)).await? {
            WriteResponse::EventAppended { id } => Ok(id),
            other => Err(unexpected_response("append_event", &other)),
        }
    }

    /// Adds one job to the journal through the single writer.
    ///
    /// The row starts in `queued`, which is the phase the runtime reports before the
    /// scheduler has an attempt to run.
    pub async fn insert_job(&self, job: NewJob) -> Result<i64, StorageError> {
        match self
            .writer
            .send(WriteRequest::InsertJob(Box::new(job)))
            .await?
        {
            WriteResponse::JobInserted { id } => Ok(id),
            other => Err(unexpected_response("insert_job", &other)),
        }
    }

    /// Applies one change to a journal row, reporting whether the row was still there.
    ///
    /// `false` is not an error: a job that finished while a late progress report was in
    /// flight is the ordinary case, and the guards in `jobs::update` are what stop that
    /// report from resurrecting a terminal row.
    pub async fn update_job(&self, id: i64, change: JobChange) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::UpdateJob {
                id,
                change: Box::new(change),
            })
            .await?
        {
            WriteResponse::JobUpdated { changed } => Ok(changed),
            other => Err(unexpected_response("update_job", &other)),
        }
    }

    /// One journal row, or `None` when no such row exists.
    pub async fn job(&self, id: i64) -> Result<Option<JobRecord>, StorageError> {
        crate::jobs::get(&self.readers, id).await
    }

    /// A page of journal rows, newest first.
    pub async fn list_jobs(&self, request: PageRequest) -> Result<Vec<JobRecord>, StorageError> {
        crate::jobs::list(&self.readers, request).await
    }

    /// Inserts one title in its `stub` state and reports its row id.
    ///
    /// A stub is the identity a provider sync hangs its data off: the row exists, the name
    /// is known, and every field a provider fills is still NULL until that sync runs
    /// (PG-E5-06).
    pub async fn insert_stub_title(&self, stub: StubTitle) -> Result<i64, StorageError> {
        match self
            .writer
            .send(WriteRequest::InsertStubTitle(Box::new(stub)))
            .await?
        {
            WriteResponse::TitleInserted { id } => Ok(id),
            other => Err(unexpected_response("insert_stub_title", &other)),
        }
    }

    /// Attaches external ids to a title, refusing any that another title owns.
    ///
    /// # Errors
    ///
    /// [`StorageError::Conflict`] when an id in `ids` is already attached elsewhere; nothing
    /// in the call is applied in that case.
    pub async fn attach_external_ids(
        &self,
        title_id: i64,
        ids: &[TitleExternalId],
    ) -> Result<(), StorageError> {
        match self
            .writer
            .send(WriteRequest::AttachExternalIds {
                title_id,
                ids: ids.to_vec(),
            })
            .await?
        {
            WriteResponse::ExternalIdsAttached => Ok(()),
            other => Err(unexpected_response("attach_external_ids", &other)),
        }
    }

    /// One title's identity, when `scope` may see it.
    ///
    /// A title outside the scope reads as `None` rather than as a refusal: an account must
    /// not be able to tell a title it may not see from one that does not exist (AGENTS R34).
    pub async fn title_identity(
        &self,
        scope: AccountScope,
        id: i64,
    ) -> Result<Option<TitleIdentity>, StorageError> {
        crate::catalogue::identity(&self.readers, scope, id).await
    }

    /// The newest page of titles `scope` may see.
    pub async fn list_titles(
        &self,
        scope: AccountScope,
        request: PageRequest,
    ) -> Result<Vec<TitleIdentity>, StorageError> {
        crate::catalogue::list_titles(&self.readers, scope, request).await
    }

    /// The title an external id belongs to, or `None` when no title has it.
    pub async fn title_for_external_id(
        &self,
        id: &peregrine_domain::ExternalId,
    ) -> Result<Option<i64>, StorageError> {
        crate::catalogue::title_for(&self.readers, id).await
    }

    /// Every external id attached to a title, in provider order.
    pub async fn external_ids_of(
        &self,
        title_id: i64,
    ) -> Result<Vec<TitleExternalId>, StorageError> {
        crate::catalogue::external_ids(&self.readers, title_id).await
    }

    /// How many titles the catalogue holds.
    pub async fn title_count(&self) -> Result<i64, StorageError> {
        crate::catalogue::title_count(&self.readers).await
    }

    /// One page of titles `scope` may see that match `query`, the highest id first.
    ///
    /// `query` is the user's own text, and this is the only place it reaches `FTS5`: it is
    /// turned into a `MATCH` expression of quoted terms, so a query the grammar would refuse
    /// is answered with an empty page instead of an error (AGENTS R34 and §4's hostile row).
    /// An empty or whitespace-only query is not a search and reads nothing.
    pub async fn search(
        &self,
        scope: peregrine_domain::AccountScope,
        query: &str,
        request: PageRequest,
    ) -> Result<Vec<TitleIdentity>, StorageError> {
        crate::fts::search(&self.readers, scope, query, request).await
    }

    /// Rewrites one title's search index row from the source tables.
    ///
    /// Returns `false` when no title holds `title_id`, in which case nothing was indexed.
    /// The caller is the provider sync that changed a title's name or its cast; nothing in
    /// the repository layer updates a `title` row yet (PG-E5-06), so this is exercised by
    /// the tests and is the call that path will make.
    pub async fn reindex_title(&self, title_id: i64) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::ReindexTitle { title_id })
            .await?
        {
            WriteResponse::TitleReindexed { indexed } => Ok(indexed),
            other => Err(unexpected_response("reindex_title", &other)),
        }
    }

    /// Removes one title's search index row, reporting whether there was one.
    ///
    /// The title itself is not touched: this is the index half of a removal, and the row is
    /// what is left when the catalogue still holds the title.
    pub async fn remove_title_index(&self, title_id: i64) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::RemoveTitleIndex { title_id })
            .await?
        {
            WriteResponse::TitleIndexRemoved { removed } => Ok(removed),
            other => Err(unexpected_response("remove_title_index", &other)),
        }
    }

    /// Rebuilds the whole search index from the source tables, reporting how many titles it
    /// indexed.
    ///
    /// It runs through the single writer, so it cannot interleave with a title being indexed,
    /// and it is the recovery path `DATA_MODEL` §6 names after a dataset reimport or a change
    /// to what the index reads.
    pub async fn rebuild_search_index(&self) -> Result<u64, StorageError> {
        match self.writer.send(WriteRequest::RebuildSearchIndex).await? {
            WriteResponse::SearchIndexRebuilt { indexed } => Ok(indexed),
            other => Err(unexpected_response("rebuild_search_index", &other)),
        }
    }

    /// What a retention run at `at_millis` would remove, before anything is removed.
    ///
    /// The instant arrives as a value (AGENTS R14) and the answer is a plan the caller can log
    /// or show (AGENTS R22); nothing is deleted until [`Database::apply_retention`] is handed
    /// that plan.
    pub async fn plan_retention(&self, at_millis: i64) -> Result<RetentionPlan, StorageError> {
        crate::retention::plan(&self.readers, at_millis).await
    }

    /// Applies a retention plan through the single writer.
    ///
    /// Best-effort per table, like the pre-migration copy rotation: a table whose statement
    /// fails is reported in the outcome and the remaining tables still run. The one error
    /// returned is a stopped writer, when nothing can be applied at all.
    pub async fn apply_retention(
        &self,
        plan: &RetentionPlan,
    ) -> Result<RetentionOutcome, StorageError> {
        crate::retention::apply(&self.writer, plan).await
    }

    /// The bytes the database can account for, per `DATA_MODEL` §7 tier.
    ///
    /// Every number is the database's own, and each entry names the side of the
    /// database/filesystem line it came from ([`crate::MeasuredFrom`]); the filesystem's answer
    /// is a directory walk this crate does not do.
    pub async fn disk_usage(&self) -> Result<DiskUsage, StorageError> {
        crate::usage::disk_usage(&self.readers).await
    }

    /// The library's recorded bytes by root and quality label, for the `scope` that may see them.
    ///
    /// This is the §4 query the storage screen and the free-space floor guard read.
    pub async fn library_storage_usage(
        &self,
        scope: AccountScope,
    ) -> Result<Vec<LibraryUsageRow>, StorageError> {
        crate::usage::library_storage_usage(&self.readers, scope).await
    }

    /// Inserts one library root and reports the id `SQLite` assigned.
    pub async fn insert_library_root(&self, root: LibraryRoot) -> Result<i64, StorageError> {
        match self
            .writer
            .send(WriteRequest::InsertLibraryRoot(Box::new(root)))
            .await?
        {
            WriteResponse::RowInserted { id } => Ok(id),
            other => Err(unexpected_response("insert_library_root", &other)),
        }
    }

    /// One library root, or `None` when no such row exists.
    ///
    /// No [`AccountScope`] applies and none is asked for: a root is a directory the user
    /// configured, not content.
    pub async fn library_root(&self, id: i64) -> Result<Option<LibraryRoot>, StorageError> {
        crate::library::root(&self.readers, id).await
    }

    /// A page of library roots, newest first.
    pub async fn list_library_roots(
        &self,
        request: PageRequest,
    ) -> Result<Vec<crate::library::LibraryRootRow>, StorageError> {
        crate::library::list_roots(&self.readers, request).await
    }

    /// Records that a root was scanned at `at`, reporting whether the row was there.
    pub async fn record_root_scan(&self, id: i64, at: i64) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::RecordRootScan { id, at })
            .await?
        {
            WriteResponse::RowChanged { changed } => Ok(changed),
            other => Err(unexpected_response("record_root_scan", &other)),
        }
    }

    /// Inserts one media item and reports the id `SQLite` assigned.
    ///
    /// The two partial unique indices refuse a second film item for one title or a second item
    /// for one episode (I2); that arrives as a statement failure rather than a silent second
    /// row.
    pub async fn insert_media_item(&self, item: MediaItem) -> Result<i64, StorageError> {
        match self
            .writer
            .send(WriteRequest::InsertMediaItem(Box::new(item)))
            .await?
        {
            WriteResponse::RowInserted { id } => Ok(id),
            other => Err(unexpected_response("insert_media_item", &other)),
        }
    }

    /// One media item, when `scope` may see the title it is for.
    pub async fn media_item(
        &self,
        scope: AccountScope,
        id: i64,
    ) -> Result<Option<MediaItem>, StorageError> {
        crate::library::item(&self.readers, scope, id).await
    }

    /// The item for one title or one episode, when `scope` may see it.
    pub async fn media_item_for(
        &self,
        scope: AccountScope,
        title_id: i64,
        episode_id: Option<i64>,
    ) -> Result<Option<MediaItem>, StorageError> {
        crate::library::item_for(&self.readers, scope, title_id, episode_id).await
    }

    /// A page of one title's items, newest first, when `scope` may see the title.
    pub async fn list_media_items(
        &self,
        scope: AccountScope,
        title_id: i64,
        request: PageRequest,
    ) -> Result<Vec<crate::library::MediaItemRow>, StorageError> {
        crate::library::list_items_for_title(&self.readers, scope, title_id, request).await
    }

    /// Inserts one media file and reports the id `SQLite` assigned.
    ///
    /// `media_file.path` is `UNIQUE`, so two items can never claim one file (I3): the second
    /// insert fails rather than producing a second row for the same bytes.
    pub async fn insert_media_file(&self, file: MediaFile) -> Result<i64, StorageError> {
        match self
            .writer
            .send(WriteRequest::InsertMediaFile(Box::new(file)))
            .await?
        {
            WriteResponse::RowInserted { id } => Ok(id),
            other => Err(unexpected_response("insert_media_file", &other)),
        }
    }

    /// One file, when `scope` may see the title its item is for.
    pub async fn media_file(
        &self,
        scope: AccountScope,
        id: i64,
    ) -> Result<Option<MediaFile>, StorageError> {
        crate::library::file(&self.readers, scope, id).await
    }

    /// The file a path names, when `scope` may see it.
    pub async fn media_file_by_path(
        &self,
        scope: AccountScope,
        path: &str,
    ) -> Result<Option<MediaFile>, StorageError> {
        crate::library::file_by_path(&self.readers, scope, path).await
    }

    /// A page of one item's files, newest first, when `scope` may see the item.
    pub async fn list_media_files(
        &self,
        scope: AccountScope,
        media_item_id: i64,
        request: PageRequest,
    ) -> Result<Vec<crate::library::MediaFileRow>, StorageError> {
        crate::library::list_files_for_item(&self.readers, scope, media_item_id, request).await
    }

    /// Marks one file absent at scan time, or present again.
    ///
    /// The instant is the caller's — [`MediaFile::mark_missing`] keeps the *first* one, so a
    /// file that stays gone keeps saying when it was first noticed (I11) — and `false` means
    /// the row was not there, which is not an error.
    pub async fn set_media_file_missing(
        &self,
        id: i64,
        missing_since: Option<i64>,
        updated_at: i64,
    ) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::SetFileMissing {
                id,
                missing_since,
                updated_at,
            })
            .await?
        {
            WriteResponse::RowChanged { changed } => Ok(changed),
            other => Err(unexpected_response("set_media_file_missing", &other)),
        }
    }

    /// Inserts one unmatched file and reports its id, or `None` when the path was already
    /// there.
    ///
    /// `ignore_conflict` is the scan's answer to seeing the same unplaceable file twice: with
    /// it set the second sighting leaves the first row — and its `first_seen_at` — alone.
    pub async fn insert_unmatched_file(
        &self,
        file: UnmatchedFile,
        ignore_conflict: bool,
    ) -> Result<Option<i64>, StorageError> {
        match self
            .writer
            .send(WriteRequest::InsertUnmatchedFile {
                file: Box::new(file),
                ignore_conflict,
            })
            .await?
        {
            WriteResponse::RowInserted { id } => Ok(Some(id)),
            WriteResponse::RowIgnored => Ok(None),
            other => Err(unexpected_response("insert_unmatched_file", &other)),
        }
    }

    /// One unmatched file, when `scope` may be offered its suggestion.
    pub async fn unmatched_file(
        &self,
        scope: AccountScope,
        id: i64,
    ) -> Result<Option<UnmatchedFile>, StorageError> {
        crate::library::unmatched(&self.readers, scope, id).await
    }

    /// A page of unmatched files, newest first.
    pub async fn list_unmatched_files(
        &self,
        scope: AccountScope,
        request: PageRequest,
    ) -> Result<Vec<crate::library::UnmatchedFileRow>, StorageError> {
        crate::library::list_unmatched(&self.readers, scope, request).await
    }

    /// How many rows `unmatched_file` holds.
    pub async fn unmatched_count(&self) -> Result<i64, StorageError> {
        crate::library::unmatched_count(&self.readers).await
    }

    /// Inserts one subtitle track and reports the id `SQLite` assigned.
    pub async fn insert_subtitle_track(&self, track: SubtitleTrack) -> Result<i64, StorageError> {
        match self
            .writer
            .send(WriteRequest::InsertSubtitleTrack(Box::new(track)))
            .await?
        {
            WriteResponse::RowInserted { id } => Ok(id),
            other => Err(unexpected_response("insert_subtitle_track", &other)),
        }
    }

    /// One subtitle track, when `scope` may see the media it belongs to.
    pub async fn subtitle_track(
        &self,
        scope: AccountScope,
        id: i64,
    ) -> Result<Option<SubtitleTrack>, StorageError> {
        crate::library::subtitle(&self.readers, scope, id).await
    }

    /// A page of one file's subtitle tracks, newest first.
    pub async fn list_subtitle_tracks(
        &self,
        scope: AccountScope,
        media_file_id: i64,
        request: PageRequest,
    ) -> Result<Vec<crate::library::SubtitleTrackRow>, StorageError> {
        crate::library::list_subtitles_for_file(&self.readers, scope, media_file_id, request).await
    }

    /// Writes one account's progress for one title or episode.
    ///
    /// The value is keyed on `(account_id, entity_key)`, so a player reporting a position twice
    /// updates one row. The percentage is the domain's derivation, never this layer's (I6).
    pub async fn record_watch_progress(&self, progress: WatchProgress) -> Result<(), StorageError> {
        match self
            .writer
            .send(WriteRequest::UpsertWatchProgress(Box::new(progress)))
            .await?
        {
            WriteResponse::Applied => Ok(()),
            other => Err(unexpected_response("record_watch_progress", &other)),
        }
    }

    /// One account's progress for one title or episode, when `scope` may see the title.
    pub async fn watch_progress(
        &self,
        scope: AccountScope,
        title_id: i64,
        episode_id: Option<i64>,
    ) -> Result<Option<WatchProgress>, StorageError> {
        crate::watch::progress_for(&self.readers, scope, title_id, episode_id).await
    }

    /// A page of what one account is part-way through, most recently updated first.
    pub async fn list_continue_watching(
        &self,
        scope: AccountScope,
        request: PageRequest,
    ) -> Result<Vec<crate::watch::WatchProgressRow>, StorageError> {
        crate::watch::list_continue(&self.readers, scope, request).await
    }

    /// A page of everything one account has watched, most recently updated first.
    pub async fn list_watch_progress(
        &self,
        scope: AccountScope,
        request: PageRequest,
    ) -> Result<Vec<crate::watch::WatchProgressRow>, StorageError> {
        crate::watch::list_progress(&self.readers, scope, request).await
    }

    /// Writes one watchlist entry.
    pub async fn add_to_watchlist(&self, entry: WatchlistEntry) -> Result<(), StorageError> {
        match self
            .writer
            .send(WriteRequest::UpsertWatchlist(Box::new(entry)))
            .await?
        {
            WriteResponse::Applied => Ok(()),
            other => Err(unexpected_response("add_to_watchlist", &other)),
        }
    }

    /// Removes one watchlist entry, reporting whether it was there.
    pub async fn remove_from_watchlist(
        &self,
        account_id: peregrine_domain::AccountId,
        title_id: i64,
    ) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::RemoveWatchlist {
                account_id,
                title_id,
            })
            .await?
        {
            WriteResponse::RowChanged { changed } => Ok(changed),
            other => Err(unexpected_response("remove_from_watchlist", &other)),
        }
    }

    /// One watchlist entry, when `scope` may see the title.
    pub async fn watchlist_entry(
        &self,
        scope: AccountScope,
        title_id: i64,
    ) -> Result<Option<WatchlistEntry>, StorageError> {
        crate::watch::watchlist_entry(&self.readers, scope, title_id).await
    }

    /// A page of one account's watchlist, newest first.
    pub async fn list_watchlist(
        &self,
        scope: AccountScope,
        request: PageRequest,
    ) -> Result<Vec<crate::watch::WatchlistRow>, StorageError> {
        crate::watch::list_watchlist(&self.readers, scope, request).await
    }

    /// How many rows `event_log` holds, read through the pool.
    ///
    /// This is the read path in miniature — a typed query on a pooled connection — and
    /// it is what makes "no write was lost" checkable from outside the writer.
    pub async fn event_count(&self) -> Result<u64, StorageError> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM event_log")
            .fetch_one(&self.readers)
            .await
            .map_err(StorageError::from_sqlx)?;
        // `COUNT(*)` cannot be negative; the conversion exists so a count cannot be
        // mistaken for the rowid `append_event` returns.
        Ok(u64::try_from(count).unwrap_or_default())
    }
}

/// The writer answered a request it was not sent.
///
/// Unreachable by construction — the handle sends one request and reads its own answer —
/// so a mismatch is an internal invariant violation rather than a database condition.
fn unexpected_response(operation: &'static str, response: &WriteResponse) -> StorageError {
    StorageError::UnexpectedValue {
        column: operation,
        found: format!("{response:?}"),
    }
}

/// Checks an existing file for damage and rebuilds the database when it is damaged.
///
/// A file that is not there yet, or is empty, has nothing to check: that is a first install.
/// The check runs on its own connection, which is closed before anything is renamed, because
/// a filesystem that refuses to move an open file would turn detection into a failure.
async fn recover(path: &Path) -> Result<Recovery, StorageError> {
    match tokio::fs::metadata(path).await {
        Ok(metadata) if metadata.len() > 0 => {}
        Ok(_) => return Ok(Recovery::Intact),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Recovery::Intact);
        }
        Err(error) => {
            return Err(StorageError::Open {
                path: path.to_path_buf(),
                source: Box::new(error),
            });
        }
    }

    let mut connection = crate::integrity::probe(path).await?;
    let damage = crate::integrity::check(&mut connection).await?;
    connection
        .close()
        .await
        .map_err(|error| StorageError::open(path, error))?;

    let Some(reason) = damage else {
        return Ok(Recovery::Intact);
    };
    warn!(
        path = %path.display(),
        reason = %reason,
        "the database failed its integrity check"
    );

    let quarantine = crate::integrity::quarantine(path).await?;
    warn!(
        path = %path.display(),
        quarantine = %quarantine.display(),
        "a clean schema replaces the damaged database, so the library must be rescanned"
    );
    Ok(Recovery::Rebuilt { quarantine })
}

/// Opens the connection the writer task will own, with the documented pragmas applied
/// and then verified, because an existing database cannot change every one of them.
///
/// A required pragma that the database will not report fails the open with
/// [`StorageError::Pragma`]; this is the connection every write runs on, so it is the one
/// whose pragmas actually decide whether the schema's constraints are enforced.
pub(crate) async fn open_writer_connection(path: &Path) -> Result<SqliteConnection, StorageError> {
    let mut connection = connection_options(path)
        .connect()
        .await
        .map_err(|error| StorageError::open(path, error))?;
    pragmas::enforce(&mut connection, path).await?;
    Ok(connection)
}

/// Opens the reader pool with the documented pragma set on every connection.
///
/// The options cover connections the pool opens later, and the `after_connect` hook
/// verifies them, so a connection that cannot report a required pragma is discarded by
/// the pool instead of serving reads under different pragmas. `sqlx` retries a connection
/// its `after_connect` hook rejects, so that path reaches the caller as `PoolTimedOut`
/// rather than as [`StorageError::Pragma`]; the writer connection checked in `open` is the
/// one that reports the typed failure.
pub(crate) async fn open_reader_pool(path: &Path) -> Result<SqlitePool, sqlx::Error> {
    let pragma_path = path.to_path_buf();
    SqlitePoolOptions::new()
        .max_connections(reader_pool_size())
        .after_connect(move |connection, _metadata| {
            // `after_connect` can only return a driver error, so the typed pragma failure
            // travels inside one.
            let pragma_path = pragma_path.clone();
            Box::pin(async move {
                pragmas::enforce(connection, &pragma_path)
                    .await
                    .map_err(|error| sqlx::Error::Configuration(Box::new(error)))
            })
        })
        .connect_with(connection_options(path))
        .await
}

/// `min(4, cores)`, per ARCHITECTURE §10.
fn reader_pool_size() -> u32 {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    u32::try_from(cores)
        .unwrap_or(READER_POOL_MAX)
        .min(READER_POOL_MAX)
}

fn connection_options(path: &Path) -> SqliteConnectOptions {
    pragmas::apply_to(
        SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use tempfile::TempDir;

    const WRITERS: i64 = 20;
    const WRITES_PER_WRITER: i64 = 5;

    async fn database(directory: &TempDir) -> Database {
        Database::open(directory.path().join("peregrine.sqlite"))
            .await
            .unwrap()
    }

    fn event(writer: i64, sequence: i64) -> NewEvent {
        NewEvent {
            // Injected, never the wall clock (AGENTS R14).
            at_millis: 1_700_000_000_000 + writer * WRITES_PER_WRITER + sequence,
            // A declared event, because the writer accepts nothing else (PG-E4-16); the writer
            // index stands in for a job id so the rows are distinguishable.
            event: peregrine_domain::Event::DownloadQueued { job_id: writer },
            detail_json: None,
        }
    }

    /// PG-E3-02: twenty callers writing at once must serialize behind the single writer
    /// and every write must land.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn twenty_concurrent_writers_serialize_without_busy_or_lost_writes() {
        let directory = TempDir::new().unwrap();
        let database = Arc::new(database(&directory).await);

        let mut writers = Vec::new();
        for writer in 0..WRITERS {
            let database = Arc::clone(&database);
            writers.push(tokio::spawn(async move {
                let mut outcomes = Vec::new();
                for sequence in 0..WRITES_PER_WRITER {
                    outcomes.push(database.append_event(event(writer, sequence)).await);
                }
                outcomes
            }));
        }

        let mut ids = BTreeSet::new();
        let mut busy = 0_usize;
        let mut failures = Vec::new();
        for writer in writers {
            for outcome in writer.await.unwrap() {
                match outcome {
                    Ok(id) => {
                        ids.insert(id);
                    }
                    Err(error @ StorageError::Busy { .. }) => {
                        busy += 1;
                        failures.push(error.to_string());
                    }
                    Err(error) => failures.push(error.to_string()),
                }
            }
        }

        assert_eq!(busy, 0, "no writer may see SQLITE_BUSY: {failures:#?}");
        assert!(
            failures.is_empty(),
            "twenty writers must serialize through the single writer: {failures:#?}"
        );

        let expected = usize::try_from(WRITERS * WRITES_PER_WRITER).unwrap();
        assert_eq!(
            ids.len(),
            expected,
            "a lost write would reuse or skip a rowid"
        );
        assert_eq!(
            *ids.first().unwrap(),
            1,
            "SQLite numbers a fresh table's rowids from one"
        );
        assert_eq!(*ids.last().unwrap(), i64::try_from(expected).unwrap());

        let stored = database.event_count().await.unwrap();
        assert_eq!(
            usize::try_from(stored).unwrap(),
            expected,
            "every acknowledged write must be readable from the log"
        );
    }

    #[tokio::test]
    async fn the_reader_pool_is_sized_by_cores_within_the_documented_ceiling() {
        let directory = TempDir::new().unwrap();
        let pool = open_reader_pool(&directory.path().join("peregrine.sqlite"))
            .await
            .unwrap();

        let max_connections = pool.options().get_max_connections();
        assert!((1..=READER_POOL_MAX).contains(&max_connections));
        assert_eq!(max_connections, reader_pool_size());
    }

    #[tokio::test]
    async fn reopening_an_existing_database_keeps_the_writer_working() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");

        let first = Database::open(&path).await.unwrap();
        let first_id = first.append_event(event(0, 0)).await.unwrap();
        let recovery = first.recovery().clone();
        drop(first);
        assert_eq!(recovery, Recovery::Intact);

        let second = Database::open(&path).await.unwrap();
        let second_id = second.append_event(event(0, 1)).await.unwrap();

        assert_eq!(first_id, 1);
        assert_eq!(second_id, 2, "the reopened writer continues the log");
    }

    /// The files a recovery path writes, by name: a copy taken before an upgrade and a
    /// quarantined file are the only two things a boot may add to the data directory.
    async fn recovery_artifacts(directory: &TempDir) -> Vec<String> {
        let mut names = Vec::new();
        let mut entries = tokio::fs::read_dir(directory.path()).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.contains(".backup-") || name.contains(".corrupt-") {
                names.push(name);
            }
        }
        names.sort();
        names
    }

    /// PG-E3-14's healthy path: nothing observable happens. No copy, no quarantine, and the
    /// rows that were there are still there.
    #[tokio::test]
    async fn a_healthy_database_boots_without_writing_anything() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");

        let first = Database::open(&path).await.unwrap();
        assert_eq!(*first.recovery(), Recovery::Intact);
        assert!(!first.recovery().requires_rescan());
        assert_eq!(first.append_event(event(0, 0)).await.unwrap(), 1);
        drop(first);

        let second = Database::open(&path).await.unwrap();

        assert_eq!(*second.recovery(), Recovery::Intact);
        assert_eq!(
            second.event_count().await.unwrap(),
            1,
            "the healthy path must not touch the rows"
        );
        assert!(
            recovery_artifacts(&directory).await.is_empty(),
            "a healthy boot leaves no copy and no quarantine: {:?}",
            recovery_artifacts(&directory).await
        );
    }

    /// PG-E3-14's acceptance: injected corruption is detected, the damaged file is kept
    /// rather than deleted, a clean schema replaces it, and the boot says the library has
    /// to be rescanned.
    #[tokio::test]
    async fn a_corrupt_database_is_quarantined_and_rebuilt() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");

        // A database that worked and held a row, closed cleanly so the fixture is not
        // relying on a destructor running before the file is overwritten.
        let mut connection = open_writer_connection(&path).await.unwrap();
        crate::migrations::apply(&mut connection, &path)
            .await
            .unwrap();
        sqlx::query("INSERT INTO event_log (at, kind) VALUES (0, 'test.before_corruption')")
            .execute(&mut connection)
            .await
            .unwrap();
        connection.close().await.unwrap();

        // Real corruption: the file's header is overwritten, so it is no longer a database.
        let mut bytes = tokio::fs::read(&path).await.unwrap();
        bytes[..100].fill(0xff);
        tokio::fs::write(&path, &bytes).await.unwrap();

        let database = Database::open(&path).await.unwrap();

        let Recovery::Rebuilt { quarantine } = database.recovery() else {
            panic!(
                "the damaged file must be rebuilt, got {:?}",
                database.recovery()
            );
        };
        assert!(
            database.recovery().requires_rescan(),
            "the rows are gone, so the library has to be scanned again"
        );
        assert_eq!(
            tokio::fs::read(quarantine).await.unwrap(),
            bytes,
            "the damaged file is kept byte for byte, not deleted"
        );
        assert_eq!(
            database.event_count().await.unwrap(),
            0,
            "the rebuilt schema is empty: the old rows are in the quarantined file"
        );
        let tables: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'title'",
        )
        .fetch_one(&database.readers)
        .await
        .unwrap();
        assert_eq!(tables, 1, "the clean schema is the embedded one");

        // The rebuilt database is a working one, and the next boot finds it healthy.
        assert_eq!(database.append_event(event(0, 0)).await.unwrap(), 1);
        drop(database);
        let next = Database::open(&path).await.unwrap();
        assert_eq!(*next.recovery(), Recovery::Intact);
    }

    /// A file that was never a database at all is treated the same way: the recovery path
    /// is about the file, not about how it became unreadable.
    #[tokio::test]
    async fn a_file_that_is_not_a_database_is_quarantined_too() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");
        tokio::fs::write(&path, b"this was never a database")
            .await
            .unwrap();

        let database = Database::open(&path).await.unwrap();

        let Recovery::Rebuilt { quarantine } = database.recovery() else {
            panic!("expected a rebuild, got {:?}", database.recovery());
        };
        assert_eq!(
            tokio::fs::read(&quarantine).await.unwrap(),
            b"this was never a database"
        );
        assert_eq!(database.title_count().await.unwrap(), 0);
    }

    /// The permissions edge case: damage that cannot be moved aside is reported, and the
    /// file is still there. Nothing is deleted to make the boot succeed.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_damaged_database_that_cannot_be_moved_aside_is_reported() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");
        tokio::fs::write(&path, b"not a database").await.unwrap();
        let original = tokio::fs::metadata(directory.path())
            .await
            .unwrap()
            .permissions();
        tokio::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o500))
            .await
            .unwrap();

        let error = Database::open(&path)
            .await
            .expect_err("a damaged file that cannot be moved must not be silently replaced");

        // Restore before asserting, or a failing assertion leaves the temporary directory
        // undeletable.
        tokio::fs::set_permissions(directory.path(), original)
            .await
            .unwrap();

        assert!(
            matches!(error, StorageError::Quarantine { .. }),
            "expected a typed quarantine failure, got {error:?}"
        );
        assert!(
            error.to_string().contains("close any other copy"),
            "the error must say what to do: {error}"
        );
        assert_eq!(
            tokio::fs::read(&path).await.unwrap(),
            b"not a database",
            "the damaged file must not be deleted when it cannot be moved"
        );
    }
}
