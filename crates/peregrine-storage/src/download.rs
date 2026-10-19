//! The acquisition repositories: `download_job` and `download_file` (`DATA_MODEL` §2.7).
//!
//! Two aggregates with one caller each: [`crate::Database::create_download_job`] and its
//! siblings are what `PG-E10-19`'s `AcquisitionService` drives through `FLOWS` F6, and the two
//! job reads ([`crate::Database::download_job`], [`crate::Database::list_download_jobs`]) are
//! what the queue screen (`PG-E13-03`) pages. Nothing here decides anything: the inspection gate
//! judges, the service sequences, and this module stores what it is told and reports what it
//! finds.
//!
//! # The state column has one writer
//!
//! `download_job.state` is written by [`transition`] and by nothing else. No parameter of any
//! method here is a state, no statement binds one, and [`DownloadJobRow`] is a read type. In one
//! transaction that function reads the state the row holds, calls
//! [`peregrine_domain::transition`] — the domain's own machine, whose module doc says a
//! repository that writes a state must have obtained it from there — and writes the state it
//! returned together with the [`SafetyReport`] the event carries.
//!
//! I4 therefore holds on the write path and not only in the domain: there is no way to reach
//! `metadata` that does not write the report in the statement that reaches it, and a refused
//! transition writes nothing at all. A report-carrying event that is refused leaves the stored
//! report exactly as it was — the `UPDATE` is only built once the transition has succeeded, and
//! an event that carries no report does not touch the column, so a job dismissed from `blocked`
//! keeps the report that explains the block.
//!
//! # I36 is the database's, and its violation is typed
//!
//! `download_job_hash_uq` is a partial unique index over the *live* states (`queued`,
//! `inspecting`, `metadata`, `downloading`, `paused`, `seeding` — migration 0008's set), so
//! `SQLite` is what refuses a second live job for one info hash. [`insert`] turns that one
//! violation into [`StorageError::Conflict`], which a caller can match, and lets every other
//! failure through as the statement failure it is. Only that index can be violated by this
//! statement, because the rowid is assigned by the database and the table holds no other unique
//! constraint. A terminal job — and a `blocked` one, which is waiting for a user rather than
//! holding a torrent in the engine — leaves the live set, so a retry after a failure or a
//! removal is a new row rather than a conflict (`FLOWS` SM1).
//!
//! # Cursors
//!
//! [`list_jobs`] orders by `id DESC`, so the cursor is the row id and is total by construction:
//! `SQLite` numbers an `INTEGER PRIMARY KEY` uniquely. The state filter is a *set* rather than a
//! column value, so it arrives as a JSON array of the enum's own spellings and is matched with
//! `json_each` — JSON1 is compiled into the bundled `SQLite` this crate links
//! (`libsqlite3-sys`'s build sets `SQLITE_ENABLE_JSON1`), and the alternative is a statement
//! assembled per call, which is the shape `sqlx` 0.9 makes a caller audit. The array is rendered
//! by [`crate::json::encode_string_array`], the crate's one JSON array encoder, so the spellings
//! cannot be split on a delimiter they contain.
//!
//! **The cost, as `EXPLAIN QUERY PLAN` answers it:** a scan of `download_job` in primary-key
//! order with the state list materialized once and no temporary `B-tree` for the ordering — so a
//! page costs the rows it returns plus the rows it walks past to find them, and it stops as soon
//! as the limit is met. `download_job_active_idx` is not the plan: it is ordered by
//! `(state, priority)` and this read is ordered by `id`, so using it would mean merging the states
//! into one sorted stream, which is a sort rather than a walk. A state set that matches few rows
//! therefore walks whatever lies above them, which is the price of one total cursor over a set of
//! states rather than one cursor per state.
//!
//! [`files`] is the one read here that is not paged: its bound is one torrent's own metadata list
//! rather than the table's growth, exactly as `library::present_files` reads one root's files,
//! and the caller wants that list whole rather than resumed.
//!
//! # What an engine state is stored as
//!
//! `engine_state_json` is declared `TEXT` and the engine's `EngineState` is opaque bytes — a type
//! this crate may not even name, because `peregrine-storage` may depend on `peregrine-domain`
//! alone (`cargo xtask check-arch`). The column is written and read **as those bytes**: `SQLite`
//! stores a value of any type in any column, and `media_file.probe_json` — the crate's other
//! opaque payload in a `TEXT`-declared column — already travels this way, so a second encoding
//! would be a second spelling of one thing. The cost is stated rather than hidden: the column's
//! declared type does not describe a value stored this way, so only a reader that asks for bytes
//! can read it, a `CAST(… AS TEXT)` or a `LIKE` over it is meaningless, and the `_json` suffix is
//! a name no migration may change. The alternatives were both worse: hexadecimal doubles every
//! state, and an array of byte values costs about 3.5 bytes per byte; neither is the engine's own
//! format, and neither buys anything a caller can observe.
//!
//! # Time, scope, and what is deliberately absent
//!
//! Every instant comes from the caller ([`NewDownloadJob::added_at`], and nothing here reads a
//! clock — AGENTS R14, the way `library.rs` and `watch.rs` do it). Every read takes an
//! [`AccountScope`] and applies it in SQL: the scope is the flag
//! [`crate::catalogue::hidden_by_scope`] gives, the same one `catalogue.rs`, `library.rs`, and
//! `discover_rail.rs` bind, so a job for a title outside the scope is indistinguishable from a
//! job that does not exist (AGENTS R34).
//!
//! `media_item_id`, `quality_profile_id`, `indexer_id`, `job_id`, and `is_upgrade` are read but
//! never written: the admission `FLOWS` F6 describes does not know them, and each belongs to the
//! task that does (`PG-E12-10`'s upgrade decision, `PG-E14`'s import). `blocked_reason` on a file
//! row is the same: `PG-E11-07` is its writer. None of them is a value this module guesses at.

use std::fmt;
use std::path::Path;

use jiff::Timestamp;
use peregrine_domain::{
    AccountScope, DownloadEvent, DownloadStage, DownloadState, EntryKind, InfoHash, ItemJob,
    NonEmptyText, NzbHash, PayloadDescription, PayloadEntry, Protocol, SafetyReport,
    TransitionRejected, is_media_extension,
};
use sqlx::Connection as _;

use crate::catalogue::hidden_by_scope;
use crate::error::StorageError;
use crate::json::encode_string_array;
use crate::page::PageRequest;

/// Who asked for a download job, as `download_job.added_by` stores it.
///
/// The database's `CHECK` constraint lists exactly these spellings, and a test asserts that
/// order against the schema; the enum is what stops a caller from inventing another, the way
/// [`crate::JobPhase`] does for the journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddedBy {
    /// A person pressed a download button.
    User,
    /// An auto-acquisition rule (`PG-E12`) chose the release without being asked.
    Auto,
}

impl AddedBy {
    /// Every variant, in the order the `CHECK` constraint lists them.
    pub const ALL: &[Self] = &[Self::User, Self::Auto];

    /// The spelling the `download_job.added_by` constraint accepts.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Auto => "auto",
        }
    }

    /// Parses the stored spelling, or `None` when the column holds something else.
    ///
    /// A `CHECK` constraint makes an unknown value impossible, so this is a shape guard against a
    /// database written by another version rather than a validation step.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }
}

/// A magnet URI, as `download_job.magnet_uri` stores it.
///
/// The wrapper exists for its [`fmt::Debug`]: a derive over a bare `String` would put the URI
/// into the first `tracing` field or panic message that formatted a job, and a magnet carries
/// the info-hash and a tracker list besides (SECURITY T13 — [`InfoHash`]'s own `Debug` prints
/// nothing for the same reason). Nothing here validates the URI: the deep-link parser and the
/// indexer adapters own that grammar, and a second one would be a second answer to what a magnet
/// is.
#[derive(Clone, PartialEq, Eq)]
pub struct MagnetUri(NonEmptyText);

impl MagnetUri {
    /// Wraps the URI text the row is to hold.
    #[must_use]
    pub const fn new(value: NonEmptyText) -> Self {
        Self(value)
    }

    /// The URI, for the caller that hands it back to the engine.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for MagnetUri {
    /// Prints the type and nothing of the URI.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MagnetUri(..)")
    }
}

/// Where a job's payload comes from, as the source columns `download_job` stores it in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadSource {
    /// A magnet URI the user, an indexer, or a deep link supplied.
    Magnet(MagnetUri),
    /// The application's own copy of the `.torrent` file, by path.
    ///
    /// The path is what the row stores; the *contents* are what the engine is handed, so the
    /// file has to still exist when the job is added (`PG-E10-08`, ADR 0005).
    TorrentFile(NonEmptyText),
    /// The application's own copy of the `.nzb` file, by path.
    ///
    /// The path is what the row stores; the parsed document is what the engine is handed.
    NzbFile(NonEmptyText),
}

/// A download job as it is admitted, before the gate has run (`FLOWS` F6).
///
/// Every column an admission knows, and nothing else. `state` is not a field because the
/// statement writes `queued` as a literal — the one state F6 admits a job in, and the only state
/// a caller may not choose. `safety_report_json` and `engine_state_json` are written later, by
/// the gate's transition and by the engine respectively. The timestamps a transfer sets
/// (`started_at`, `completed_at`, `seeding_until`, `removed_at`) and the failure pair have no
/// writer in this slice because their caller — the queue's own actions (`PG-E13`) — is not in it.
#[derive(Debug, Clone, PartialEq)]
pub struct NewDownloadJob {
    /// The title the job is for.
    pub title_id: i64,
    /// The episode, for a series, or `None` for a film.
    pub episode_id: Option<i64>,
    /// The release's title as the indexer wrote it.
    pub release_title: String,
    /// The source tag the release name parsed to, when it had one.
    pub release_source: Option<String>,
    /// The release group, when the name named one.
    pub release_group: Option<String>,
    /// The resolution tag, when the name carried one.
    pub resolution: Option<String>,
    /// The video codec tag, when the name carried one.
    pub video_codec: Option<String>,
    /// The high-dynamic-range format, when the name named one.
    pub hdr_format: Option<String>,
    /// What the name said about the audio.
    pub audio_summary: Option<String>,
    /// The size the indexer claimed, which is a claim until the metadata arrives.
    pub reported_size_bytes: Option<u64>,
    /// The torrent's identity, when the release carried one.
    pub info_hash: Option<InfoHash>,
    /// The protocol this job transfers over.
    pub protocol: Protocol,
    /// The NZB's SHA-256, when the source is Usenet.
    pub nzb_sha256: Option<NzbHash>,
    /// How the engine is told what to transfer.
    pub source: DownloadSource,
    /// The absolute destination the guard resolved, which every file path is relative to.
    pub save_path: NonEmptyText,
    /// Queue priority; the scheduler's admission order is what reads it (`PG-E6-01`).
    pub priority: i64,
    /// Who asked for it.
    pub added_by: AddedBy,
    /// Unix milliseconds when the job was admitted.
    pub added_at: Timestamp,
}

/// One coalesced statistics tick (`FLOWS` F6).
///
/// The caller coalesces to at most one write per second per job; this module does not, because a
/// tick is one statement either way and a second coalescer here would be a second place the
/// interval is decided.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DownloadStatistics {
    /// Payload bytes received so far.
    pub downloaded_bytes: u64,
    /// Payload bytes sent back so far.
    pub uploaded_bytes: u64,
    /// Completion in permille, where 1000 is the whole payload.
    pub progress_permille: i64,
    /// Current download rate in bytes per second.
    pub download_rate_bps: u64,
    /// Current upload rate in bytes per second.
    pub upload_rate_bps: u64,
    /// Peers this torrent is connected to.
    pub peers_connected: u32,
    /// Peers the swarm is believed to hold.
    pub peers_swarm: u32,
    /// Seconds the transfer is expected to take, when there is an estimate.
    pub eta_seconds: Option<u64>,
    /// Uploaded bytes against downloaded bytes, when anything has been downloaded.
    pub ratio: Option<f64>,
}

/// One row of `download_job`, with the id a page resumes below.
///
/// The whole row rather than a screen's subset, so the two readers — the service and the queue
/// screen — cannot disagree about what a job says. `engine_state_json` is deliberately not here:
/// it is the one column only the reattach path reads, one job at a time, and
/// [`crate::Database::download_engine_state`] is that read.
#[derive(Debug, Clone, PartialEq)]
pub struct DownloadJobRow {
    /// The row id, which is also the paging cursor.
    pub id: i64,
    /// The journal row this transfer is run under, when it is run under one.
    pub job_id: Option<i64>,
    /// The title the job is for.
    pub title_id: i64,
    /// The episode, for a series.
    pub episode_id: Option<i64>,
    /// The library item this download is to fill, when one is already known.
    pub media_item_id: Option<i64>,
    /// The quality profile the release was chosen against.
    pub quality_profile_id: Option<i64>,
    /// The indexer the release came from.
    pub indexer_id: Option<i64>,
    /// The lifecycle state the domain's machine last produced.
    pub state: DownloadState,
    /// Queue priority.
    pub priority: i64,
    /// Whether this job is replacing a lower-quality copy.
    pub is_upgrade: bool,
    /// The release's title.
    pub release_title: String,
    /// The source tag the release name parsed to.
    pub release_source: Option<String>,
    /// The release group.
    pub release_group: Option<String>,
    /// The resolution tag.
    pub resolution: Option<String>,
    /// The video codec tag.
    pub video_codec: Option<String>,
    /// The high-dynamic-range format.
    pub hdr_format: Option<String>,
    /// What the release name said about the audio.
    pub audio_summary: Option<String>,
    /// The size the indexer claimed.
    pub reported_size_bytes: Option<u64>,
    /// The torrent's identity, when the row holds one this build can read.
    pub info_hash: Option<InfoHash>,
    /// How the engine is told what to transfer, when the row records a source.
    pub source: Option<DownloadSource>,
    /// The protocol this job transfers over.
    pub protocol: Protocol,
    /// What the job is doing inside its transfer state.
    pub stage: DownloadStage,
    /// The NZB's SHA-256, when the source is Usenet.
    pub nzb_sha256: Option<NzbHash>,
    /// The absolute destination the guard resolved.
    pub save_path: NonEmptyText,
    /// Payload bytes received so far.
    pub downloaded_bytes: u64,
    /// Payload bytes sent back so far.
    pub uploaded_bytes: u64,
    /// Completion in permille.
    pub progress_permille: i64,
    /// Last reported download rate in bytes per second.
    pub download_rate_bps: u64,
    /// Last reported upload rate in bytes per second.
    pub upload_rate_bps: u64,
    /// Peers the last tick was connected to.
    pub peers_connected: u32,
    /// Peers the swarm was believed to hold.
    pub peers_swarm: u32,
    /// Seconds the transfer was expected to take.
    pub eta_seconds: Option<u64>,
    /// Uploaded bytes against downloaded bytes.
    pub ratio: Option<f64>,
    /// The persisted verdict from the safety gate, which is what I4 is about.
    pub safety_report: Option<SafetyReport>,
    /// The engine's stable failure code, when it failed.
    pub error_code: Option<String>,
    /// The engine's failure message, already redacted.
    pub error_message: Option<String>,
    /// Who asked for the job.
    pub added_by: AddedBy,
    /// Unix milliseconds when the job was admitted.
    pub added_at: Timestamp,
    /// Unix milliseconds when the transfer first started.
    pub started_at: Option<Timestamp>,
    /// Unix milliseconds when the payload reached the library.
    pub completed_at: Option<Timestamp>,
    /// Unix milliseconds until which the job keeps seeding.
    pub seeding_until: Option<Timestamp>,
    /// Unix milliseconds when the user removed the job.
    pub removed_at: Option<Timestamp>,
}

/// One row of `download_file`: one entry of a job's metadata, and whether it is selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadFileRow {
    /// The row id.
    pub id: i64,
    /// The job the entry belongs to.
    pub download_job_id: i64,
    /// The engine's index for the entry, which is its position in the metadata's list.
    pub file_index: i64,
    /// The relative path the metadata gave, which the inspection gate validated.
    pub path: NonEmptyText,
    /// The size the metadata claims for the entry.
    pub size_bytes: u64,
    /// Whether the engine is to write this entry.
    pub selected: bool,
    /// Whether the entry is a playable media file.
    pub is_media: bool,
}

/// Why a job's state did not change.
///
/// A refusal is not a storage failure: the domain refused an event for the state the row holds,
/// and the caller's answer is to report why rather than to retry. The two are separated here
/// because a caller that meant to move a job needs to tell "the job is not where you think" from
/// "the database is not answering".
#[derive(Debug, thiserror::Error)]
pub enum DownloadTransitionError {
    /// The domain refused the event for the state the row holds.
    #[error(transparent)]
    Rejected(#[from] TransitionRejected),
    /// The row could not be read or written.
    #[error(transparent)]
    Storage(#[from] StorageError),
}

/// Inserts one row of `download_job` and returns the row the database holds.
///
/// One transaction, because the row is read back inside it: what the caller receives is exactly
/// what was written, including the columns the database defaulted, rather than a second read
/// that a later write could have changed.
///
/// The state is a literal `queued` — `FLOWS` F6's admission — and no caller can name another.
///
/// # Errors
///
/// [`StorageError::Conflict`] when `download_job_hash_uq` or `download_job_nzb_uq` refuses a
/// second *live* job for the same source (I36). The message deliberately does not name the hash:
/// a validated hash is not emitted anywhere (SECURITY T13), and the caller already holds the
/// value it passed.
pub(crate) async fn insert(
    connection: &mut sqlx::SqliteConnection,
    job: &NewDownloadJob,
) -> Result<DownloadJobRow, StorageError> {
    let save_path = absolute_path(&job.save_path, "download_job.save_path")?;
    let (magnet, torrent_file, nzb_file) = match &job.source {
        DownloadSource::Magnet(uri) => (Some(uri.as_str()), None, None),
        DownloadSource::TorrentFile(path) => (None, Some(path.as_str()), None),
        DownloadSource::NzbFile(path) => (None, None, Some(path.as_str())),
    };
    let mut transaction = connection.begin().await.map_err(StorageError::from_sqlx)?;
    let result = sqlx::query(
        "INSERT INTO download_job (title_id, episode_id, state, priority, release_title, \
         release_source, release_group, resolution, video_codec, hdr_format, audio_summary, \
         reported_size_bytes, info_hash, magnet_uri, torrent_file_path, nzb_sha256, nzb_path, \
         protocol, save_path, added_by, added_at) VALUES (?1, ?2, 'queued', ?3, ?4, ?5, ?6, ?7, \
         ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
    )
    .bind(job.title_id)
    .bind(job.episode_id)
    .bind(job.priority)
    .bind(&job.release_title)
    .bind(job.release_source.as_deref())
    .bind(job.release_group.as_deref())
    .bind(job.resolution.as_deref())
    .bind(job.video_codec.as_deref())
    .bind(job.hdr_format.as_deref())
    .bind(job.audio_summary.as_deref())
    .bind(
        job.reported_size_bytes
            .map(|bytes| storable(bytes, "download_job.reported_size_bytes"))
            .transpose()?,
    )
    .bind(job.info_hash.map(|hash| hash.to_hex()))
    .bind(magnet)
    .bind(torrent_file)
    .bind(job.nzb_sha256.as_ref().map(NzbHash::as_str))
    .bind(nzb_file)
    .bind(job.protocol.as_str())
    .bind(save_path.as_str())
    .bind(job.added_by.as_str())
    .bind(as_millis(job.added_at))
    .execute(&mut *transaction)
    .await
    .map_err(|error| {
        if is_unique_violation(&error) {
            StorageError::Conflict {
                detail: "a live download job already holds this release".to_owned(),
            }
        } else {
            StorageError::from_sqlx(error)
        }
    })?;
    let id = result.last_insert_rowid();
    let row = read_by_id(&mut transaction, id).await?;
    transaction
        .commit()
        .await
        .map_err(StorageError::from_sqlx)?;
    Ok(row)
}

/// Applies one event to a job's state, or reports why it could not move.
///
/// The only writer of `download_job.state`. `None` means no job holds that id: nothing was there
/// to move, which is a different answer from a refusal and is what a caller that lost a race with
/// a removal needs.
///
/// # Errors
///
/// [`DownloadTransitionError::Rejected`] when the domain refuses the event for the state the row
/// holds — nothing is written in that case — and [`DownloadTransitionError::Storage`] when the
/// row could not be read or written.
pub(crate) async fn transition(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
    event: DownloadEvent,
) -> Result<Option<DownloadState>, DownloadTransitionError> {
    // Encoded before the event is consumed, so the two statements below each bind a literal and
    // a job dismissed from `blocked` keeps the report that explains the block.
    let report = report_of(&event).map(encode_report).transpose()?;
    let mut transaction = connection.begin().await.map_err(StorageError::from_sqlx)?;
    let stored = sqlx::query_as::<_, (String, String)>(
        "SELECT state, protocol FROM download_job WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(StorageError::from_sqlx)?;
    let Some((stored, protocol)) = stored else {
        transaction
            .rollback()
            .await
            .map_err(StorageError::from_sqlx)?;
        return Ok(None);
    };
    let current = DownloadState::parse(&stored).ok_or_else(|| StorageError::UnexpectedValue {
        column: "download_job.state",
        found: stored.clone(),
    })?;
    let protocol = Protocol::parse(&protocol).ok_or_else(|| StorageError::UnexpectedValue {
        column: "download_job.protocol",
        found: protocol.clone(),
    })?;
    let next = match peregrine_domain::transition_for(current, event, protocol) {
        Ok(next) => next,
        Err(rejection) => {
            // Rolled back rather than dropped, so the connection is out of the read
            // transaction before the caller sees the refusal.
            transaction
                .rollback()
                .await
                .map_err(StorageError::from_sqlx)?;
            return Err(DownloadTransitionError::Rejected(rejection));
        }
    };
    match report {
        Some(json) => {
            sqlx::query(
                "UPDATE download_job SET state = ?1, safety_report_json = ?2 WHERE id = ?3",
            )
            .bind(next.as_str())
            .bind(json)
            .bind(id)
            .execute(&mut *transaction)
            .await
            .map_err(StorageError::from_sqlx)?;
        }
        None => {
            // A separate statement rather than binding `NULL`: a job dismissed from `blocked` has
            // to keep the report that explains the block.
            sqlx::query("UPDATE download_job SET state = ?1 WHERE id = ?2")
                .bind(next.as_str())
                .bind(id)
                .execute(&mut *transaction)
                .await
                .map_err(StorageError::from_sqlx)?;
        }
    }
    transaction
        .commit()
        .await
        .map_err(StorageError::from_sqlx)?;
    Ok(Some(next))
}

/// Writes one statistics tick, reporting whether the job was there to write it to.
///
/// A tick is a level rather than an edge, so it writes no state, and the state gate is the
/// caller's own [`DownloadEvent::Progress`] transition: repeating it in SQL would be a second
/// spelling of the state machine. `false` is an ordinary answer for a tick that arrives after the
/// row was removed.
pub(crate) async fn record_statistics(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
    statistics: &DownloadStatistics,
) -> Result<bool, StorageError> {
    let result = sqlx::query(
        "UPDATE download_job SET downloaded_bytes = ?1, uploaded_bytes = ?2, \
         progress_permille = ?3, download_rate_bps = ?4, upload_rate_bps = ?5, \
         peers_connected = ?6, peers_swarm = ?7, eta_seconds = ?8, ratio = ?9 WHERE id = ?10",
    )
    .bind(storable(
        statistics.downloaded_bytes,
        "download_job.downloaded_bytes",
    )?)
    .bind(storable(
        statistics.uploaded_bytes,
        "download_job.uploaded_bytes",
    )?)
    .bind(statistics.progress_permille)
    .bind(storable(
        statistics.download_rate_bps,
        "download_job.download_rate_bps",
    )?)
    .bind(storable(
        statistics.upload_rate_bps,
        "download_job.upload_rate_bps",
    )?)
    .bind(i64::from(statistics.peers_connected))
    .bind(i64::from(statistics.peers_swarm))
    .bind(
        statistics
            .eta_seconds
            .map(|seconds| storable(seconds, "download_job.eta_seconds"))
            .transpose()?,
    )
    .bind(statistics.ratio)
    .bind(id)
    .execute(connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(result.rows_affected() > 0)
}

/// Writes a job's file rows from the metadata's list and the engine's selection.
///
/// One row per metadata entry, keyed on the entry's position, which is the index the engine's
/// selection names. Writing the same list again updates the flags rather than adding rows, which
/// is what makes a later selection change (`PG-E10-04`) an update of the same rows.
///
/// `path` is the relative path the metadata gave and the inspection gate validated. This layer
/// does not judge it: `PathGuard` owns that grammar (`AGENTS` R21), and a second one here would
/// be a second answer to what a safe entry path is. `is_media` is the domain's own extension
/// classifier, so the flag means what the scanner's means.
///
/// # Errors
///
/// [`StorageError::UnexpectedValue`] when the selection names an entry the metadata does not
/// have — the engine refuses that too (`EngineError::UnknownFile`) — or when a path or size
/// cannot be stored. A refusal writes nothing: no row of the list is applied until all of them
/// can be.
pub(crate) async fn select_files(
    connection: &mut sqlx::SqliteConnection,
    job_id: i64,
    description: &PayloadDescription,
    selection: &[usize],
) -> Result<(), StorageError> {
    let entries = description.entries();
    for index in selection {
        if *index >= entries.len() {
            return Err(StorageError::UnexpectedValue {
                column: "download_file.file_index",
                found: index.to_string(),
            });
        }
    }
    // Sorted once so the membership test is logarithmic: a season pack's list is thousands of
    // entries, and the set semantics a selection has are the caller's own (`FileSelection`).
    let mut selected: Vec<usize> = selection.to_vec();
    selected.sort_unstable();
    selected.dedup();

    // Every value is judged before the transaction opens, so a refusal cannot leave part of a
    // list behind: a path nothing could have validated, or a size the column cannot hold, is
    // reported without a statement having run.
    let mut rows = Vec::with_capacity(entries.len());
    for (position, entry) in entries.iter().enumerate() {
        let index = i64::try_from(position).map_err(|_| StorageError::UnexpectedValue {
            column: "download_file.file_index",
            found: position.to_string(),
        })?;
        rows.push((
            index,
            text(entry.path().to_owned(), "download_file.path")?,
            storable(entry.bytes(), "download_file.size_bytes")?,
            selected.binary_search(&position).is_ok(),
            is_media(entry),
        ));
    }

    let mut transaction = connection.begin().await.map_err(StorageError::from_sqlx)?;
    for (index, path, size_bytes, is_selected, media) in rows {
        sqlx::query(
            "INSERT INTO download_file (download_job_id, file_index, path, size_bytes, \
             selected, is_media) VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT (download_job_id, file_index) DO UPDATE SET \
             path = excluded.path, size_bytes = excluded.size_bytes, \
             selected = excluded.selected, is_media = excluded.is_media",
        )
        .bind(job_id)
        .bind(index)
        .bind(path.as_str())
        .bind(size_bytes)
        .bind(i64::from(is_selected))
        .bind(i64::from(media))
        .execute(&mut *transaction)
        .await
        .map_err(StorageError::from_sqlx)?;
    }
    transaction
        .commit()
        .await
        .map_err(StorageError::from_sqlx)?;
    Ok(())
}

/// Sets what a live job is doing inside its transfer state.
///
/// A stage is presentation, not a state: the statement scopes out the terminal rows so a late
/// post-processing tick cannot rewrite the stage of a job that has finished, and it never writes
/// `state`. `None` means no live job holds that id.
pub(crate) async fn set_stage(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
    stage: DownloadStage,
) -> Result<Option<DownloadStage>, StorageError> {
    let result = sqlx::query(
        "UPDATE download_job SET stage = ?1 WHERE id = ?2 \
         AND state NOT IN ('completed','failed','removed')",
    )
    .bind(stage.as_str())
    .bind(id)
    .execute(connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok((result.rows_affected() > 0).then_some(stage))
}

/// Persists the engine's own state for one job, reporting whether the job was there.
///
/// The bytes are stored as they arrived; the module doc says what that costs. An empty slice is a
/// real answer and not an absent one — the engine distinguishes "nothing private to remember"
/// from "no state has ever been persisted", and so does the read.
pub(crate) async fn record_engine_state(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
    state: &[u8],
) -> Result<bool, StorageError> {
    let result = sqlx::query("UPDATE download_job SET engine_state_json = ?1 WHERE id = ?2")
        .bind(state)
        .bind(id)
        .execute(connection)
        .await
        .map_err(StorageError::from_sqlx)?;
    Ok(result.rows_affected() > 0)
}

/// Reads one job by id, without a scope, for the write path's own read-back.
///
/// The row was written by this connection one statement earlier, so there is no scope question to
/// ask and no absent case to answer: a row that is not there is an internal invariant violation
/// and arrives as a statement failure rather than as a `None` a caller would have to re-decide.
async fn read_by_id(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
) -> Result<DownloadJobRow, StorageError> {
    let row = sqlx::query_as::<_, JobRow>(
        "SELECT id, job_id, title_id, episode_id, media_item_id, quality_profile_id, indexer_id, \
         state, priority, is_upgrade, release_title, release_source, release_group, resolution, \
         video_codec, hdr_format, audio_summary, reported_size_bytes, info_hash, magnet_uri, \
         torrent_file_path, nzb_sha256, nzb_path, protocol, stage, save_path, downloaded_bytes, \
         uploaded_bytes, progress_permille, download_rate_bps, upload_rate_bps, peers_connected, \
         peers_swarm, eta_seconds, ratio, safety_report_json, error_code, error_message, added_by, \
         added_at, started_at, completed_at, seeding_until, removed_at FROM download_job \
         WHERE id = ?1",
    )
    .bind(id)
    .fetch_one(connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    DownloadJobRow::try_from(row)
}

/// One download job as availability reads it, with the item it names when it names one.
///
/// `media_item_id` is nullable in the schema and null here for a real reason rather than a missing
/// value: acquisition is requested before the import creates the item (`PG-E14-19`), so a job can be
/// running for a title the library does not hold an item of yet. The availability derivation reads
/// the jobs an *item* carries, and the title's offer reads them all — which is why this carries the
/// attribution rather than making the caller guess it from an item it does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobEvidence {
    /// The item the job is attributed to, or `None` while no item exists for it.
    pub media_item_id: Option<i64>,
    /// The job's identity, state, and insertion instant.
    pub job: ItemJob,
}

/// Every download job one title carries, when `scope` may see the title.
///
/// Not paged, and for one title that is the honest bound: a job set is one acquisition per episode at
/// most. The caller needs the whole set — whether *any* job is live, and which is newest — rather
/// than a page of it, so a cursor here would be a lie about what half the answer can decide.
pub(crate) async fn title_jobs(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    title_id: i64,
) -> Result<Vec<JobEvidence>, StorageError> {
    let rows = sqlx::query_as::<_, JobEvidenceRow>(
        "SELECT d.id, d.media_item_id, d.state, d.added_at FROM download_job d \
         JOIN title t ON t.id = d.title_id \
         WHERE d.title_id = ?1 AND (?2 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL)) \
         ORDER BY d.id",
    )
    .bind(title_id)
    .bind(hidden_by_scope(scope))
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    rows.into_iter().map(TryInto::try_into).collect()
}

/// The columns [`title_jobs`] reads, which are the three the derivation consults.
#[derive(sqlx::FromRow)]
struct JobEvidenceRow {
    id: i64,
    media_item_id: Option<i64>,
    state: String,
    added_at: i64,
}

impl TryFrom<JobEvidenceRow> for JobEvidence {
    type Error = StorageError;

    fn try_from(row: JobEvidenceRow) -> Result<Self, Self::Error> {
        let state =
            DownloadState::parse(&row.state).ok_or_else(|| StorageError::UnexpectedValue {
                column: "download_job.state",
                found: row.state.clone(),
            })?;
        let added_at = Timestamp::from_millisecond(row.added_at).map_err(|_| {
            StorageError::UnexpectedValue {
                column: "download_job.added_at",
                found: row.added_at.to_string(),
            }
        })?;
        Ok(Self {
            media_item_id: row.media_item_id,
            job: ItemJob {
                id: row.id,
                state,
                added_at,
            },
        })
    }
}

/// Reads one job, when `scope` may see the title it is for.
///
/// A job outside the scope reads as `None`, which is the whole point of `AGENTS` R34: a
/// restricted account must not be able to tell "no such job" from "not for you".
pub(crate) async fn job(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    id: i64,
) -> Result<Option<DownloadJobRow>, StorageError> {
    let row = sqlx::query_as::<_, JobRow>(
        "SELECT d.id, d.job_id, d.title_id, d.episode_id, d.media_item_id, d.quality_profile_id, \
         d.indexer_id, d.state, d.priority, d.is_upgrade, d.release_title, d.release_source, \
         d.release_group, d.resolution, d.video_codec, d.hdr_format, d.audio_summary, \
         d.reported_size_bytes, d.info_hash, d.magnet_uri, d.torrent_file_path, d.nzb_sha256, d.nzb_path, d.protocol, d.stage, d.save_path, \
         d.downloaded_bytes, d.uploaded_bytes, d.progress_permille, d.download_rate_bps, \
         d.upload_rate_bps, d.peers_connected, d.peers_swarm, d.eta_seconds, d.ratio, \
         d.safety_report_json, d.error_code, d.error_message, d.added_by, d.added_at, \
         d.started_at, d.completed_at, d.seeding_until, d.removed_at FROM download_job d \
         JOIN title t ON t.id = d.title_id \
         WHERE d.id = ?1 AND (?2 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL))",
    )
    .bind(id)
    .bind(hidden_by_scope(scope))
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    row.map(TryInto::try_into).transpose()
}

/// Reads a page of jobs in `states`, newest first, when `scope` may see their titles.
///
/// The state set is a filter rather than an ordering: the page is ordered by `id DESC`, so the
/// cursor in `request` is the id of the previous page's last row, and a job created between two
/// page reads cannot shift the second one.
pub(crate) async fn list_jobs(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    states: &[DownloadState],
    request: PageRequest,
) -> Result<Vec<DownloadJobRow>, StorageError> {
    let states = encode_states(states);
    let rows = sqlx::query_as::<_, JobRow>(
        "SELECT d.id, d.job_id, d.title_id, d.episode_id, d.media_item_id, d.quality_profile_id, \
         d.indexer_id, d.state, d.priority, d.is_upgrade, d.release_title, d.release_source, \
         d.release_group, d.resolution, d.video_codec, d.hdr_format, d.audio_summary, \
         d.reported_size_bytes, d.info_hash, d.magnet_uri, d.torrent_file_path, d.nzb_sha256, d.nzb_path, d.protocol, d.stage, d.save_path, \
         d.downloaded_bytes, d.uploaded_bytes, d.progress_permille, d.download_rate_bps, \
         d.upload_rate_bps, d.peers_connected, d.peers_swarm, d.eta_seconds, d.ratio, \
         d.safety_report_json, d.error_code, d.error_message, d.added_by, d.added_at, \
         d.started_at, d.completed_at, d.seeding_until, d.removed_at FROM download_job d \
         JOIN title t ON t.id = d.title_id \
         WHERE (?1 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL)) \
           AND d.state IN (SELECT value FROM json_each(?2)) \
           AND (?3 IS NULL OR d.id < ?3) ORDER BY d.id DESC LIMIT ?4",
    )
    .bind(hidden_by_scope(scope))
    .bind(states)
    .bind(request.bindable_cursor())
    .bind(request.bindable_limit())
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    rows.into_iter().map(TryInto::try_into).collect()
}

/// Reads one job's file rows, in the metadata's own order, when `scope` may see its title.
///
/// Not paged, and the module doc says why: the bound is one torrent's metadata list, not the
/// table's growth. The order is `file_index` rather than the row id, because that is the
/// order the engine and the metadata use, and a caller selecting a file for playback needs the
/// index rather than the newest row.
pub(crate) async fn files(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    job_id: i64,
) -> Result<Vec<DownloadFileRow>, StorageError> {
    let rows = sqlx::query_as::<_, FileRow>(
        "SELECT f.id, f.download_job_id, f.file_index, f.path, f.size_bytes, f.selected, \
         f.is_media FROM download_file f \
         JOIN download_job d ON d.id = f.download_job_id \
         JOIN title t ON t.id = d.title_id \
         WHERE f.download_job_id = ?1 \
           AND (?2 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL)) \
         ORDER BY f.file_index",
    )
    .bind(job_id)
    .bind(hidden_by_scope(scope))
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    rows.into_iter().map(TryInto::try_into).collect()
}

/// Reads the engine's persisted state for one job, when `scope` may see its title.
///
/// `None` covers both "the row is not there or not visible" and "no state has ever been
/// persisted", which read the same to the reattach path: there is nothing to hand back.
pub(crate) async fn engine_state(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    id: i64,
) -> Result<Option<Vec<u8>>, StorageError> {
    let stored = sqlx::query_scalar::<_, Option<Vec<u8>>>(
        "SELECT d.engine_state_json FROM download_job d JOIN title t ON t.id = d.title_id \
         WHERE d.id = ?1 AND (?2 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL))",
    )
    .bind(id)
    .bind(hidden_by_scope(scope))
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(stored.flatten())
}

/// Deletes one finished `download_job` row; its file rows cascade.
///
/// The finished states are scoped inside the statement rather than read first, so an id that raced
/// a state change cannot delete a live transfer. Answers whether a row went.
///
/// `completed` is finished: the payload is whole and the app has stopped seeding it, so the row is
/// history rather than a transfer — clearing it is not how anything is stopped.
pub(crate) async fn delete_finished(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
) -> Result<bool, StorageError> {
    let result = sqlx::query(
        "DELETE FROM download_job WHERE id = ?1 AND state IN ('completed', 'failed', 'removed')",
    )
    .bind(id)
    .execute(connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(result.rows_affected() > 0)
}

/// Deletes every finished `download_job` row, answering how many went.
///
/// Files on disk are untouched: this removes the queue entry, not the bytes.
pub(crate) async fn delete_all_finished(
    connection: &mut sqlx::SqliteConnection,
) -> Result<u64, StorageError> {
    let result =
        sqlx::query("DELETE FROM download_job WHERE state IN ('completed', 'failed', 'removed')")
            .execute(connection)
            .await
            .map_err(StorageError::from_sqlx)?;
    Ok(result.rows_affected())
}

/// The columns [`DownloadJobRow`] is built from.
#[derive(sqlx::FromRow)]
struct JobRow {
    id: i64,
    job_id: Option<i64>,
    title_id: i64,
    episode_id: Option<i64>,
    media_item_id: Option<i64>,
    quality_profile_id: Option<i64>,
    indexer_id: Option<i64>,
    state: String,
    priority: i64,
    is_upgrade: i64,
    release_title: String,
    release_source: Option<String>,
    release_group: Option<String>,
    resolution: Option<String>,
    video_codec: Option<String>,
    hdr_format: Option<String>,
    audio_summary: Option<String>,
    reported_size_bytes: Option<i64>,
    info_hash: Option<String>,
    magnet_uri: Option<String>,
    torrent_file_path: Option<String>,
    nzb_sha256: Option<String>,
    nzb_path: Option<String>,
    protocol: String,
    stage: String,
    save_path: String,
    downloaded_bytes: i64,
    uploaded_bytes: i64,
    progress_permille: i64,
    download_rate_bps: i64,
    upload_rate_bps: i64,
    peers_connected: i64,
    peers_swarm: i64,
    eta_seconds: Option<i64>,
    ratio: Option<f64>,
    safety_report_json: Option<String>,
    error_code: Option<String>,
    error_message: Option<String>,
    added_by: String,
    added_at: i64,
    started_at: Option<i64>,
    completed_at: Option<i64>,
    seeding_until: Option<i64>,
    removed_at: Option<i64>,
}

fn parse_nzb_sha256(found: Option<String>) -> Result<Option<NzbHash>, StorageError> {
    found
        .map(|value| {
            NzbHash::parse(&value).ok_or_else(|| StorageError::UnexpectedValue {
                column: "download_job.nzb_sha256",
                found: value,
            })
        })
        .transpose()
}

fn source_from(
    magnet: Option<String>,
    torrent: Option<String>,
    nzb: Option<String>,
) -> Result<Option<DownloadSource>, StorageError> {
    Ok(match (magnet, torrent, nzb) {
        (Some(uri), _, _) => Some(DownloadSource::Magnet(MagnetUri(text(
            uri,
            "download_job.magnet_uri",
        )?))),
        (None, Some(path), _) => Some(DownloadSource::TorrentFile(text(
            path,
            "download_job.torrent_file_path",
        )?)),
        (None, None, Some(path)) => Some(DownloadSource::NzbFile(text(
            path,
            "download_job.nzb_path",
        )?)),
        (None, None, None) => None,
    })
}

impl TryFrom<JobRow> for DownloadJobRow {
    type Error = StorageError;

    fn try_from(row: JobRow) -> Result<Self, Self::Error> {
        let state =
            DownloadState::parse(&row.state).ok_or_else(|| StorageError::UnexpectedValue {
                column: "download_job.state",
                found: row.state.clone(),
            })?;
        let added_by =
            AddedBy::parse(&row.added_by).ok_or_else(|| StorageError::UnexpectedValue {
                column: "download_job.added_by",
                found: row.added_by.clone(),
            })?;
        let protocol =
            Protocol::parse(&row.protocol).ok_or_else(|| StorageError::UnexpectedValue {
                column: "download_job.protocol",
                found: row.protocol.clone(),
            })?;
        let download_stage =
            DownloadStage::parse(&row.stage).ok_or_else(|| StorageError::UnexpectedValue {
                column: "download_job.stage",
                found: row.stage.clone(),
            })?;
        let nzb_sha256 = parse_nzb_sha256(row.nzb_sha256)?;
        // `from_hex` accepts either case, so a row another build wrote in upper case still reads;
        // what this build writes is the lowercase form the unique index compares.
        let info_hash = row
            .info_hash
            .map(|found| {
                InfoHash::from_hex(&found).map_err(|_| StorageError::UnexpectedValue {
                    column: "download_job.info_hash",
                    found,
                })
            })
            .transpose()?;
        let source = source_from(row.magnet_uri, row.torrent_file_path, row.nzb_path)?;
        let safety_report = row
            .safety_report_json
            .map(|json| decode_report(&json))
            .transpose()?;
        Ok(Self {
            id: row.id,
            job_id: row.job_id,
            title_id: row.title_id,
            episode_id: row.episode_id,
            media_item_id: row.media_item_id,
            quality_profile_id: row.quality_profile_id,
            indexer_id: row.indexer_id,
            state,
            priority: row.priority,
            is_upgrade: row.is_upgrade != 0,
            release_title: row.release_title,
            release_source: row.release_source,
            release_group: row.release_group,
            resolution: row.resolution,
            video_codec: row.video_codec,
            hdr_format: row.hdr_format,
            audio_summary: row.audio_summary,
            reported_size_bytes: row
                .reported_size_bytes
                .map(|bytes| unsigned(bytes, "download_job.reported_size_bytes"))
                .transpose()?,
            info_hash,
            source,
            protocol,
            stage: download_stage,
            nzb_sha256,
            save_path: absolute_path(
                &text(row.save_path, "download_job.save_path")?,
                "download_job.save_path",
            )?,
            downloaded_bytes: unsigned(row.downloaded_bytes, "download_job.downloaded_bytes")?,
            uploaded_bytes: unsigned(row.uploaded_bytes, "download_job.uploaded_bytes")?,
            progress_permille: row.progress_permille,
            download_rate_bps: unsigned(row.download_rate_bps, "download_job.download_rate_bps")?,
            upload_rate_bps: unsigned(row.upload_rate_bps, "download_job.upload_rate_bps")?,
            peers_connected: bounded(row.peers_connected, "download_job.peers_connected")?,
            peers_swarm: bounded(row.peers_swarm, "download_job.peers_swarm")?,
            eta_seconds: row
                .eta_seconds
                .map(|seconds| unsigned(seconds, "download_job.eta_seconds"))
                .transpose()?,
            ratio: row.ratio,
            safety_report,
            error_code: row.error_code,
            error_message: row.error_message,
            added_by,
            added_at: millis(row.added_at),
            started_at: row.started_at.map(millis),
            completed_at: row.completed_at.map(millis),
            seeding_until: row.seeding_until.map(millis),
            removed_at: row.removed_at.map(millis),
        })
    }
}

/// The columns [`DownloadFileRow`] is built from.
#[derive(sqlx::FromRow)]
struct FileRow {
    id: i64,
    download_job_id: i64,
    file_index: i64,
    path: String,
    size_bytes: i64,
    selected: i64,
    is_media: i64,
}

impl TryFrom<FileRow> for DownloadFileRow {
    type Error = StorageError;

    fn try_from(row: FileRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            download_job_id: row.download_job_id,
            file_index: row.file_index,
            path: text(row.path, "download_file.path")?,
            size_bytes: unsigned(row.size_bytes, "download_file.size_bytes")?,
            selected: row.selected != 0,
            is_media: row.is_media != 0,
        })
    }
}

/// The safety report an event carries, if it carries one.
///
/// Only the two gate events do, and the domain's own module doc says why: they are the only
/// spellings that can cross the gate.
fn report_of(event: &DownloadEvent) -> Option<&SafetyReport> {
    match event {
        DownloadEvent::Inspected(report) | DownloadEvent::Consented(report) => Some(report),
        _ => None,
    }
}

/// The report's JSON, as `download_job.safety_report_json` stores it.
///
/// Serializing this type cannot fail in practice — an enum, a list of the same, and two integers
/// — but the failure is mapped rather than expected away: it would happen inside the writer task,
/// where a panic takes every later write with it.
fn encode_report(report: &SafetyReport) -> Result<String, StorageError> {
    serde_json::to_string(report).map_err(|error| StorageError::InvalidJsonField {
        column: "download_job.safety_report_json",
        detail: error.to_string(),
    })
}

/// The report a stored column holds, or a report of what it holds instead.
fn decode_report(json: &str) -> Result<SafetyReport, StorageError> {
    serde_json::from_str(json).map_err(|error| StorageError::InvalidJsonField {
        column: "download_job.safety_report_json",
        detail: error.to_string(),
    })
}

/// The state set as the JSON array the filter compares against.
///
/// The crate's one array encoder rather than a join: a spelling that held a delimiter could not
/// split into two states, and this is the same shape `media_file.audio_languages` is written in.
fn encode_states(states: &[DownloadState]) -> String {
    let spelled: Vec<String> = states
        .iter()
        .map(|state| state.as_str().to_owned())
        .collect();
    encode_string_array(&spelled)
}

/// Whether an entry is a playable media file.
///
/// The domain's own extension classifier, so the flag means what the scanner's means. A link is
/// never media: the gate refuses one, and an entry nothing will be written from is not a file the
/// library could import.
fn is_media(entry: &PayloadEntry) -> bool {
    if entry.kind() != EntryKind::File {
        return false;
    }
    Path::new(entry.path())
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(is_media_extension)
}

/// Whether a driver error is a unique index refusing a duplicate.
///
/// `SQLite` reports `SQLITE_CONSTRAINT_UNIQUE`, which `sqlx` maps to
/// [`sqlx::error::ErrorKind::UniqueViolation`].
fn is_unique_violation(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(database) if database.is_unique_violation())
}

/// A byte count as the `i64` an `INTEGER` column holds, or a refusal.
///
/// A value above `i64::MAX` is not a download this application can meet — that is 8 EiB — and
/// clamping is the wrong answer for a claim: it would store a number nobody reported and make a
/// corrupt release look like a plausible one. `library.rs` saturates the same conversion for a
/// size the scanner measured, where refusing would fail a scan over one bad stat; here the value
/// is the caller's input, and a caller can be told.
fn storable(value: u64, column: &'static str) -> Result<i64, StorageError> {
    i64::try_from(value).map_err(|_| StorageError::UnexpectedValue {
        column,
        found: value.to_string(),
    })
}

/// An `INTEGER` column as the unsigned count it holds, or a refusal.
///
/// A negative one is a row this build cannot read rather than a value to fold to zero: folding
/// would turn a corrupt size into a plausible one, and the schema constrains none of these
/// columns to be positive.
fn unsigned(value: i64, column: &'static str) -> Result<u64, StorageError> {
    u64::try_from(value).map_err(|_| StorageError::UnexpectedValue {
        column,
        found: value.to_string(),
    })
}

/// An `INTEGER` column as the narrower type the row carries, or a refusal.
fn bounded<T>(value: i64, column: &'static str) -> Result<T, StorageError>
where
    T: TryFrom<i64>,
{
    T::try_from(value).map_err(|_| StorageError::UnexpectedValue {
        column,
        found: value.to_string(),
    })
}

/// A `TEXT` column as the required text it holds, or a refusal.
fn text(value: String, column: &'static str) -> Result<NonEmptyText, StorageError> {
    NonEmptyText::new(&value).map_err(|_| StorageError::UnexpectedValue {
        column,
        found: value,
    })
}

/// The destination as the absolute path every row this build writes holds, or a refusal.
///
/// This is not a second path grammar: containment, traversal, and reserved names are
/// [`peregrine_domain::PathGuard`]'s, and the caller here has already been through it. What is
/// checked is the one property the column needs — a destination that is absolute — because a
/// relative one would be resolved against whatever directory the process happened to be in.
fn absolute_path(value: &NonEmptyText, column: &'static str) -> Result<NonEmptyText, StorageError> {
    if Path::new(value.as_str()).is_absolute() {
        return Ok(value.clone());
    }
    Err(StorageError::UnexpectedValue {
        column,
        found: value.as_str().to_owned(),
    })
}

/// A `TIMESTAMP` column as the `i64` milliseconds it is stored as.
fn as_millis(value: Timestamp) -> i64 {
    value.as_millisecond()
}

/// The inverse of [`as_millis`], for a row this build is reading.
///
/// `SQLite` stores what it is given, so a column can hold a millisecond outside `jiff`'s range;
/// the clamp keeps the row readable and the mapper's other checks still apply. The common case is
/// an exact value, which is the one the round-trip test asserts.
fn millis(value: i64) -> Timestamp {
    Timestamp::from_millisecond(value).unwrap_or(Timestamp::UNIX_EPOCH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Database;
    use crate::database::open_writer_connection;
    use crate::page::Cursor;
    use crate::test_support::{NOW, account, database, dump, episode, path_of, season, title};
    use peregrine_domain::AccountId;
    use peregrine_domain::inspect::{InspectionLimits, OptIn, PayloadInspector};
    use peregrine_domain::path::PathGuard;
    use proptest::prelude::*;
    use tempfile::TempDir;

    /// The scope the fixtures are read with: an account with no ceiling and no curation.
    fn any_account() -> AccountScope {
        AccountScope::unrestricted(AccountId::new(1).expect("account one exists"))
    }

    fn restricted() -> AccountScope {
        AccountScope::restricted(AccountId::new(2).expect("account two exists"))
    }

    fn text(value: &str) -> NonEmptyText {
        NonEmptyText::new(value).expect("fixture text is not empty")
    }

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millisecond(millis).expect("a millisecond inside the range")
    }

    /// A hash no other fixture holds, spelled from one byte.
    fn hash(byte: u8) -> InfoHash {
        InfoHash::from_bytes([byte; 20])
    }

    /// A catalogue with a certified title, an adult one, and one nothing has rated.
    async fn catalogue(directory: &TempDir) -> (Database, i64, i64, i64) {
        let path = path_of(directory);
        let database = database(directory).await;
        account(&path, 1, "Adult").await;
        account(&path, 2, "Child").await;
        title(&path, 1, "Alien", 0, Some("R")).await;
        title(&path, 2, "Adult Film", 1, Some("X")).await;
        title(&path, 3, "Unrated Film", 0, None).await;
        (database, 1, 2, 3)
    }

    /// An absolute destination, which every row this build writes holds.
    fn save_path(directory: &TempDir) -> NonEmptyText {
        let path = directory.path().join("downloads");
        text(path.to_str().expect("a temporary path is UTF-8"))
    }

    /// A job the way the service admits one.
    fn new_job(directory: &TempDir, title_id: i64) -> NewDownloadJob {
        NewDownloadJob {
            title_id,
            episode_id: None,
            release_title: "Alien.1979.1080p.BluRay.x264-GROUP".to_owned(),
            release_source: Some("BluRay".to_owned()),
            release_group: Some("GROUP".to_owned()),
            resolution: Some("1080p".to_owned()),
            video_codec: Some("h264".to_owned()),
            hdr_format: None,
            audio_summary: Some("DTS 5.1".to_owned()),
            reported_size_bytes: Some(8_000_000_000),
            info_hash: Some(hash(1)),
            protocol: Protocol::Torrent,
            nzb_sha256: None,
            source: DownloadSource::Magnet(MagnetUri::new(text(
                "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567",
            ))),
            save_path: save_path(directory),
            priority: 100,
            added_by: AddedBy::User,
            added_at: at(NOW),
        }
    }

    /// A Usenet job the way the service will admit one: an NZB hash, an NZB path, and no
    /// info hash.
    fn new_nzb_job(directory: &TempDir, title_id: i64, sha256: &str) -> NewDownloadJob {
        let mut job = new_job(directory, title_id);
        job.info_hash = None;
        job.protocol = Protocol::Usenet;
        job.nzb_sha256 = NzbHash::parse(sha256);
        job.source = DownloadSource::NzbFile(text("/data/nzb/0123456789abcdef.nzb"));
        job
    }

    /// The root the gate judges fixture paths against.
    ///
    /// Built from the temporary directory rather than a literal, so the fixture root is absolute
    /// on every platform this suite runs on.
    fn guard(directory: &TempDir) -> PathGuard {
        PathGuard::new(directory.path().join("downloads")).expect("a temporary path is absolute")
    }

    /// Runs the real inspector over one entry, so every report is one the gate could produce.
    fn report(directory: &TempDir, path: &str, opt_in: OptIn) -> SafetyReport {
        let description =
            PayloadDescription::new(vec![PayloadEntry::new(path, 1, EntryKind::File)]);
        PayloadInspector::new(InspectionLimits::new(1_000_000, u64::MAX)).inspect(
            &description,
            &guard(directory),
            opt_in,
        )
    }

    /// A torrent the gate allows outright.
    fn allowed(directory: &TempDir) -> SafetyReport {
        report(directory, "Movie.2024.1080p.mkv", OptIn::NotGranted)
    }

    /// A nested archive: refused until the user takes the advanced opt-in.
    fn opt_in_required(directory: &TempDir) -> SafetyReport {
        report(directory, "extras.zip", OptIn::NotGranted)
    }

    fn ids_of(rows: &[DownloadJobRow]) -> Vec<i64> {
        rows.iter().map(|row| row.id).collect()
    }

    /// Admits one job and returns its id.
    async fn admit(database: &Database, directory: &TempDir, title_id: i64) -> i64 {
        database
            .create_download_job(new_job(directory, title_id))
            .await
            .expect("the job is admitted")
            .id
    }

    /// Admits one job that holds `byte`'s hash, and moves it into `inspecting`.
    async fn inspecting(database: &Database, directory: &TempDir, byte: u8) -> i64 {
        let mut job = new_job(directory, 1);
        job.info_hash = Some(hash(byte));
        let id = database
            .create_download_job(job)
            .await
            .expect("the job is admitted")
            .id;
        assert_eq!(
            database
                .transition_download_job(id, DownloadEvent::Admitted)
                .await
                .expect("the event is legal from `queued`"),
            Some(DownloadState::Inspecting)
        );
        id
    }

    // ---- the enums the schema constrains -----------------------------------------

    /// The two spellings are the database's `CHECK` constraint, so the enum and the schema cannot
    /// drift apart without this failing.
    #[test]
    fn every_added_by_spelling_matches_the_schema_constraint() {
        assert_eq!(
            AddedBy::ALL
                .iter()
                .map(|added| added.as_str())
                .collect::<Vec<_>>(),
            ["user", "auto"]
        );
        for added in AddedBy::ALL {
            assert_eq!(AddedBy::parse(added.as_str()), Some(*added));
        }
        assert_eq!(AddedBy::parse("robot"), None);
        assert_eq!(AddedBy::parse(""), None);
    }

    /// SECURITY T13: a magnet carries the info-hash and a tracker list, so it never reaches a
    /// rendering of the value it is stored in.
    #[test]
    fn a_magnet_never_reaches_a_debug_rendering() {
        let directory = TempDir::new().unwrap();
        let uri = MagnetUri::new(text(
            "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&tr=http://tracker",
        ));
        assert_eq!(format!("{uri:?}"), "MagnetUri(..)");
        assert!(uri.as_str().starts_with("magnet:"));

        let job = new_job(&directory, 1);
        let debug = format!("{job:?}");
        assert!(!debug.contains("magnet:"), "{debug}");
        assert!(!debug.contains(&hash(1).to_hex()), "{debug}");
        assert!(debug.contains("MagnetUri(..)"), "{debug}");
    }

    // ---- I36: one live job per info hash -----------------------------------------

    /// I36 at the write that enforces it: the second live job is refused with an error a caller
    /// can match, and the first row is untouched.
    #[tokio::test]
    async fn a_second_live_job_for_one_info_hash_is_a_typed_conflict() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _, _) = catalogue(&directory).await;
        let first = database
            .create_download_job(new_job(&directory, certified))
            .await
            .expect("the first job is admitted");
        assert_eq!(
            first.state,
            DownloadState::Queued,
            "F6 admits a job in `queued`"
        );

        let error = database
            .create_download_job(new_job(&directory, certified))
            .await
            .expect_err("a live job already holds this hash");
        assert!(
            matches!(error, StorageError::Conflict { .. }),
            "the violation must reach the caller as a matchable conflict: {error:?}"
        );
        assert!(
            !error.to_string().contains(&hash(1).to_hex()),
            "SECURITY T13: the conflict must not name the info hash: {error}"
        );
        assert_eq!(
            database
                .list_download_jobs(any_account(), DownloadState::ALL, PageRequest::first(10))
                .await
                .unwrap()
                .len(),
            1,
            "the refused insert wrote nothing"
        );
    }

    #[tokio::test]
    async fn a_usenet_job_round_trips_its_protocol_and_nzb_source() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _, _) = catalogue(&directory).await;
        let sha256 = "a".repeat(64);
        let stored = database
            .create_download_job(new_nzb_job(&directory, certified, &sha256))
            .await
            .expect("a Usenet job is admitted");

        assert_eq!(stored.protocol, Protocol::Usenet);
        assert_eq!(stored.stage, DownloadStage::Transfer);
        assert_eq!(
            stored.nzb_sha256.as_ref().map(NzbHash::as_str),
            Some(sha256.as_str())
        );
        assert!(matches!(stored.source, Some(DownloadSource::NzbFile(_))));
    }

    #[tokio::test]
    async fn a_live_usenet_job_holds_its_nzb_hash_until_it_is_terminal() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _, _) = catalogue(&directory).await;
        let sha256 = "b".repeat(64);
        let first = database
            .create_download_job(new_nzb_job(&directory, certified, &sha256))
            .await
            .expect("the first job is admitted");

        let error = database
            .create_download_job(new_nzb_job(&directory, certified, &sha256))
            .await
            .expect_err("a live job already holds this post");
        assert!(
            matches!(error, StorageError::Conflict { .. }),
            "the NZB index must reach the caller as a matchable conflict: {error:?}"
        );

        database
            .transition_download_job(first.id, DownloadEvent::Removed)
            .await
            .expect("a queued job can be removed");
        database
            .create_download_job(new_nzb_job(&directory, certified, &sha256))
            .await
            .expect("a terminal job leaves the live set");
    }

    #[tokio::test]
    async fn a_stage_is_written_only_while_a_job_is_live() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _, _) = catalogue(&directory).await;
        let job = database
            .create_download_job(new_job(&directory, certified))
            .await
            .expect("a job is admitted");

        assert_eq!(
            database
                .set_download_stage(job.id, DownloadStage::Repairing)
                .await
                .unwrap(),
            Some(DownloadStage::Repairing)
        );
        assert_eq!(
            database
                .download_job(any_account(), job.id)
                .await
                .unwrap()
                .expect("the job is there")
                .stage,
            DownloadStage::Repairing
        );

        database
            .transition_download_job(job.id, DownloadEvent::Removed)
            .await
            .expect("a queued job can be removed");
        assert_eq!(
            database
                .set_download_stage(job.id, DownloadStage::Extracting)
                .await
                .unwrap(),
            None,
            "a terminal row keeps the stage it finished at"
        );
    }

    /// SM1: a retry after a removal is a new job, because a terminal job is history rather than a
    /// live one.
    #[tokio::test]
    async fn a_removed_job_leaves_its_hash_free_for_a_new_one() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _, _) = catalogue(&directory).await;
        let first = database
            .create_download_job(new_job(&directory, certified))
            .await
            .unwrap();
        assert_eq!(
            database
                .transition_download_job(first.id, DownloadEvent::Removed)
                .await
                .unwrap(),
            Some(DownloadState::Removed)
        );

        let second = database
            .create_download_job(new_job(&directory, certified))
            .await
            .expect("a removed job does not hold its hash");
        assert_ne!(second.id, first.id, "the retry is its own row");
    }

    /// SM1: a failure is absorbing for the job and vacates the hash for the retry, which walks
    /// the gate again rather than reusing the failed row.
    #[tokio::test]
    async fn a_failed_job_leaves_its_hash_free_for_a_retry() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _, _) = catalogue(&directory).await;
        let id = inspecting(&database, &directory, 4).await;
        assert_eq!(
            database
                .transition_download_job(id, DownloadEvent::Inspected(allowed(&directory)))
                .await
                .unwrap(),
            Some(DownloadState::Metadata)
        );
        assert_eq!(
            database
                .transition_download_job(id, DownloadEvent::MetadataTimedOut)
                .await
                .unwrap(),
            Some(DownloadState::Failed)
        );

        let mut retry = new_job(&directory, certified);
        retry.info_hash = Some(hash(4));
        database
            .create_download_job(retry)
            .await
            .expect("a failed job does not hold its hash");
    }

    /// I36's live set is the partial index's: `blocked` is waiting for a user rather than holding
    /// a torrent in the engine, so it is not live and a new job may hold the same hash.
    #[tokio::test]
    async fn a_blocked_job_is_not_live_so_a_new_job_may_hold_its_hash() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _, _) = catalogue(&directory).await;
        let id = inspecting(&database, &directory, 5).await;
        assert_eq!(
            database
                .transition_download_job(id, DownloadEvent::Inspected(opt_in_required(&directory)))
                .await
                .unwrap(),
            Some(DownloadState::Blocked)
        );

        let mut second = new_job(&directory, certified);
        second.info_hash = Some(hash(5));
        database
            .create_download_job(second)
            .await
            .expect("`blocked` is not in `download_job_hash_uq`'s live set");
    }

    // ---- the transition is the only writer of `state` ----------------------------

    /// I4: the state and the report that authorized it arrive together, and a report-carrying
    /// event that is refused writes neither — so there is no window in which a job stands past
    /// the gate without its report, which is what a crash between two statements would leave.
    #[tokio::test]
    async fn reaching_metadata_writes_the_report_in_the_same_transaction() {
        let directory = TempDir::new().unwrap();
        let (database, _, _, _) = catalogue(&directory).await;
        let path = path_of(&directory);
        let id = inspecting(&database, &directory, 6).await;

        let before = dump(&path, "download_job").await;
        let error = database
            .transition_download_job(id, DownloadEvent::EngineAccepted)
            .await
            .expect_err("the engine cannot accept a job the gate has not passed");
        assert!(
            matches!(
                error,
                DownloadTransitionError::Rejected(TransitionRejected::OutOfOrder { .. })
            ),
            "{error:?}"
        );
        assert_eq!(
            dump(&path, "download_job").await,
            before,
            "a refused transition writes nothing at all"
        );

        let report = allowed(&directory);
        assert_eq!(
            database
                .transition_download_job(id, DownloadEvent::Inspected(report.clone()))
                .await
                .unwrap(),
            Some(DownloadState::Metadata)
        );
        let stored = database
            .download_job(any_account(), id)
            .await
            .unwrap()
            .expect("the row is there");
        assert_eq!(stored.state, DownloadState::Metadata);
        assert_eq!(
            stored.safety_report.as_ref(),
            Some(&report),
            "I4: the state that crossed the gate and the report that let it are one write"
        );

        // A later, refused report-carrying event must not overwrite the stored one.
        let other = opt_in_required(&directory);
        let refused = database
            .transition_download_job(id, DownloadEvent::Inspected(other))
            .await
            .expect_err("a second inspection cannot re-answer a job past the gate");
        assert!(
            matches!(
                refused,
                DownloadTransitionError::Rejected(TransitionRejected::OutOfOrder { .. })
            ),
            "{refused:?}"
        );
        assert_eq!(
            database
                .download_job(any_account(), id)
                .await
                .unwrap()
                .unwrap()
                .safety_report,
            Some(report),
            "the report that was persisted is still the one that crossed the gate"
        );
    }

    /// I4 as the negative, over every event that carries no report: none of them moves a job out
    /// of `queued`, `inspecting`, or `removed`, so `metadata` is unreachable without a report.
    #[tokio::test]
    async fn a_job_that_never_received_a_report_never_leaves_the_gate() {
        let directory = TempDir::new().unwrap();
        let (database, _, _, _) = catalogue(&directory).await;

        for (index, event) in [
            DownloadEvent::Admitted,
            DownloadEvent::EngineAccepted,
            DownloadEvent::MetadataTimedOut,
            DownloadEvent::Paused,
            DownloadEvent::Resumed,
            DownloadEvent::PayloadComplete,
            DownloadEvent::SeedingSatisfied,
            DownloadEvent::EngineFailed,
            DownloadEvent::DiskFull,
            DownloadEvent::Progress,
            DownloadEvent::Removed,
        ]
        .into_iter()
        .enumerate()
        {
            let id = inspecting(&database, &directory, 20 + index as u8).await;
            // The name is taken before the event is consumed, for the failure message.
            let name = event.to_string();
            // The first `Admitted` has already moved the job into `inspecting`; a second one is a
            // refusal, which is the answer this test wants for that event too.
            let _ = database.transition_download_job(id, event).await;
            let row = database
                .download_job(any_account(), id)
                .await
                .unwrap()
                .expect("the row is there");
            assert!(
                matches!(
                    row.state,
                    DownloadState::Queued | DownloadState::Inspecting | DownloadState::Removed
                ),
                "I4: `{name}` reached {:?} with no report",
                row.state
            );
            assert_eq!(
                row.safety_report, None,
                "I4: no event without a report may persist one"
            );
        }
    }

    /// F6: the report is persisted with the job so a block is still explainable after a restart,
    /// and dismissing the block does not take the explanation with it.
    #[tokio::test]
    async fn a_blocked_job_keeps_the_report_that_blocked_it() {
        let directory = TempDir::new().unwrap();
        let (database, _, _, _) = catalogue(&directory).await;
        let id = inspecting(&database, &directory, 7).await;
        let report = opt_in_required(&directory);

        assert_eq!(
            database
                .transition_download_job(id, DownloadEvent::Inspected(report.clone()))
                .await
                .unwrap(),
            Some(DownloadState::Blocked)
        );
        let blocked = database
            .download_job(any_account(), id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(blocked.state, DownloadState::Blocked);
        assert_eq!(blocked.safety_report, Some(report.clone()));

        assert_eq!(
            database
                .transition_download_job(id, DownloadEvent::Removed)
                .await
                .unwrap(),
            Some(DownloadState::Removed)
        );
        assert_eq!(
            database
                .download_job(any_account(), id)
                .await
                .unwrap()
                .unwrap()
                .safety_report,
            Some(report),
            "a dismissal is not a reason to forget why the job was blocked"
        );
    }

    /// A refusal is the domain's answer rather than a storage failure, and a duplicate or
    /// out-of-order event leaves the row byte-identical.
    #[tokio::test]
    async fn a_refused_transition_leaves_the_row_byte_identical() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _, _) = catalogue(&directory).await;
        let path = path_of(&directory);
        let id = database
            .create_download_job(new_job(&directory, certified))
            .await
            .unwrap()
            .id;

        let before = dump(&path, "download_job").await;
        let error = database
            .transition_download_job(id, DownloadEvent::Paused)
            .await
            .expect_err("`paused` is legal later in the machine and out of order here");
        assert!(
            matches!(
                error,
                DownloadTransitionError::Rejected(TransitionRejected::OutOfOrder {
                    state: DownloadState::Queued,
                    event: "paused",
                })
            ),
            "{error:?}"
        );
        assert_eq!(dump(&path, "download_job").await, before);

        database
            .transition_download_job(id, DownloadEvent::Admitted)
            .await
            .unwrap();
        let admitted = dump(&path, "download_job").await;
        let duplicate = database
            .transition_download_job(id, DownloadEvent::Admitted)
            .await
            .expect_err("a second admission is a duplicate");
        assert!(
            matches!(
                duplicate,
                DownloadTransitionError::Rejected(TransitionRejected::OutOfOrder { .. })
            ),
            "{duplicate:?}"
        );
        assert_eq!(dump(&path, "download_job").await, admitted);
    }

    /// A terminal state absorbs: a completion that arrives after a removal must not resurrect the
    /// job, and the refusal names the terminal rule rather than the order.
    #[tokio::test]
    async fn a_late_event_cannot_resurrect_a_removed_job() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _, _) = catalogue(&directory).await;
        let path = path_of(&directory);
        let id = database
            .create_download_job(new_job(&directory, certified))
            .await
            .unwrap()
            .id;
        database
            .transition_download_job(id, DownloadEvent::Removed)
            .await
            .unwrap();
        let removed = dump(&path, "download_job").await;

        for late in [
            DownloadEvent::Progress,
            DownloadEvent::PayloadComplete,
            DownloadEvent::Admitted,
        ] {
            let error = database
                .transition_download_job(id, late)
                .await
                .expect_err("nothing leaves a terminal state");
            assert!(
                matches!(
                    error,
                    DownloadTransitionError::Rejected(TransitionRejected::Terminal { .. })
                ),
                "{error:?}"
            );
        }
        assert_eq!(dump(&path, "download_job").await, removed);
    }

    /// A job id nothing holds has no state to move, which is a different answer from a refusal.
    #[tokio::test]
    async fn a_transition_for_a_job_that_is_not_there_reports_no_state() {
        let directory = TempDir::new().unwrap();
        let (database, _, _, _) = catalogue(&directory).await;
        assert_eq!(
            database
                .transition_download_job(9_999, DownloadEvent::Admitted)
                .await
                .unwrap(),
            None
        );
    }

    /// SM1's path from the queue to completion, one event at a time, each state read back.
    #[tokio::test]
    async fn a_job_walks_the_declared_path_from_the_queue_to_completion() {
        let directory = TempDir::new().unwrap();
        let (database, _, _, _) = catalogue(&directory).await;
        let id = admit(&database, &directory, 1).await;

        let path: [(DownloadEvent, DownloadState); 5] = [
            (DownloadEvent::Admitted, DownloadState::Inspecting),
            (
                DownloadEvent::Inspected(allowed(&directory)),
                DownloadState::Metadata,
            ),
            (DownloadEvent::EngineAccepted, DownloadState::Downloading),
            (DownloadEvent::PayloadComplete, DownloadState::Seeding),
            (DownloadEvent::SeedingSatisfied, DownloadState::Completed),
        ];
        for (event, expected) in path {
            let moved = database.transition_download_job(id, event).await.unwrap();
            assert_eq!(moved, Some(expected));
            assert_eq!(
                database
                    .download_job(any_account(), id)
                    .await
                    .unwrap()
                    .unwrap()
                    .state,
                expected,
                "what the transition returned is what the row holds"
            );
        }
    }

    /// ARCHITECTURE §15: a full disk pauses a transfer rather than failing it, and the pause is
    /// reversible.
    #[tokio::test]
    async fn a_full_disk_pauses_a_transfer_and_a_resume_continues_it() {
        let directory = TempDir::new().unwrap();
        let (database, _, _, _) = catalogue(&directory).await;
        let id = admit(&database, &directory, 1).await;
        for (event, expected) in [
            (DownloadEvent::Admitted, DownloadState::Inspecting),
            (
                DownloadEvent::Inspected(allowed(&directory)),
                DownloadState::Metadata,
            ),
            (DownloadEvent::EngineAccepted, DownloadState::Downloading),
        ] {
            assert_eq!(
                database.transition_download_job(id, event).await.unwrap(),
                Some(expected)
            );
        }

        assert_eq!(
            database
                .transition_download_job(id, DownloadEvent::DiskFull)
                .await
                .unwrap(),
            Some(DownloadState::Paused)
        );
        // The condition is a level: reporting it again while paused changes nothing.
        assert_eq!(
            database
                .transition_download_job(id, DownloadEvent::DiskFull)
                .await
                .unwrap(),
            Some(DownloadState::Paused)
        );
        assert_eq!(
            database
                .transition_download_job(id, DownloadEvent::Resumed)
                .await
                .unwrap(),
            Some(DownloadState::Downloading)
        );
    }

    // ---- statistics --------------------------------------------------------------

    /// Every figure a tick carries is written, and a tick moves no state.
    #[tokio::test]
    async fn a_statistics_tick_writes_every_figure_it_carries() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _, _) = catalogue(&directory).await;
        let id = database
            .create_download_job(new_job(&directory, certified))
            .await
            .unwrap()
            .id;

        let statistics = DownloadStatistics {
            downloaded_bytes: 1_500,
            uploaded_bytes: 250,
            progress_permille: 1_000,
            download_rate_bps: 12_000,
            upload_rate_bps: 300,
            peers_connected: 12,
            peers_swarm: 400,
            eta_seconds: None,
            ratio: Some(0.17),
        };
        assert!(
            database
                .record_download_statistics(id, statistics)
                .await
                .unwrap()
        );

        let row = database
            .download_job(any_account(), id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.downloaded_bytes, 1_500);
        assert_eq!(row.uploaded_bytes, 250);
        assert_eq!(
            row.progress_permille, 1_000,
            "100% is a real progress value"
        );
        assert_eq!(row.download_rate_bps, 12_000);
        assert_eq!(row.upload_rate_bps, 300);
        assert_eq!(row.peers_connected, 12);
        assert_eq!(row.peers_swarm, 400);
        assert_eq!(row.eta_seconds, None, "an absent estimate is not zero");
        assert_eq!(row.ratio, Some(0.17));
        assert_eq!(
            row.state,
            DownloadState::Queued,
            "a tick is a level, not an edge: the state gate is the caller's `Progress` transition"
        );
    }

    /// The boundary the other end of the same scale: zero bytes is a value the first tick has.
    #[tokio::test]
    async fn a_tick_of_zeroes_is_stored_as_given() {
        let directory = TempDir::new().unwrap();
        let (database, _, _, _) = catalogue(&directory).await;
        let id = admit(&database, &directory, 1).await;

        let statistics = DownloadStatistics {
            downloaded_bytes: 0,
            uploaded_bytes: 0,
            progress_permille: 0,
            download_rate_bps: 0,
            upload_rate_bps: 0,
            peers_connected: 0,
            peers_swarm: 0,
            eta_seconds: Some(0),
            ratio: None,
        };
        assert!(
            database
                .record_download_statistics(id, statistics)
                .await
                .unwrap()
        );

        let row = database
            .download_job(any_account(), id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.downloaded_bytes, 0);
        assert_eq!(row.progress_permille, 0);
        assert_eq!(row.eta_seconds, Some(0));
        assert_eq!(row.ratio, None, "nothing downloaded is not a ratio of zero");
    }

    /// A tick that arrives after the row was removed changes nothing and says so.
    #[tokio::test]
    async fn a_statistics_tick_for_a_job_that_is_not_there_changes_nothing() {
        let directory = TempDir::new().unwrap();
        let (database, _, _, _) = catalogue(&directory).await;
        let statistics = DownloadStatistics {
            downloaded_bytes: 1,
            uploaded_bytes: 0,
            progress_permille: 1,
            download_rate_bps: 1,
            upload_rate_bps: 0,
            peers_connected: 1,
            peers_swarm: 1,
            eta_seconds: None,
            ratio: None,
        };
        assert!(
            !database
                .record_download_statistics(9_999, statistics)
                .await
                .unwrap()
        );
    }

    /// A counter the column cannot hold is refused rather than clamped: a clamped counter is a
    /// number nobody measured.
    #[tokio::test]
    async fn a_statistics_tick_that_cannot_be_stored_is_refused() {
        let directory = TempDir::new().unwrap();
        let (database, _, _, _) = catalogue(&directory).await;
        let id = admit(&database, &directory, 1).await;

        let error = database
            .record_download_statistics(
                id,
                DownloadStatistics {
                    downloaded_bytes: u64::MAX,
                    uploaded_bytes: 0,
                    progress_permille: 0,
                    download_rate_bps: 0,
                    upload_rate_bps: 0,
                    peers_connected: 0,
                    peers_swarm: 0,
                    eta_seconds: None,
                    ratio: None,
                },
            )
            .await
            .expect_err("this is not a byte count a column can hold");
        assert!(
            matches!(
                error,
                StorageError::UnexpectedValue {
                    column: "download_job.downloaded_bytes",
                    ..
                }
            ),
            "{error:?}"
        );
    }

    // ---- the file selection ------------------------------------------------------

    /// Every row of a selection is written from the metadata, with the flags the caller chose and
    /// the domain's own media classification.
    #[tokio::test]
    async fn a_selection_round_trips_with_its_selected_and_media_flags() {
        let directory = TempDir::new().unwrap();
        let (database, _, _, _) = catalogue(&directory).await;
        let id = admit(&database, &directory, 1).await;

        let description = PayloadDescription::new(vec![
            PayloadEntry::new("Show/S01E01.mkv", 1_000, EntryKind::File),
            PayloadEntry::new("Show/S01E02.mkv", 2_000, EntryKind::File),
            PayloadEntry::new("Show/notes.txt", 10, EntryKind::File),
            PayloadEntry::new("Show/link.mkv", 5, EntryKind::Symlink),
        ]);
        database
            .select_download_files(id, description, &[0])
            .await
            .unwrap();

        let rows = database.download_files(any_account(), id).await.unwrap();
        assert_eq!(
            rows.iter().map(|row| row.file_index).collect::<Vec<_>>(),
            vec![0, 1, 2, 3],
            "the metadata's own order is the read's order"
        );
        assert_eq!(
            rows.iter().map(|row| row.path.as_str()).collect::<Vec<_>>(),
            [
                "Show/S01E01.mkv",
                "Show/S01E02.mkv",
                "Show/notes.txt",
                "Show/link.mkv"
            ]
        );
        assert_eq!(
            rows.iter().map(|row| row.size_bytes).collect::<Vec<_>>(),
            vec![1_000, 2_000, 10, 5]
        );
        assert_eq!(
            rows.iter().map(|row| row.selected).collect::<Vec<_>>(),
            vec![true, false, false, false]
        );
        assert_eq!(
            rows.iter().map(|row| row.is_media).collect::<Vec<_>>(),
            vec![true, true, false, false],
            "a text file is not media and a link is not a file anything is written from"
        );
        assert!(rows.iter().all(|row| row.download_job_id == id));
    }

    /// A later selection is the *same* rows with different flags, which is what makes
    /// `PG-E10-04`'s change an update rather than a second file list.
    #[tokio::test]
    async fn a_new_selection_replaces_the_flags_without_a_second_row() {
        let directory = TempDir::new().unwrap();
        let (database, _, _, _) = catalogue(&directory).await;
        let id = admit(&database, &directory, 1).await;
        let description = PayloadDescription::new(vec![
            PayloadEntry::new("Show/S01E01.mkv", 1_000, EntryKind::File),
            PayloadEntry::new("Show/S01E02.mkv", 2_000, EntryKind::File),
        ]);

        database
            .select_download_files(id, description.clone(), &[0])
            .await
            .unwrap();
        let first = database.download_files(any_account(), id).await.unwrap();
        database
            .select_download_files(id, description, &[1])
            .await
            .unwrap();
        let second = database.download_files(any_account(), id).await.unwrap();

        assert_eq!(
            second.iter().map(|row| row.id).collect::<Vec<_>>(),
            first.iter().map(|row| row.id).collect::<Vec<_>>(),
            "the unique key is (job, index), so the same selection is the same rows"
        );
        assert_eq!(
            second.iter().map(|row| row.selected).collect::<Vec<_>>(),
            vec![false, true]
        );
    }

    /// An empty metadata list writes nothing, and a selection that names nothing writes every row
    /// unselected: the two empty cases are different answers.
    #[tokio::test]
    async fn an_empty_metadata_list_writes_no_rows() {
        let directory = TempDir::new().unwrap();
        let (database, _, _, _) = catalogue(&directory).await;
        let id = admit(&database, &directory, 1).await;

        database
            .select_download_files(id, PayloadDescription::new(Vec::new()), &[])
            .await
            .unwrap();
        assert!(
            database
                .download_files(any_account(), id)
                .await
                .unwrap()
                .is_empty(),
            "an empty metadata list writes no rows"
        );

        let description = PayloadDescription::new(vec![
            PayloadEntry::new("Show/S01E01.mkv", 1_000, EntryKind::File),
            PayloadEntry::new("Show/S01E02.mkv", 2_000, EntryKind::File),
        ]);
        database
            .select_download_files(id, description, &[])
            .await
            .unwrap();
        assert_eq!(
            database
                .download_files(any_account(), id)
                .await
                .unwrap()
                .iter()
                .map(|row| row.selected)
                .collect::<Vec<_>>(),
            vec![false, false],
            "a selection that names nothing is still a recorded file list"
        );
    }

    /// A selection index the metadata does not have names no file, so it is refused rather than
    /// silently dropped.
    #[tokio::test]
    async fn a_selection_that_names_no_such_file_is_refused() {
        let directory = TempDir::new().unwrap();
        let (database, _, _, _) = catalogue(&directory).await;
        let id = admit(&database, &directory, 1).await;
        let description = PayloadDescription::new(vec![PayloadEntry::new(
            "Show/S01E01.mkv",
            1,
            EntryKind::File,
        )]);

        let error = database
            .select_download_files(id, description, &[1])
            .await
            .expect_err("the metadata has one entry, so index 1 names nothing");
        assert!(
            matches!(
                error,
                StorageError::UnexpectedValue {
                    column: "download_file.file_index",
                    ..
                }
            ),
            "{error:?}"
        );
        assert!(
            database
                .download_files(any_account(), id)
                .await
                .unwrap()
                .is_empty(),
            "a refused selection writes none of the list"
        );
    }

    /// A file size the column cannot hold is refused, and the refusal applies to the whole list.
    #[tokio::test]
    async fn a_file_size_the_column_cannot_hold_is_refused() {
        let directory = TempDir::new().unwrap();
        let (database, _, _, _) = catalogue(&directory).await;
        let id = admit(&database, &directory, 1).await;
        let description = PayloadDescription::new(vec![
            PayloadEntry::new("Show/S01E01.mkv", 1, EntryKind::File),
            PayloadEntry::new("Show/S01E02.mkv", u64::MAX, EntryKind::File),
        ]);

        let error = database
            .select_download_files(id, description, &[0])
            .await
            .expect_err("8 EiB is not a size a column can hold");
        assert!(
            matches!(
                error,
                StorageError::UnexpectedValue {
                    column: "download_file.size_bytes",
                    ..
                }
            ),
            "{error:?}"
        );
        assert!(
            database
                .download_files(any_account(), id)
                .await
                .unwrap()
                .is_empty(),
            "the transaction leaves no half-written list"
        );
    }

    // ---- the engine's own state --------------------------------------------------

    /// The read that belongs to the column is the only one that carries it, and an absent state
    /// is distinguishable from an empty one.
    #[tokio::test]
    async fn the_engine_state_is_absent_until_it_is_written_and_then_exact() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _, _) = catalogue(&directory).await;
        let id = database
            .create_download_job(new_job(&directory, certified))
            .await
            .unwrap()
            .id;

        assert_eq!(
            database
                .download_engine_state(any_account(), id)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            database
                .download_job(any_account(), id)
                .await
                .unwrap()
                .unwrap()
                .state,
            DownloadState::Queued,
            "the job read is what a caller reads for everything but the engine's bytes"
        );

        for state in [
            Vec::new(),
            vec![0_u8],
            vec![0xff, 0xfe, 0x00, 0x80],
            b"tracker=http://example.invalid/announce".to_vec(),
        ] {
            assert!(
                database
                    .record_download_engine_state(id, &state)
                    .await
                    .unwrap()
            );
            assert_eq!(
                database
                    .download_engine_state(any_account(), id)
                    .await
                    .unwrap(),
                Some(state.clone()),
                "{state:?} must survive: the engine's format is not this crate's to reinterpret"
            );
        }
        assert!(
            !database
                .record_download_engine_state(9_999, b"x")
                .await
                .unwrap(),
            "a state for a job that is not there changes nothing"
        );
    }

    // ---- scope -------------------------------------------------------------------

    /// R34: a job for a title outside the scope is indistinguishable from a job that does not
    /// exist, at every read that returns content.
    #[tokio::test]
    async fn a_restricted_account_cannot_see_a_job_outside_its_scope() {
        let directory = TempDir::new().unwrap();
        let (database, certified, adult, unrated) = catalogue(&directory).await;
        let mut admitted = Vec::new();
        for (offset, title_id) in [certified, adult, unrated].into_iter().enumerate() {
            let mut job = new_job(&directory, title_id);
            job.info_hash = Some(hash(30 + offset as u8));
            admitted.push(database.create_download_job(job).await.unwrap().id);
        }
        let (visible, hidden_adult, hidden_unrated) = (admitted[0], admitted[1], admitted[2]);
        let description = PayloadDescription::new(vec![PayloadEntry::new(
            "Show/S01E01.mkv",
            1,
            EntryKind::File,
        )]);
        for id in &admitted {
            database
                .select_download_files(*id, description.clone(), &[0])
                .await
                .unwrap();
            database
                .record_download_engine_state(*id, b"state")
                .await
                .unwrap();
        }

        for hidden in [hidden_adult, hidden_unrated] {
            assert_eq!(
                database.download_job(restricted(), hidden).await.unwrap(),
                database.download_job(restricted(), 9_999).await.unwrap(),
                "a hidden job reads exactly like one that does not exist"
            );
            assert!(
                database
                    .download_files(restricted(), hidden)
                    .await
                    .unwrap()
                    .is_empty(),
                "a job outside the scope has no readable file rows"
            );
            assert_eq!(
                database
                    .download_engine_state(restricted(), hidden)
                    .await
                    .unwrap(),
                None
            );
            assert_eq!(
                database
                    .transition_download_job(hidden, DownloadEvent::Admitted)
                    .await
                    .unwrap(),
                Some(DownloadState::Inspecting),
                "the scope is a read rule: a write the caller already owns is not scoped"
            );
        }

        assert_eq!(
            ids_of(
                &database
                    .list_download_jobs(restricted(), DownloadState::ALL, PageRequest::first(10))
                    .await
                    .unwrap()
            ),
            vec![visible],
            "a restricted page holds only what the scope may see"
        );
        assert_eq!(
            database
                .download_files(restricted(), visible)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    // ---- keyset pagination -------------------------------------------------------

    /// The queue screen's read: a page below a cursor is not shifted by a row written after the
    /// first page was read, and every row is reached exactly once across the pages.
    #[tokio::test]
    async fn pages_of_jobs_resume_below_the_cursor_and_a_new_row_does_not_shift_them() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _, _) = catalogue(&directory).await;
        let mut ids = Vec::new();
        for byte in 0_u8..5 {
            let mut job = new_job(&directory, certified);
            job.info_hash = Some(hash(byte));
            ids.push(database.create_download_job(job).await.unwrap().id);
        }

        let first = database
            .list_download_jobs(any_account(), DownloadState::ALL, PageRequest::first(2))
            .await
            .unwrap();
        assert_eq!(ids_of(&first), vec![ids[4], ids[3]]);

        let mut interleaved = new_job(&directory, certified);
        interleaved.info_hash = Some(hash(90));
        database.create_download_job(interleaved).await.unwrap();

        let second = database
            .list_download_jobs(
                any_account(),
                DownloadState::ALL,
                PageRequest::after(Cursor::new(first.last().unwrap().id), 2),
            )
            .await
            .unwrap();
        assert_eq!(
            ids_of(&second),
            vec![ids[2], ids[1]],
            "the row written between the reads is not in the second page"
        );
        let third = database
            .list_download_jobs(
                any_account(),
                DownloadState::ALL,
                PageRequest::after(Cursor::new(second.last().unwrap().id), 2),
            )
            .await
            .unwrap();
        assert_eq!(ids_of(&third), vec![ids[0]]);
        assert!(
            database
                .list_download_jobs(
                    any_account(),
                    DownloadState::ALL,
                    PageRequest::after(Cursor::new(ids[0]), 2),
                )
                .await
                .unwrap()
                .is_empty(),
            "the oldest row is the end of the list"
        );

        let mut walked = ids_of(&first);
        walked.extend(ids_of(&second));
        walked.extend(ids_of(&third));
        let mut expected = ids.clone();
        expected.reverse();
        assert_eq!(walked, expected, "the pages reach every row exactly once");
    }

    /// The empty table, and a cursor below every row.
    #[tokio::test]
    async fn an_empty_table_lists_nothing() {
        let directory = TempDir::new().unwrap();
        let (database, _, _, _) = catalogue(&directory).await;
        assert!(
            database
                .list_download_jobs(any_account(), DownloadState::ALL, PageRequest::first(10))
                .await
                .unwrap()
                .is_empty(),
            "a library that has never asked for a download has no jobs"
        );
        assert!(
            database
                .list_download_jobs(
                    any_account(),
                    DownloadState::ALL,
                    PageRequest::after(Cursor::new(1), 10),
                )
                .await
                .unwrap()
                .is_empty(),
            "a cursor below every row has nothing below it"
        );
    }

    /// The state set is what the page holds: the six live states, the terminal three, and the
    /// empty set are all expressible, and nothing else comes back.
    #[tokio::test]
    async fn the_state_filter_selects_exactly_the_states_it_names() {
        let directory = TempDir::new().unwrap();
        let (database, _, _, _) = catalogue(&directory).await;
        let queued = admit(&database, &directory, 1).await;
        let downloading = inspecting(&database, &directory, 40).await;
        assert_eq!(
            database
                .transition_download_job(downloading, DownloadEvent::Inspected(allowed(&directory)))
                .await
                .unwrap(),
            Some(DownloadState::Metadata)
        );
        assert_eq!(
            database
                .transition_download_job(downloading, DownloadEvent::EngineAccepted)
                .await
                .unwrap(),
            Some(DownloadState::Downloading)
        );
        let blocked = inspecting(&database, &directory, 41).await;
        assert_eq!(
            database
                .transition_download_job(
                    blocked,
                    DownloadEvent::Inspected(opt_in_required(&directory))
                )
                .await
                .unwrap(),
            Some(DownloadState::Blocked)
        );

        let list = |states: &[DownloadState]| {
            let database = &database;
            let states = states.to_vec();
            async move {
                database
                    .list_download_jobs(any_account(), &states, PageRequest::first(10))
                    .await
                    .unwrap()
            }
        };
        assert_eq!(ids_of(&list(&[DownloadState::Queued]).await), vec![queued]);
        assert_eq!(
            ids_of(&list(&[DownloadState::Downloading, DownloadState::Paused]).await),
            vec![downloading]
        );
        assert_eq!(
            ids_of(
                &list(&[
                    DownloadState::Queued,
                    DownloadState::Inspecting,
                    DownloadState::Metadata,
                ])
                .await
            ),
            vec![queued]
        );
        assert_eq!(
            ids_of(&list(&[DownloadState::Blocked]).await),
            vec![blocked]
        );
        assert!(
            list(&[
                DownloadState::Completed,
                DownloadState::Failed,
                DownloadState::Removed,
            ])
            .await
            .is_empty(),
            "no job in this fixture is terminal"
        );
        assert!(
            list(&[]).await.is_empty(),
            "a state set that names nothing selects nothing"
        );
        assert_eq!(
            ids_of(&list(DownloadState::ALL).await),
            vec![blocked, downloading, queued],
            "every state is the newest-first whole list"
        );
    }

    // ---- input the repository must refuse ----------------------------------------

    /// The destination is the absolute path the guard resolved; text no guard could have produced
    /// is refused before a row exists.
    #[tokio::test]
    async fn a_save_path_that_is_not_absolute_is_refused() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _, _) = catalogue(&directory).await;
        let mut job = new_job(&directory, certified);
        job.save_path = text("downloads/Alien.1979");

        let error = database
            .create_download_job(job)
            .await
            .expect_err("a relative destination would be resolved against the process's directory");
        assert!(
            matches!(
                error,
                StorageError::UnexpectedValue {
                    column: "download_job.save_path",
                    ..
                }
            ),
            "{error:?}"
        );
        assert!(
            database
                .list_download_jobs(any_account(), DownloadState::ALL, PageRequest::first(10))
                .await
                .unwrap()
                .is_empty(),
            "a refused admission writes no row"
        );
    }

    /// The title is a foreign key, so a job for a title that does not exist never lands.
    #[tokio::test]
    async fn an_unknown_title_id_is_refused_by_the_foreign_key() {
        let directory = TempDir::new().unwrap();
        let (database, _, _, _) = catalogue(&directory).await;
        let job = NewDownloadJob {
            title_id: 9_999,
            ..new_job(&directory, 1)
        };

        let error = database
            .create_download_job(job)
            .await
            .expect_err("the schema's foreign key is what says the title must exist");
        assert!(matches!(error, StorageError::Statement { .. }), "{error:?}");
        assert!(
            database
                .list_download_jobs(any_account(), DownloadState::ALL, PageRequest::first(10))
                .await
                .unwrap()
                .is_empty(),
            "a refused admission writes no row"
        );
    }

    /// A claim the column cannot hold is refused rather than clamped, so the row never records a
    /// size nobody reported.
    #[tokio::test]
    async fn a_size_the_column_cannot_hold_is_refused() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _, _) = catalogue(&directory).await;
        let mut job = new_job(&directory, certified);
        job.reported_size_bytes = Some(u64::MAX);

        let error = database
            .create_download_job(job)
            .await
            .expect_err("8 EiB is not a size a claim can hold");
        assert!(
            matches!(
                error,
                StorageError::UnexpectedValue {
                    column: "download_job.reported_size_bytes",
                    ..
                }
            ),
            "{error:?}"
        );
    }

    /// A row another version wrote can hold a negative count — no `CHECK` constrains one — and it
    /// is reported rather than folded to zero.
    #[tokio::test]
    async fn a_row_with_a_negative_size_is_reported() {
        let directory = TempDir::new().unwrap();
        let (database, _, _, _) = catalogue(&directory).await;
        let path = path_of(&directory);
        let id = admit(&database, &directory, 1).await;
        let description = PayloadDescription::new(vec![PayloadEntry::new(
            "Show/S01E01.mkv",
            1,
            EntryKind::File,
        )]);
        database
            .select_download_files(id, description, &[0])
            .await
            .unwrap();

        let mut connection = open_writer_connection(&path).await.unwrap();
        sqlx::query("UPDATE download_job SET downloaded_bytes = -1 WHERE id = ?1")
            .bind(id)
            .execute(&mut connection)
            .await
            .unwrap();
        sqlx::query("UPDATE download_file SET size_bytes = -1 WHERE download_job_id = ?1")
            .bind(id)
            .execute(&mut connection)
            .await
            .unwrap();
        connection.close().await.unwrap();

        let error = database
            .download_job(any_account(), id)
            .await
            .expect_err("a negative byte count is not a count");
        assert!(
            matches!(
                error,
                StorageError::UnexpectedValue {
                    column: "download_job.downloaded_bytes",
                    ..
                }
            ),
            "{error:?}"
        );
        let error = database
            .download_files(any_account(), id)
            .await
            .expect_err("a negative size is not a size");
        assert!(
            matches!(
                error,
                StorageError::UnexpectedValue {
                    column: "download_file.size_bytes",
                    ..
                }
            ),
            "{error:?}"
        );
    }

    /// A row written by another version is reported, not guessed at: every stored enum, the hash,
    /// the report, and the destination are checked on the way out.
    #[tokio::test]
    async fn a_row_this_build_cannot_read_is_reported() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _, _) = catalogue(&directory).await;
        let path = path_of(&directory);

        // Each case gets its own row, because the first bad column read fails the whole row.
        let cases = [
            ("state = 'cancelled'", "download_job.state", false),
            ("added_by = 'robot'", "download_job.added_by", false),
            ("info_hash = 'nonsense'", "download_job.info_hash", false),
            (
                "safety_report_json = 'nonsense'",
                "download_job.safety_report_json",
                true,
            ),
            (
                "save_path = 'relative/path'",
                "download_job.save_path",
                false,
            ),
            ("magnet_uri = ''", "download_job.magnet_uri", false),
        ];
        let mut connection = open_writer_connection(&path).await.unwrap();
        sqlx::query("PRAGMA ignore_check_constraints = ON")
            .execute(&mut connection)
            .await
            .unwrap();
        let mut ids = Vec::new();
        for (statement, column, is_json) in cases {
            let mut job = new_job(&directory, certified);
            job.info_hash = Some(hash(60 + ids.len() as u8));
            let id = database.create_download_job(job).await.unwrap().id;
            let sql = format!("UPDATE download_job SET {statement} WHERE id = {id}");
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .execute(&mut connection)
                .await
                .unwrap();
            ids.push((id, column, is_json));
        }
        // The hash was overwritten in one case, so the rows no longer hold distinct hashes; the
        // partial index is not consulted by a read.
        connection.close().await.unwrap();

        for (id, column, is_json) in ids {
            let error = database
                .download_job(any_account(), id)
                .await
                .expect_err("this row holds a value this build cannot read");
            if is_json {
                assert!(
                    matches!(error, StorageError::InvalidJsonField { column: found, .. } if found == column),
                    "{column}: {error:?}"
                );
            } else {
                assert!(
                    matches!(error, StorageError::UnexpectedValue { column: found, .. } if found == column),
                    "{column}: {error:?}"
                );
            }
        }
    }

    // ---- property tests ----------------------------------------------------------

    fn any_millis() -> impl Strategy<Value = i64> {
        1_600_000_000_000_i64..1_800_000_000_000
    }

    fn any_optional<T: std::fmt::Debug + Clone + 'static>(
        value: impl Strategy<Value = T>,
    ) -> impl Strategy<Value = Option<T>> {
        proptest::option::of(value)
    }

    // Every column an admission writes survives the round trip, and every column it does not
    // write is the value the database defaults to.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(24))]

        #[test]
        fn any_download_job_round_trips(
            release_title in "[A-Za-z0-9. ]{1,24}",
            release_source in any_optional("[A-Za-z]{1,8}"),
            release_group in any_optional("[A-Za-z0-9-]{1,8}"),
            resolution in any_optional("[0-9]{3,4}p"),
            video_codec in any_optional("[a-z0-9]{1,8}"),
            hdr_format in any_optional("[a-z0-9]{1,8}"),
            audio_summary in any_optional("[A-Za-z0-9. ]{1,12}"),
            reported_size_bytes in any_optional(0_u64..1_000_000_000_000),
            info_hash in any_optional(any::<[u8; 20]>()),
            with_episode in any::<bool>(),
            magnet in any::<bool>(),
            source_text in "[A-Za-z0-9:?=&._/-]{1,24}",
            suffix in "[A-Za-z0-9_.]{1,12}",
            priority in 0_i64..1_000,
            added_by in prop::sample::select(AddedBy::ALL.to_vec()),
            added_at in any_millis(),
        ) {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a test runtime");
            runtime.block_on(async {
                let directory = TempDir::new().expect("a temporary directory");
                let (database, certified, _, _) = catalogue(&directory).await;
                let episode_id = if with_episode {
                    let path = path_of(&directory);
                    season(&path, 1, certified, 1).await;
                    episode(&path, 1, certified, 1, 1, 1).await;
                    Some(1)
                } else {
                    None
                };
                let destination = directory.path().join(&suffix);
                let save_path = text(
                    destination.to_str().expect("a temporary path is UTF-8"),
                );
                let source = if magnet {
                    DownloadSource::Magnet(MagnetUri::new(text(&source_text)))
                } else {
                    DownloadSource::TorrentFile(text(&source_text))
                };
                let expected = NewDownloadJob {
                    title_id: certified,
                    episode_id,
                    release_title: release_title.clone(),
                    release_source: release_source.clone(),
                    release_group: release_group.clone(),
                    resolution: resolution.clone(),
                    video_codec: video_codec.clone(),
                    hdr_format: hdr_format.clone(),
                    audio_summary: audio_summary.clone(),
                    reported_size_bytes,
                    info_hash: info_hash.map(InfoHash::from_bytes),
                    protocol: Protocol::Torrent,
                    nzb_sha256: None,
                    source: source.clone(),
                    save_path,
                    priority,
                    added_by,
                    added_at: at(added_at),
                };

                let id = database
                    .create_download_job(expected.clone())
                    .await
                    .expect("inserts")
                    .id;
                let stored = database
                    .download_job(any_account(), id)
                    .await
                    .expect("reads")
                    .expect("the row");

                prop_assert_eq!(stored.id, id);
                prop_assert_eq!(stored.title_id, certified);
                prop_assert_eq!(stored.episode_id, episode_id);
                prop_assert_eq!(&stored.release_title, &release_title);
                prop_assert_eq!(stored.release_source.as_deref(), release_source.as_deref());
                prop_assert_eq!(stored.release_group.as_deref(), release_group.as_deref());
                prop_assert_eq!(stored.resolution.as_deref(), resolution.as_deref());
                prop_assert_eq!(stored.video_codec.as_deref(), video_codec.as_deref());
                prop_assert_eq!(stored.hdr_format.as_deref(), hdr_format.as_deref());
                prop_assert_eq!(stored.audio_summary.as_deref(), audio_summary.as_deref());
                prop_assert_eq!(stored.reported_size_bytes, reported_size_bytes);
                prop_assert_eq!(stored.info_hash, info_hash.map(InfoHash::from_bytes));
                prop_assert_eq!(stored.source.as_ref(), Some(&source));
                prop_assert_eq!(stored.save_path.as_str(), destination.to_str().expect("UTF-8"));
                prop_assert_eq!(stored.priority, priority);
                prop_assert_eq!(stored.added_by, added_by);
                prop_assert_eq!(stored.added_at, at(added_at));

                // Everything an admission does not write.
                prop_assert_eq!(stored.state, DownloadState::Queued);
                prop_assert_eq!(stored.job_id, None);
                prop_assert_eq!(stored.media_item_id, None);
                prop_assert_eq!(stored.quality_profile_id, None);
                prop_assert_eq!(stored.indexer_id, None);
                prop_assert!(!stored.is_upgrade);
                prop_assert_eq!(stored.downloaded_bytes, 0);
                prop_assert_eq!(stored.uploaded_bytes, 0);
                prop_assert_eq!(stored.progress_permille, 0);
                prop_assert_eq!(stored.download_rate_bps, 0);
                prop_assert_eq!(stored.upload_rate_bps, 0);
                prop_assert_eq!(stored.peers_connected, 0);
                prop_assert_eq!(stored.peers_swarm, 0);
                prop_assert_eq!(stored.eta_seconds, None);
                prop_assert_eq!(stored.ratio, None);
                prop_assert_eq!(stored.safety_report, None);
                prop_assert_eq!(stored.error_code, None);
                prop_assert_eq!(stored.error_message, None);
                prop_assert_eq!(stored.started_at, None);
                prop_assert_eq!(stored.completed_at, None);
                prop_assert_eq!(stored.seeding_until, None);
                prop_assert_eq!(stored.removed_at, None);
                Ok(())
            })?;
        }
    }

    // Every column of `download_file` survives the round trip, for any list of entries and any
    // selection drawn from it.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(24))]

        #[test]
        fn any_download_file_round_trips(
            files in proptest::collection::vec(
                ("[A-Za-z0-9_.]{1,16}", 0_u64..1_000_000_000, any::<bool>()),
                1..6,
            ),
        ) {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a test runtime");
            runtime.block_on(async {
                let directory = TempDir::new().expect("a temporary directory");
                let (database, certified, _, _) = catalogue(&directory).await;
                let id = database
                    .create_download_job(new_job(&directory, certified))
                    .await
                    .expect("inserts")
                    .id;
                let description = PayloadDescription::new(
                    files
                        .iter()
                        .map(|(path, bytes, _)| PayloadEntry::new(path.clone(), *bytes, EntryKind::File))
                        .collect(),
                );
                let selection: Vec<usize> = files
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, _, selected))| *selected)
                    .map(|(index, _)| index)
                    .collect();

                database
                    .select_download_files(id, description, &selection)
                    .await
                    .expect("writes");
                let rows = database
                    .download_files(any_account(), id)
                    .await
                    .expect("reads");

                prop_assert_eq!(rows.len(), files.len());
                for (position, ((path, bytes, selected), row)) in files.iter().zip(rows.iter()).enumerate() {
                    prop_assert_eq!(row.download_job_id, id);
                    prop_assert_eq!(row.file_index, i64::try_from(position).expect("small"));
                    prop_assert_eq!(row.path.as_str(), path.as_str());
                    prop_assert_eq!(row.size_bytes, *bytes);
                    prop_assert_eq!(row.selected, *selected);
                    prop_assert_eq!(row.is_media, is_media_extension(extension_of(path)));
                }
                Ok(())
            })?;
        }
    }

    // The engine's bytes are stored as they arrived, whatever they are: a byte sequence that is
    // not text must not be lossy or reinterpreted.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(24))]

        #[test]
        fn any_engine_state_round_trips(state in proptest::collection::vec(any::<u8>(), 0..64)) {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a test runtime");
            runtime.block_on(async {
                let directory = TempDir::new().expect("a temporary directory");
                let (database, certified, _, _) = catalogue(&directory).await;
                let id = database
                    .create_download_job(new_job(&directory, certified))
                    .await
                    .expect("inserts")
                    .id;
                database
                    .record_download_engine_state(id, &state)
                    .await
                    .expect("writes");
                prop_assert_eq!(
                    database
                        .download_engine_state(any_account(), id)
                        .await
                        .expect("reads"),
                    Some(state)
                );
                Ok(())
            })?;
        }
    }

    /// The extension of a fixture path, as the media classifier reads it.
    fn extension_of(path: &str) -> &str {
        Path::new(path)
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("")
    }
}
