//! The single write connection.
//!
//! `SQLite` permits one writer at a time, and ARCHITECTURE §5 turns that constraint into
//! the design: one task owns the write connection and every mutation arrives as a
//! message on one channel. Callers therefore cannot contend for the lock, and a write
//! is serialized behind the writer task instead of failing with `SQLITE_BUSY`.

use peregrine_domain::{AccountScope, Event, ScanMode};
use sqlx::Connection as _;
use tokio::sync::{mpsc, oneshot};

use peregrine_domain::centre::{Delivery, NewCentreEntry, NotificationId, Recorded};

use crate::catalogue::{StubTitle, TitleExternalId};
use crate::error::StorageError;
use crate::jobs::{JobChange, NewJob};

/// How many requests may wait behind the one the writer is applying.
///
/// A single slot is enough because the writer is itself the serialization point: a
/// deeper queue shortens the wait for a caller but not the wait for the write, and it
/// hides backpressure from the caller that creates it.
const WRITE_QUEUE_DEPTH: usize = 1;

/// A mutation the single writer applies, in the order the requests arrive.
#[derive(Debug)]
pub(crate) enum WriteRequest {
    /// Append one row to `event_log`.
    AppendEvent(NewEvent),
    /// Insert one row into `job`.
    ///
    /// Boxed because the request travels through a channel sized at one slot, so the
    /// enum's size is the size of its largest variant, and a job row is much larger than
    /// the alternative.
    InsertJob(Box<NewJob>),
    /// Apply one change to a `job` row.
    UpdateJob {
        id: i64,
        change: Box<JobChange>,
    },
    /// Insert one title in its `stub` state.
    InsertStubTitle(Box<StubTitle>),
    /// Insert one `season` row.
    InsertStubSeason(Box<crate::catalogue::StubSeason>),
    /// Insert one `episode` row.
    InsertStubEpisode(Box<crate::catalogue::StubEpisode>),
    /// Insert the built-in default quality profile when the table is empty.
    ///
    /// The rules are built and serialized by the domain, so this never carries hand-written JSON.
    SetDefaultQualityResolutions {
        resolutions_json: String,
        at: i64,
    },
    InsertDefaultQualityProfile {
        /// The profile's name.
        name: String,
        /// `QualityRules` as JSON.
        rules_json: String,
        /// The rank upgrading stops at.
        cutoff_rank: i64,
        /// Whether a better release may replace a file.
        upgrade_allowed: bool,
        /// Unix milliseconds.
        at: i64,
    },
    /// Write what a provider said about one season, creating the row when the library has none.
    UpsertSeasonDetails {
        /// The title the season belongs to.
        title_id: i64,
        /// What the provider said, whose `season_number` is the conflict key.
        details: Box<peregrine_domain::catalogue::SeasonDetails>,
        /// The provider that answered, for the artwork columns.
        provider: String,
        /// Unix milliseconds.
        at: i64,
    },
    /// Write what a provider said about one episode, creating the row when the library has none.
    UpsertEpisodeDetails {
        /// The title the episode belongs to.
        title_id: i64,
        /// The season row it belongs to, which resolves the schema's foreign key.
        season_id: i64,
        /// What the provider said, whose numbers are the conflict key.
        details: Box<peregrine_domain::catalogue::EpisodeDetails>,
        /// The provider that answered, for the artwork columns.
        provider: String,
        /// Unix milliseconds.
        at: i64,
    },
    /// Write a provider's answer onto an existing title, and reindex it.
    UpdateTitleMetadata {
        /// The title row.
        id: i64,
        /// What the provider said.
        details: Box<peregrine_domain::catalogue::TitleDetails>,
        /// The name to sort by, already normalized by the caller.
        sort_name: String,
        /// The state the sync is putting the title into.
        state: crate::MetadataState,
        /// The provider that answered, for the artwork columns.
        provider: String,
        /// Unix milliseconds.
        at: i64,
    },
    /// Find or create the title one provider id names, and attach the id, in one transaction.
    UpsertProviderTitle(Box<crate::catalogue::ProviderTitle>),
    /// Attach external ids to one title.
    AttachExternalIds {
        title_id: i64,
        ids: Vec<TitleExternalId>,
    },
    /// Insert one row into `library_root`.
    InsertLibraryRoot(Box<peregrine_domain::LibraryRoot>),
    /// Insert one row into `media_item`.
    InsertMediaItem(Box<peregrine_domain::MediaItem>),
    /// Insert one row into `media_file`.
    InsertMediaFile(Box<peregrine_domain::MediaFile>),
    /// Insert one row into `unmatched_file`.
    InsertUnmatchedFile {
        file: Box<peregrine_domain::UnmatchedFile>,
        ignore_conflict: bool,
    },
    /// Insert one row into `subtitle_track`.
    InsertSubtitleTrack(Box<peregrine_domain::SubtitleTrack>),
    /// Record that a library root was scanned.
    RemoveLibraryRoot {
        id: i64,
    },
    SetRootScanMode {
        id: i64,
        mode: ScanMode,
    },
    RecordRootScan {
        id: i64,
        at: i64,
    },
    /// Mark a media file absent, or present again.
    SetFileMissing {
        id: i64,
        missing_since: Option<i64>,
        updated_at: i64,
    },
    /// Write one account's progress for one title or episode.
    UpsertWatchProgress(Box<peregrine_domain::WatchProgress>),
    /// Write one watchlist entry.
    UpsertWatchlist(Box<peregrine_domain::WatchlistEntry>),
    /// Remove one watchlist entry.
    RemoveWatchlist {
        account_id: peregrine_domain::AccountId,
        title_id: i64,
    },
    /// Refresh one file's stamp and root.
    RefreshFileStamp {
        id: i64,
        size_bytes: i64,
        mtime_ms: i64,
        root_id: i64,
        updated_at: i64,
    },
    /// Pin or unpin one cached image.
    PinImage {
        key: String,
        pinned: bool,
    },
    /// Remove every cached image whose key is listed.
    RemoveImages {
        keys: Vec<String>,
    },
    /// Remove one `match_rule` row.
    RemoveMatchRule {
        id: i64,
    },
    /// Record or replace one `match_rule` row.
    InsertMatchRule(Box<crate::catalogue::NewMatchRule>),
    /// Remove one `unmatched_file` row.
    RemoveUnmatchedFile {
        id: i64,
    },
    /// Insert or refresh one `image_cache` row.
    RecordImageCache(Box<crate::image_cache::ImageCacheEntry>),
    /// Report that one cached image was read.
    TouchImageCache {
        key: String,
        at: i64,
    },
    /// Remove one `image_cache` row.
    RemoveImageCache {
        key: String,
    },
    /// Insert or refresh one `notification` row.
    ///
    /// The read-then-write of the dedupe window happens on the writer's own connection,
    /// so two events arriving at once cannot both decide they are the first.
    RecordNotification(Box<NewCentreEntry>),
    /// Replace the whole Discover rails manifest with what one fetch published.
    ReplaceDiscoverRails {
        /// One entry per rail, each in the order that rail lists its titles.
        rails: Vec<(String, Vec<i64>)>,
    },
    /// Apply one delivery outcome to a `notification` row.
    MarkNotificationDelivered {
        id: NotificationId,
        delivery: Box<Delivery>,
    },
    /// Mark one `notification` row the scope may see as read.
    MarkNotificationRead {
        scope: AccountScope,
        id: NotificationId,
        at: i64,
    },
    /// Mark every unread `notification` row the scope may see as read.
    MarkAllNotificationsRead {
        scope: AccountScope,
        at: i64,
    },
    /// Dismiss one `notification` row the scope may see.
    DismissNotification {
        scope: AccountScope,
        id: NotificationId,
        at: i64,
    },
    /// Rewrite one title's search index row from the source tables.
    ReindexTitle {
        title_id: i64,
    },
    /// Remove one title's search index row.
    RemoveTitleIndex {
        title_id: i64,
    },
    /// Rebuild the whole search index from the source tables.
    RebuildSearchIndex,
    /// Merge the search index's b-tree segments, which is `FTS5`'s `optimize`.
    OptimizeSearchIndex,
    /// `ANALYZE`, so the query planner's statistics describe the data that is there.
    Analyze,
    /// Compact the database file and checkpoint the write-ahead log.
    ///
    /// It is dispatched on the connection in autocommit, never inside a transaction: `SQLite`
    /// refuses `VACUUM` from within one, and the checkpoint that makes the compaction visible
    /// on disk cannot be in one either.
    Vacuum,
    /// Prune one bounded batch of the rows a retention rule makes eligible.
    ///
    /// The instant comes from the plan rather than from a caller's clock read, and the limit
    /// is what bounds the statement, so a retention sweep arrives as a series of short writes
    /// rather than one long one (PG-E3-15).
    PruneRetention {
        /// The rule the batch prunes.
        rule: &'static crate::retention::RetentionRule,
        /// The instant the plan was taken at.
        at_millis: i64,
        /// The most rows this statement may remove.
        limit: u64,
    },
    /// Insert one row into `download_job`, in the state `FLOWS` F6 admits a job in.
    ///
    /// Boxed for the same reason [`WriteRequest::InsertJob`] is: the enum is the size of its
    /// largest variant and a job row is much larger than the alternatives.
    CreateDownloadJob(Box<crate::download::NewDownloadJob>),
    /// Apply one event to a `download_job` row, writing the safety report the event carries.
    TransitionDownloadJob {
        /// The row.
        id: i64,
        /// The event, which the domain judges against the state the row holds.
        event: Box<peregrine_domain::DownloadEvent>,
    },
    /// Delete one finished `download_job` row — the queue entry, never the files on disk.
    DeleteDownloadJob {
        /// The row.
        id: i64,
    },
    /// Delete every finished `download_job` row.
    DeleteFinishedDownloadJobs,
    /// Replace the offers a search found for one title, keeping anything already acted on.
    ReplaceReleaseOffers {
        /// The title the offers belong to.
        title_id: i64,
        /// The set the newest search found.
        offers: Vec<crate::offers::NewOffer>,
        /// Unix milliseconds.
        at: i64,
    },
    /// Mark one offer chosen and answer its source, serialized.
    ChooseReleaseOffer {
        /// The offer row.
        id: i64,
    },
    /// Mark one offer dismissed.
    DismissReleaseOffer {
        /// The offer row.
        id: i64,
    },
    /// Write one coalesced statistics tick onto a `download_job` row.
    RecordDownloadStatistics {
        /// The row.
        id: i64,
        /// The nine figures the tick carries.
        statistics: Box<crate::download::DownloadStatistics>,
    },
    /// Write a job's `download_file` rows from the metadata's list and the engine's selection.
    SelectDownloadFiles {
        /// The job.
        job_id: i64,
        /// The metadata's file list, which the inspection gate judged.
        description: Box<peregrine_domain::TorrentDescription>,
        /// The engine's selection, as indices into that list.
        selection: Vec<usize>,
    },
    /// Persist the engine's own state for a `download_job` row.
    RecordEngineState {
        /// The job.
        id: i64,
        /// The session's bytes, opaque to this crate.
        state: Vec<u8>,
    },
}

/// The writer's answer to a [`WriteRequest`].
#[derive(Debug)]
pub(crate) enum WriteResponse {
    /// The rowid `SQLite` assigned to the appended `event_log` row.
    EventAppended { id: i64 },
    /// The rowid `SQLite` assigned to the inserted `job` row.
    JobInserted { id: i64 },
    /// Whether a `job` row was there to change.
    JobUpdated { changed: bool },
    /// The rowid `SQLite` assigned to the inserted `title` row.
    TitleInserted { id: i64 },
    /// The external ids are attached.
    ExternalIdsAttached,
    /// The rowid `SQLite` assigned to an inserted library row.
    RowInserted { id: i64 },
    /// An `INSERT OR IGNORE` that found the row already there.
    RowIgnored,
    /// Whether a row was there to change or remove.
    RowChanged { changed: bool },
    /// The rowid an upsert wrote, and whether it moved anything: a re-run that stored the values
    /// already there reports `changed: false` rather than a second row.
    RowUpserted { id: i64, changed: bool },
    /// How many cached images a removal took out of the table.
    ImagesRemoved { removed: u64 },
    /// Every row of the request was applied, or a statement that either applies or fails with
    /// nothing to report (`ANALYZE`, the search index's `optimize`).
    Applied,
    /// The notification row was inserted, or refreshed inside its dedupe window.
    NotificationRecorded { recorded: Recorded },
    /// Whether a `notification` row was there to change.
    NotificationDelivered { changed: bool },
    /// Whether a `notification` row was there to mark read.
    NotificationRead { changed: bool },
    /// How many `notification` rows a mark-all-read changed.
    NotificationsRead { changed: u64 },
    /// Whether a `notification` row was there to dismiss.
    NotificationDismissed { changed: bool },
    /// Whether a search index row was written, which is `false` for an id no title holds.
    TitleReindexed { indexed: bool },
    /// Whether a search index row was removed.
    TitleIndexRemoved { removed: bool },
    /// How many titles the rebuild indexed.
    SearchIndexRebuilt { indexed: u64 },
    /// The database was compacted; `checkpointed` is false when the log could not be truncated,
    /// so the compacted file is not yet smaller on disk.
    Vacuumed {
        /// Whether the write-ahead log was checkpointed.
        checkpointed: bool,
    },
    /// How many rows a retention batch removed.
    RetentionPruned { rows: u64 },
    /// The `download_job` row the writer inserted, as the database holds it.
    DownloadJobCreated(Box<crate::download::DownloadJobRow>),
    /// The state a `download_job` row moved to, or `None` when no such row exists.
    DownloadJobTransitioned {
        state: Option<peregrine_domain::DownloadState>,
    },
    /// The domain refused the event for the state a `download_job` row holds.
    DownloadJobRefused {
        rejection: peregrine_domain::TransitionRejected,
    },
    /// Whether a finished `download_job` row was deleted.
    DownloadJobDeleted { removed: bool },
    /// How many finished `download_job` rows a clear removed.
    DownloadJobsCleared { removed: u64 },
    /// How many offers a search stored.
    ReleaseOffersStored { written: u64 },
    /// The offer a choice named, or `None` when it was already acted on.
    ReleaseOfferChosen {
        chosen: Option<crate::offers::ChosenOfferRow>,
    },
}

/// A row to append to `event_log` (`DATA_MODEL` §2.8).
///
/// The timestamp and the detail are supplied rather than derived: the caller owns the clock, so a
/// test can write a deterministic timestamp and a replay can write the original one. The kind,
/// the subject kind, and the subject id are the one thing a caller cannot supply, because they
/// come from [`peregrine_domain::Event`]: the declared vocabulary is the only way to name an
/// event, so a repository cannot write a name this build does not know.
///
/// ```
/// # use peregrine_domain::Event;
/// # use peregrine_storage::NewEvent;
/// let stored = NewEvent {
///     at_millis: 0,
///     event: Event::DownloadFailed { job_id: 7 },
///     detail_json: None,
/// };
/// assert_eq!(stored.event.as_str(), "download.failed");
/// assert_eq!(stored.event.subject_id(), 7);
/// ```
///
/// ```compile_fail
/// # use peregrine_storage::NewEvent;
/// // A string kind is not a constructor: `NewEvent` has no `kind` field.
/// let _ = NewEvent {
///     at_millis: 0,
///     kind: "download.invented".to_owned(),
///     subject_kind: Some("download_job".to_owned()),
///     subject_id: Some(7),
///     detail_json: None,
/// };
/// ```
#[derive(Debug, Clone)]
pub struct NewEvent {
    /// Unix milliseconds UTC.
    pub at_millis: i64,
    /// The declared operation, which is also the row's kind and subject.
    pub event: Event,
    /// Pre-serialized JSON detail; never a secret (ARCHITECTURE §12).
    pub detail_json: Option<String>,
}

/// A cloneable handle to the writer task.
#[derive(Debug, Clone)]
pub(crate) struct WriterHandle {
    requests: mpsc::Sender<Envelope>,
}

/// A request together with the channel its answer comes back on.
#[derive(Debug)]
struct Envelope {
    request: WriteRequest,
    reply: oneshot::Sender<Result<WriteResponse, StorageError>>,
}

impl WriterHandle {
    /// Starts the writer task that owns `connection`.
    pub(crate) fn spawn(connection: sqlx::SqliteConnection) -> Self {
        let (requests, receiver) = mpsc::channel(WRITE_QUEUE_DEPTH);
        tokio::spawn(run(connection, receiver));
        Self { requests }
    }

    /// Applies one request and waits for its outcome.
    pub(crate) async fn send(&self, request: WriteRequest) -> Result<WriteResponse, StorageError> {
        let (reply, answer) = oneshot::channel();
        self.requests
            .send(Envelope { request, reply })
            .await
            .map_err(|_| StorageError::WriterStopped)?;
        answer.await.map_err(|_| StorageError::WriterStopped)?
    }
}

/// Applies the requests that write the library and watch state.
///
/// Split from [`apply`] because the arm list is a dispatch table: the catalogue, jobs, and
/// notification arms each read as one line, and thirteen library arms in the middle of them
/// hide both halves. The channel is still one, and every request still arrives in order.
async fn apply_library_write(
    connection: &mut sqlx::SqliteConnection,
    request: WriteRequest,
) -> Result<WriteResponse, StorageError> {
    match request {
        WriteRequest::InsertLibraryRoot(root) => crate::library::insert_root(connection, &root)
            .await
            .map(|id| WriteResponse::RowInserted { id })
            .map_err(StorageError::from_sqlx),
        WriteRequest::InsertMediaItem(item) => crate::library::insert_item(connection, &item)
            .await
            .map(|id| WriteResponse::RowInserted { id })
            .map_err(StorageError::from_sqlx),
        WriteRequest::InsertMediaFile(file) => crate::library::insert_file(connection, &file)
            .await
            .map(|id| WriteResponse::RowInserted { id })
            .map_err(StorageError::from_sqlx),
        WriteRequest::RemoveUnmatchedFile { id } => remove_unmatched_file(connection, id).await,
        WriteRequest::InsertUnmatchedFile {
            file,
            ignore_conflict,
        } => crate::library::insert_unmatched(connection, &file, ignore_conflict)
            .await
            .map(|inserted| match inserted {
                Some(id) => WriteResponse::RowInserted { id },
                None => WriteResponse::RowIgnored,
            })
            .map_err(StorageError::from_sqlx),
        WriteRequest::InsertSubtitleTrack(track) => insert_row(
            crate::library::insert_subtitle(connection, &track)
                .await
                .map_err(StorageError::from_sqlx),
        ),
        WriteRequest::RemoveLibraryRoot { id } => crate::library::remove_root(connection, id)
            .await
            .map(|removed| WriteResponse::RowChanged { changed: removed })
            .map_err(StorageError::from_sqlx),
        WriteRequest::SetRootScanMode { id, mode } => {
            crate::library::set_root_scan_mode(connection, id, mode)
                .await
                .map(|changed| WriteResponse::RowChanged { changed })
                .map_err(StorageError::from_sqlx)
        }
        WriteRequest::RecordRootScan { id, at } => {
            crate::library::record_root_scan(connection, id, at)
                .await
                .map(|changed| WriteResponse::RowChanged { changed })
                .map_err(StorageError::from_sqlx)
        }
        WriteRequest::SetFileMissing {
            id,
            missing_since,
            updated_at,
        } => crate::library::set_file_missing(connection, id, missing_since, updated_at)
            .await
            .map(|changed| WriteResponse::RowChanged { changed })
            .map_err(StorageError::from_sqlx),
        WriteRequest::RefreshFileStamp {
            id,
            size_bytes,
            mtime_ms,
            root_id,
            updated_at,
        } => refresh_file_stamp(connection, id, size_bytes, mtime_ms, root_id, updated_at).await,
        WriteRequest::RecordImageCache(entry) => crate::image_cache::record(connection, &entry)
            .await
            .map(|()| WriteResponse::Applied)
            .map_err(StorageError::from_sqlx),
        WriteRequest::TouchImageCache { key, at } => {
            crate::image_cache::touch(connection, &key, at)
                .await
                .map(|changed| WriteResponse::RowChanged { changed })
                .map_err(StorageError::from_sqlx)
        }
        WriteRequest::RemoveImageCache { key } => crate::image_cache::remove(connection, &key)
            .await
            .map(|removed| WriteResponse::RowChanged { changed: removed })
            .map_err(StorageError::from_sqlx),
        WriteRequest::SetDefaultQualityResolutions {
            resolutions_json,
            at,
        } => crate::quality::set_default_resolutions(connection, &resolutions_json, at)
            .await
            .map(|updated| {
                if updated {
                    WriteResponse::Applied
                } else {
                    WriteResponse::RowIgnored
                }
            })
            .map_err(StorageError::from_sqlx),
        WriteRequest::InsertDefaultQualityProfile {
            name,
            rules_json,
            cutoff_rank,
            upgrade_allowed,
            at,
        } => crate::quality::insert_default(
            connection,
            &name,
            &rules_json,
            cutoff_rank,
            upgrade_allowed,
            at,
        )
        .await
        .map(|inserted| {
            if inserted {
                WriteResponse::Applied
            } else {
                WriteResponse::RowIgnored
            }
        })
        .map_err(StorageError::from_sqlx),
        WriteRequest::UpsertWatchProgress(progress) => {
            crate::watch::upsert_progress(connection, &progress)
                .await
                .map(|()| WriteResponse::Applied)
                .map_err(StorageError::from_sqlx)
        }
        WriteRequest::UpsertWatchlist(entry) => crate::watch::upsert_watchlist(connection, &entry)
            .await
            .map(|()| WriteResponse::Applied)
            .map_err(StorageError::from_sqlx),
        WriteRequest::RemoveWatchlist {
            account_id,
            title_id,
        } => crate::watch::remove_watchlist(connection, account_id, title_id)
            .await
            .map(|changed| WriteResponse::RowChanged { changed })
            .map_err(StorageError::from_sqlx),
        // Reachable only if `apply` stops dispatching one of them here, which is a
        // compile-time property of the two matches rather than a runtime state.
        other => {
            unreachable!("`apply` routed a non-library request to the library writer: {other:?}")
        }
    }
}

/// Applies requests one at a time until every handle is dropped.
async fn run(mut connection: sqlx::SqliteConnection, mut requests: mpsc::Receiver<Envelope>) {
    while let Some(envelope) = requests.recv().await {
        let outcome = apply(&mut connection, envelope.request).await;
        // A caller that gave up (a cancelled task dropping its oneshot receiver) must not
        // tear down the only write connection: the write already happened, and the next
        // caller still needs a writer.
        let _ = envelope.reply.send(outcome);
    }
}

/// Refreshes one file's stamp, mapping the statement onto the response the caller expects.
async fn refresh_file_stamp(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
    size_bytes: i64,
    mtime_ms: i64,
    root_id: i64,
    updated_at: i64,
) -> Result<WriteResponse, StorageError> {
    crate::library::refresh_file_stamp(connection, id, size_bytes, mtime_ms, root_id, updated_at)
        .await
        .map(|changed| WriteResponse::RowChanged { changed })
        .map_err(StorageError::from_sqlx)
}

/// Wraps an insert's outcome as the response a caller expects.
///
/// A season, an episode, and a match rule are three rows with one shape: insert, answer the id. The
/// helper is what keeps the dispatch a list of what each request *means* rather than of how each
/// reply is wrapped.
fn insert_row(result: Result<i64, StorageError>) -> Result<WriteResponse, StorageError> {
    result.map(|id| WriteResponse::RowInserted { id })
}

/// Removes one `match_rule` row, mapping the statement onto the response the caller expects.
async fn remove_match_rule(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
) -> Result<WriteResponse, StorageError> {
    crate::catalogue::remove_match_rule(connection, id)
        .await
        .map(|removed| WriteResponse::RowChanged { changed: removed })
        .map_err(StorageError::from_sqlx)
}

/// Removes one `unmatched_file` row, mapping the statement onto the response the caller expects.
async fn remove_unmatched_file(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
) -> Result<WriteResponse, StorageError> {
    crate::library::remove_unmatched_file(connection, id)
        .await
        .map(|removed| WriteResponse::RowChanged { changed: removed })
        .map_err(StorageError::from_sqlx)
}

/// Applies one request on the writer's connection.
async fn apply(
    connection: &mut sqlx::SqliteConnection,
    request: WriteRequest,
) -> Result<WriteResponse, StorageError> {
    match request {
        WriteRequest::AppendEvent(event) => append_event(connection, &event)
            .await
            .map(|id| WriteResponse::EventAppended { id })
            .map_err(StorageError::from_sqlx),
        WriteRequest::InsertJob(job) => crate::jobs::insert(connection, &job)
            .await
            .map(|id| WriteResponse::JobInserted { id })
            .map_err(StorageError::from_sqlx),
        WriteRequest::UpdateJob { id, change } => crate::jobs::update(connection, id, &change)
            .await
            .map(|changed| WriteResponse::JobUpdated { changed })
            .map_err(StorageError::from_sqlx),
        WriteRequest::AttachExternalIds { title_id, ids } => {
            crate::catalogue::attach_external_ids(connection, title_id, &ids)
                .await
                .map(|()| WriteResponse::ExternalIdsAttached)
        }
        WriteRequest::RecordNotification(entry) => {
            crate::notify::insert_or_refresh(connection, &entry)
                .await
                .map(|recorded| WriteResponse::NotificationRecorded { recorded })
        }
        WriteRequest::MarkNotificationDelivered { id, delivery } => {
            crate::notify::apply_delivery(connection, id, &delivery)
                .await
                .map(|changed| WriteResponse::NotificationDelivered { changed })
        }
        WriteRequest::MarkNotificationRead { scope, id, at } => {
            crate::notify::mark_read(connection, scope, id, at)
                .await
                .map(|changed| WriteResponse::NotificationRead { changed })
        }
        WriteRequest::MarkAllNotificationsRead { scope, at } => {
            crate::notify::mark_all_read(connection, scope, at)
                .await
                .map(|changed| WriteResponse::NotificationsRead { changed })
        }
        WriteRequest::DismissNotification { scope, id, at } => {
            crate::notify::dismiss(connection, scope, id, at)
                .await
                .map(|changed| WriteResponse::NotificationDismissed { changed })
        }
        WriteRequest::ReindexTitle { title_id } => {
            let mut transaction = connection.begin().await.map_err(StorageError::from_sqlx)?;
            let indexed = crate::fts::reindex(&mut transaction, title_id)
                .await
                .map_err(StorageError::from_sqlx)?;
            transaction
                .commit()
                .await
                .map_err(StorageError::from_sqlx)?;
            Ok(WriteResponse::TitleReindexed { indexed })
        }
        WriteRequest::RemoveTitleIndex { title_id } => {
            let mut transaction = connection.begin().await.map_err(StorageError::from_sqlx)?;
            let removed = crate::fts::unindex(&mut transaction, title_id)
                .await
                .map_err(StorageError::from_sqlx)?;
            transaction
                .commit()
                .await
                .map_err(StorageError::from_sqlx)?;
            Ok(WriteResponse::TitleIndexRemoved { removed })
        }
        // The rebuild runs on the writer's connection, never a second one: a rebuild that
        // raced the write path would interleave its `delete-all` with a title being indexed.
        WriteRequest::RebuildSearchIndex => crate::fts::rebuild(connection)
            .await
            .map(|indexed| WriteResponse::SearchIndexRebuilt { indexed })
            .map_err(StorageError::from_sqlx),
        WriteRequest::OptimizeSearchIndex => crate::fts::optimize(connection)
            .await
            .map(|()| WriteResponse::Applied)
            .map_err(StorageError::from_sqlx),
        WriteRequest::Analyze => sqlx::query("ANALYZE")
            .execute(&mut *connection)
            .await
            .map(|_| WriteResponse::Applied)
            .map_err(StorageError::from_sqlx),
        WriteRequest::Vacuum => crate::maintenance::vacuum(connection)
            .await
            .map(|checkpointed| WriteResponse::Vacuumed { checkpointed })
            .map_err(StorageError::from_sqlx),
        WriteRequest::PruneRetention {
            rule,
            at_millis,
            limit,
        } => crate::retention::delete_batch(connection, rule, at_millis, limit)
            .await
            .map(|rows| WriteResponse::RetentionPruned { rows })
            .map_err(StorageError::from_sqlx),
        other => apply_download_write(connection, other).await,
    }
}

/// Applies the requests that write acquisition rows.
///
/// Split out of the dispatcher it fell into for the length lint, the way the catalogue and library
/// dispatchers were: one channel carries every request whatever this chain's order is, so the chain
/// is about reading one match at a time. It falls through to the catalogue dispatcher.
async fn apply_download_write(
    connection: &mut sqlx::SqliteConnection,
    request: WriteRequest,
) -> Result<WriteResponse, StorageError> {
    match request {
        WriteRequest::CreateDownloadJob(job) => crate::download::insert(connection, &job)
            .await
            .map(|row| WriteResponse::DownloadJobCreated(Box::new(row))),
        WriteRequest::TransitionDownloadJob { id, event } => {
            // A rejection is an outcome rather than an error: the domain refused the event for the
            // state the row holds, and the caller matches on it instead of reading a message.
            match crate::download::transition(connection, id, *event).await {
                Ok(state) => Ok(WriteResponse::DownloadJobTransitioned { state }),
                Err(crate::download::DownloadTransitionError::Rejected(rejection)) => {
                    Ok(WriteResponse::DownloadJobRefused { rejection })
                }
                Err(crate::download::DownloadTransitionError::Storage(error)) => Err(error),
            }
        }
        WriteRequest::DeleteDownloadJob { id } => crate::download::delete_finished(connection, id)
            .await
            .map(|removed| WriteResponse::DownloadJobDeleted { removed }),
        WriteRequest::DeleteFinishedDownloadJobs => {
            crate::download::delete_all_finished(connection)
                .await
                .map(|removed| WriteResponse::DownloadJobsCleared { removed })
        }
        WriteRequest::ReplaceReleaseOffers {
            title_id,
            offers,
            at,
        } => crate::offers::replace_offers(connection, title_id, &offers, at)
            .await
            .map(|written| WriteResponse::ReleaseOffersStored {
                written: written as u64,
            }),
        WriteRequest::ChooseReleaseOffer { id } => crate::offers::choose_offer(connection, id)
            .await
            .map(|chosen| WriteResponse::ReleaseOfferChosen { chosen }),
        WriteRequest::DismissReleaseOffer { id } => crate::offers::dismiss_offer(connection, id)
            .await
            .map(|changed| WriteResponse::RowChanged { changed }),
        WriteRequest::RecordDownloadStatistics { id, statistics } => {
            crate::download::record_statistics(connection, id, &statistics)
                .await
                .map(|changed| WriteResponse::RowChanged { changed })
        }
        WriteRequest::SelectDownloadFiles {
            job_id,
            description,
            selection,
        } => crate::download::select_files(connection, job_id, &description, &selection)
            .await
            .map(|()| WriteResponse::Applied),
        WriteRequest::RecordEngineState { id, state } => {
            crate::download::record_engine_state(connection, id, &state)
                .await
                .map(|changed| WriteResponse::RowChanged { changed })
        }
        other => apply_catalogue_write(connection, other).await,
    }
}

/// Applies the requests that write catalogue rows.
///
/// Split out of the library dispatch because that function kept reaching the length lint one arm at a
/// time, and the reason was structural: it was the *last* dispatcher in the chain, so every new
/// request landed in it whatever it wrote. The chain is now matter, then acquisition, then
/// catalogue, then library.
async fn apply_catalogue_write(
    connection: &mut sqlx::SqliteConnection,
    request: WriteRequest,
) -> Result<WriteResponse, StorageError> {
    match request {
        WriteRequest::InsertMatchRule(rule) => {
            insert_row(crate::catalogue::insert_match_rule(connection, &rule).await)
        }
        WriteRequest::PinImage { key, pinned } => {
            crate::image_cache::set_pinned(connection, &key, pinned)
                .await
                .map(|changed| WriteResponse::RowChanged { changed })
        }
        WriteRequest::RemoveImages { keys } => crate::image_cache::remove_many(connection, &keys)
            .await
            .map(|removed| WriteResponse::ImagesRemoved { removed }),
        WriteRequest::RemoveMatchRule { id } => remove_match_rule(connection, id).await,
        WriteRequest::UpdateTitleMetadata {
            id,
            details,
            sort_name,
            state,
            provider,
            at,
        } => crate::catalogue::update_title_metadata(
            connection, id, &details, &sort_name, state, &provider, at,
        )
        .await
        .map(|changed| WriteResponse::RowChanged { changed }),
        WriteRequest::UpsertProviderTitle(title) => {
            crate::catalogue::upsert_provider_title(connection, &title)
                .await
                .map(|id| WriteResponse::TitleInserted { id })
        }
        WriteRequest::ReplaceDiscoverRails { rails } => {
            crate::discover_rail::replace(connection, &rails)
                .await
                .map(|()| WriteResponse::Applied)
        }
        WriteRequest::InsertStubTitle(stub) => crate::catalogue::insert_stub(connection, &stub)
            .await
            .map(|id| WriteResponse::TitleInserted { id }),
        WriteRequest::InsertStubSeason(season) => {
            insert_row(crate::catalogue::insert_season(connection, &season).await)
        }
        WriteRequest::InsertStubEpisode(episode) => {
            insert_row(crate::catalogue::insert_episode(connection, &episode).await)
        }
        WriteRequest::UpsertSeasonDetails {
            title_id,
            details,
            provider,
            at,
        } => crate::catalogue::upsert_season_details(connection, title_id, &details, &provider, at)
            .await
            .map(|(id, changed)| WriteResponse::RowUpserted { id, changed }),
        WriteRequest::UpsertEpisodeDetails {
            title_id,
            season_id,
            details,
            provider,
            at,
        } => crate::catalogue::upsert_episode_details(
            connection, title_id, season_id, &details, &provider, at,
        )
        .await
        .map(|changed| WriteResponse::RowChanged { changed }),
        other => apply_library_write(connection, other).await,
    }
}

/// Inserts one `event_log` row and reports its rowid.
async fn append_event(
    connection: &mut sqlx::SqliteConnection,
    new_event: &NewEvent,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO event_log (at, kind, subject_kind, subject_id, detail_json) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(new_event.at_millis)
    .bind(new_event.event.as_str())
    .bind(new_event.event.subject_kind().as_str())
    .bind(new_event.event.subject_id())
    .bind(new_event.detail_json.as_deref())
    .execute(&mut *connection)
    .await?;
    Ok(result.last_insert_rowid())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::open_writer_connection;
    use crate::migrations;
    use peregrine_domain::{SubjectKind, WatchSubject};
    use sqlx::Row as _;
    use tempfile::TempDir;

    /// A declared event, because the writer accepts nothing else (PG-E4-16); the sequence
    /// stands in for a job id so two rows are distinguishable.
    fn event(sequence: i64) -> NewEvent {
        NewEvent {
            at_millis: 1_700_000_000_000 + sequence,
            event: Event::DownloadFailed { job_id: sequence },
            detail_json: None,
        }
    }

    /// A caller that is cancelled while waiting for its answer must not take the writer
    /// with it: the write already happened, and the next caller still needs a writer.
    ///
    /// The default `#[tokio::test]` flavour is a single thread, so the writer task cannot
    /// run between the request being queued and the reply receiver being dropped, which
    /// makes the dropped-reply branch deterministic rather than a race.
    #[tokio::test]
    async fn a_dropped_reply_does_not_stop_the_writer() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");
        let mut connection = open_writer_connection(&path).await.unwrap();
        migrations::apply(&mut connection, &path).await.unwrap();

        let (requests, receiver) = mpsc::channel(WRITE_QUEUE_DEPTH);
        let writer = tokio::spawn(run(connection, receiver));

        let (reply, answer) = oneshot::channel();
        requests
            .send(Envelope {
                request: WriteRequest::AppendEvent(event(0)),
                reply,
            })
            .await
            .unwrap();
        drop(answer);

        let handle = WriterHandle { requests };
        let response = handle
            .send(WriteRequest::AppendEvent(event(1)))
            .await
            .unwrap();
        let WriteResponse::EventAppended { id } = response else {
            panic!("the writer answered `append_event` with another request's response");
        };
        assert!(
            id >= 1,
            "the writer must still answer after a reply was dropped"
        );

        // The task exits when the last sender is dropped; awaiting it proves it was not
        // torn down by the failed reply.
        drop(handle);
        writer.await.unwrap();
    }

    #[tokio::test]
    async fn a_write_with_no_writer_reports_writer_stopped() {
        // The receiver is dropped, which is what a finished or panicked writer task
        // leaves behind; the caller must get an error rather than hang forever.
        let (requests, receiver) = mpsc::channel(WRITE_QUEUE_DEPTH);
        drop(receiver);
        let handle = WriterHandle { requests };

        let error = handle
            .send(WriteRequest::AppendEvent(NewEvent {
                at_millis: 0,
                event: Event::DownloadQueued { job_id: 1 },
                detail_json: None,
            }))
            .await
            .expect_err("a closed channel has no writer behind it");

        assert!(
            matches!(error, StorageError::WriterStopped),
            "got {error:?}"
        );
    }

    /// PG-E4-16: what the vocabulary declares is what the row holds and what a reader rebuilds.
    /// The row is read back through the three columns rather than through the caller's value, so
    /// the check covers the write path and the read path and not just the type.
    #[tokio::test]
    async fn an_appended_event_reads_back_as_the_declared_event() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");
        let mut connection = open_writer_connection(&path).await.unwrap();
        migrations::apply(&mut connection, &path).await.unwrap();

        let (requests, receiver) = mpsc::channel(WRITE_QUEUE_DEPTH);
        let writer = tokio::spawn(run(connection, receiver));
        let handle = WriterHandle { requests };

        let declared = Event::ProgressWatched {
            subject: WatchSubject::Episode(9),
        };
        let response = handle
            .send(WriteRequest::AppendEvent(NewEvent {
                at_millis: 1_700_000_000_000,
                event: declared,
                detail_json: Some("{\"percent\":0.97}".to_owned()),
            }))
            .await
            .unwrap();
        let WriteResponse::EventAppended { id } = response else {
            panic!("the writer answered `append_event` with another request's response");
        };
        drop(handle);
        writer.await.unwrap();

        let mut reader = open_writer_connection(&path).await.unwrap();
        let row = sqlx::query(
            "SELECT at, kind, subject_kind, subject_id, detail_json FROM event_log WHERE id = ?1",
        )
        .bind(id)
        .fetch_one(&mut reader)
        .await
        .unwrap();

        let kind: String = row.get("kind");
        let subject_kind: String = row.get("subject_kind");
        let subject_id: i64 = row.get("subject_id");
        assert_eq!(row.get::<i64, _>("at"), 1_700_000_000_000);
        assert_eq!(kind, "progress.watched");
        assert_eq!(subject_kind, "episode");
        assert_eq!(subject_id, 9);
        assert_eq!(
            row.get::<Option<String>, _>("detail_json").as_deref(),
            Some("{\"percent\":0.97}")
        );

        let parsed = Event::parse(
            &kind,
            SubjectKind::parse(&subject_kind).expect("the row holds a declared subject kind"),
            subject_id,
        );
        assert_eq!(
            parsed,
            Some(declared),
            "an appended event must read back as the event it was"
        );
    }
}
