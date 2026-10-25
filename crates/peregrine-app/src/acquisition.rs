//! `AcquisitionService`: the write half of `FLOWS` F6.
//!
//! The one place the inspection gate, the torrent engine port, and the download repositories are
//! combined. It is deliberately not the whole of acquisition: the metadata-only fetch that produces
//! a [`PayloadDescription`] is `PG-E10-03`'s, the release decision that chooses one is `PG-E12-10`'s,
//! and the queue's commands and screen are `PG-E13`'s. What this slice owns is the sequence between
//! them and the two invariants that only exist inside that sequence.
//!
//! # The order is the guarantee
//!
//! `FLOWS` F6 sequences the whole flow, and each step here is that order rather than a convenient
//! re-arrangement of it:
//!
//! 1. The title is resolved through the repository's **scoped** read. Out of scope reads as
//!    *absent* and is refused as [`AcquisitionError::TitleUnavailable`], the same refusal a title
//!    that was never inserted produces (`AGENTS` R34). This is the only part of `PG-E35-01`'s
//!    capability gate that can exist before the capability model does; `can_acquire` and rating
//!    ceilings are that task's, and nothing here invents them.
//! 2. The destination is resolved by [`PathGuard`], so the engine can only ever be handed a
//!    [`SafePath`] (`ADR` 0005). A destination that cannot be resolved is refused **before** the job
//!    row is inserted, so a job that could never be added leaves no row behind.
//! 3. The job row is inserted at `queued` — the literal the repository's own insert writes.
//! 4. `Admitted` → `inspecting`, then the real [`PayloadInspector`] runs over the description the
//!    caller supplied.
//! 5. The report that inspection produced is applied as `Inspected(report)`. That transition is what
//!    persists `safety_report_json`, and `DownloadEvent` is the *only* way this service moves a
//!    job's state: there is no parameter of any method here that is a [`DownloadState`], and every
//!    write goes through `Database::transition_download_job`, which itself calls
//!    `peregrine_domain::transition`.
//! 6. The [`AddRequest`] is built **from the report the gate produced and the transition persisted**,
//!    not from a state the caller asserted. `AddRequest::new` refuses a report whose verdict is not
//!    `Allowed`, so a blocked or opt-in-required torrent has no spelling that reaches
//!    `TorrentEngine::add`. That is F6's pre-write guarantee as a type rather than as a rule to
//!    remember — and the tests assert it against the fake's call log rather than by reading this.
//! 7. `engine.add`, then `EngineAccepted` → `downloading`, then the file rows and the engine's own
//!    state are persisted. SM1 gives every state in which the engine holds the torrent a
//!    `engine_state_json`, so a restart can reattach rather than restart (`PG-E10-08` reads it back).
//!
//! # Two refusals that must not be collapsed
//!
//! The engine holds *levels*: `pause`, `resume`, and `remove` are idempotent there because a UI
//! toggle dispatched twice must not raise an error at the state the user asked for. The job is a
//! *state machine*: a second pause is out of order and is refused
//! ([`TransitionRejected::OutOfOrder`]). Both answers are kept apart — [`AcquisitionError::Engine`]
//! is "the engine would not", [`AcquisitionError::TransitionRefused`] is "this job is not where you
//! think it is" — because a caller acts differently on each.
//!
//! # Progress is coalesced here, driven by the caller, and never by a clock this service owns
//!
//! `ARCHITECTURE` §6 budgets progress at 1 Hz to the database and 4 Hz to the UI. The second is the
//! [`EventBus`]'s job — it delivers at most one update per topic and key per 250 ms tick — so this
//! service coalesces once, at the 1 Hz write budget, and a tick the budget discards writes nothing
//! and publishes nothing. Two ticks inside one second therefore cost a clock read.
//!
//! **Nothing here polls.** `statistics` is a call the caller drives, and the free-space condition is
//! checked inside that same call, so the whole path runs under an injected clock with no `sleep`
//! anywhere (`AGENTS` R14). A shipping background poller is a separate, cancellable unit whose
//! interval is injected; the queue slice (`PG-E13`) owns it, and it is deliberately not here,
//! because a poller in this module would be the one thing in this slice that no test could stop
//! from touching real time.
//!
//! # Free space is a port, and its shipping adapter is `crate::volume::SystemVolume`
//!
//! [`VolumeSpace`] is the port the admission floor and the full-disk pause read. It is a trait
//! rather than a `u64` argument because one value decides two different things at two different
//! moments — admission before an add, and a level during a transfer — and a caller that supplied it
//! once could not report the second. A native read is FFI on both desktop platforms (`statvfs`,
//! `GetDiskFreeSpaceEx`) and every crate in this tree is `#![forbid(unsafe_code)]`, so the shipping
//! implementation asks the platform's own tool instead; it lives in [`crate::volume`], and a native
//! adapter in `peregrine-platform` can replace it behind the same trait.
//!
//! # What is deliberately absent
//!
//! - **No IPC command, no UI, and no Tauri type.** This is the seam those go through, and nothing
//!   in this file names one.
//! - **No `Runtime` registration.** `Runtime` cannot construct this service until a real engine
//!   exists (`PG-E10-02`), and a constructor nothing can call would be dead code behind an
//!   `Option`. The precedent is `PG-E1-19`: the service lands with a constructor that takes its
//!   ports, and the binary assembles it when the port it needs is real.
//! - **No per-file selection, no download-in-order toggle, no limits, no reattach, no seeding
//!   policy.** Those are `PG-E10-04`, `-05`, `-08`, `-09`, and `-10`. This slice selects every file
//!   the description lists and asks for a sequential transfer, which is F6's own default.
//! - **No `.torrent`-file source.** A row stores a path, but the engine must be handed the file's
//!   *contents* (`ADR` 0005: the engine never receives a path outside the download root), and
//!   reading them belongs to the adapter that supplies a source. It is refused as
//!   [`AcquisitionError::UnsupportedSource`] rather than read here.
//! - **No state fan-out.** Every transition is written; publishing one event per transition is
//!   `PG-E13-01`'s, which owns the topic's consumer.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jiff::Timestamp;
use peregrine_domain::{
    AccountScope, Clock, DeletionEntry, DeletionPlan, DownloadEvent, DownloadState, InfoHash,
    InspectionLimits, MediaFile, MediaItem, MediaItemState, MediaOrigin, NonEmptyText, OptIn,
    PathGuard, PathGuardError, PayloadDescription, PayloadInspector, Protocol, ReleaseName,
    SafePath, SafetyReport, TransitionRejected, Trash, TrashError, Verdict,
};
use peregrine_storage::{
    Database, DownloadFileRow, DownloadJobRow, DownloadSource, DownloadStatistics,
    DownloadTransitionError, NewDownloadJob, PageRequest, StorageError,
};
use peregrine_torrent::{
    AddIntent, AddRefused, AddRequest, EngineError, EngineState, FileSelection, TorrentEngine,
    TorrentHandle, TorrentHealth, TorrentMetadataFetch, TorrentSource,
};
use peregrine_usenet::{AddNzbRequest, NzbDocument, ServerConfig, UsenetEngine, UsenetHandle};
use thiserror::Error;
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::events::{DOWNLOAD_STATS, Event, EventBus};
use crate::library::scan::modified_millis;

/// How often one job's statistics may reach the database (`ARCHITECTURE` §6).
///
/// The UI's 4 Hz is the [`EventBus`]'s tick, so one budget serves both: a tick this discards
/// publishes nothing either.
const TICK_INTERVAL_MILLIS: i64 = 1_000;

/// How often the pump asks every live engine for its statistics.
///
/// The polling interval and the coalescing budget are one number: a faster pump would be
/// discarded by [`AcquisitionService::statistics`] without even reading the engine, and a
/// slower one would leave the queue behind the transfer it is describing.
const SAMPLE_INTERVAL: Duration = Duration::from_millis(TICK_INTERVAL_MILLIS as u64);

/// How long a peerless transfer is left alone before the service asks its swarm again.
///
/// Two minutes is longer than an announcement's round trip and shorter than a person's patience:
/// a torrent that found no peer in that window is one the trackers are done answering for.
const STALL_GRACE_MILLIS: i64 = 120_000;

/// How long between those attempts, once the first one has been made.
const STALL_RETRY_MILLIS: i64 = 300_000;

/// How many attempts a stalled row is given before it is failed with the reason.
const STALL_ATTEMPTS: u32 = 3;

/// How long an engine add may take before the service gives up on it.
///
/// **librqbit resolves a magnet's metadata inside `add`.** A torrent whose swarm never answers
/// therefore holds the calling task for as long as the magnet exists — and for an IPC command that
/// is the button that asked for it, stuck for hours behind a swarm that will never answer. Thirty
/// seconds is longer than a healthy resolution and shorter than a person's patience; a dropped add
/// leaves no torrent in the session, so trying again later is the same action.
const ADD_TIMEOUT: Duration = Duration::from_secs(30);

/// How often the pump retries a live row it holds no engine handle for.
const REATTACH_INTERVAL_MILLIS: i64 = 120_000;

/// How often the handoffs left in `metadata` are retried.
///
/// The magnet's metadata is what a handoff waits on, and Prowlarr's swarms answer in seconds, so a
/// sweep this often is a retry budget rather than a poll.
const HANDOFF_SWEEP_MILLIS: i64 = 30_000;

/// How long recovery may wait for a magnet's file list before leaving the row for the next start.
///
/// A magnet with no peers would otherwise hold startup or a Resume press for as long as the swarm
/// takes, which is a wait the queue cannot report. The timeout turns it into a refusal the person
/// can see; the row is left as it was, so trying again later is the same action.
const METADATA_RECOVERY_TIMEOUT: Duration = Duration::from_secs(20);

/// Why an acquisition operation could not be carried out.
///
/// Every message says what the caller can do, and none names a path, a magnet, or an info hash —
/// the engine's own sentences already respect that (`PG-E10-15`) and interpolating a value would
/// undo it.
#[derive(Debug, Error)]
pub enum AcquisitionError {
    /// The title is not visible to this account, or does not exist. The two are one answer on
    /// purpose (`AGENTS` R34). `can_acquire` is deliberately not consulted: `PG-E35-01` owns the
    /// capability model.
    #[error("that title is not available to this account, so it cannot be downloaded")]
    TitleUnavailable,

    /// The destination is not inside the configured download root, or is not a path a component
    /// may hold. Nothing was written.
    #[error("the download destination is not usable: {source}")]
    SavePath {
        /// Which rule refused it.
        #[source]
        source: PathGuardError,
    },

    /// A live job already holds this torrent's info hash (`DATA_MODEL` I36).
    ///
    /// A job that failed or was removed vacates the hash, so a retry after one of those is a fresh
    /// job rather than a conflict (`FLOWS` SM1).
    #[error("another download is already running for this release; remove it and try again")]
    Duplicate,

    /// The safety gate still needs the advanced opt-in for this torrent.
    #[error("the safety gate still needs the explicit opt-in for this torrent")]
    OptInStillRequired,

    /// The report consent carried refused the torrent outright, so no consent can lift it
    /// (`SECURITY` T1 to T3).
    #[error("the safety gate refused this torrent with no override, so it can never be added")]
    NoOverride,

    /// The volume holding the destination does not have the room the job needs.
    #[error(
        "the download needs {required_bytes} bytes and only {available_bytes} are free above the \
         floor; free some space and try again"
    )]
    DiskFull {
        /// What the transfer would write.
        required_bytes: u64,
        /// What is free beyond the configured floor.
        available_bytes: u64,
    },

    /// The free space could not be read, so admission fails closed rather than adding a torrent it
    /// cannot size a volume for.
    #[error("the free space on the download volume could not be read, so nothing was added")]
    SpaceUnknown {
        /// Why the read failed.
        #[source]
        source: SpaceError,
    },

    /// The engine refused the operation. Its sentences are fixed and carry no value.
    #[error(transparent)]
    Engine(#[from] EngineError),

    /// The engine would not build the request from the report it was given, which means the report
    /// was not the one the gate persisted.
    #[error(transparent)]
    AddRefused(#[from] AddRefused),

    /// The domain refused an event for the state the job is in — a second pause, a resume of a
    /// running job, a statistics tick after a terminal state.
    ///
    /// The event arrives as text because the domain's own `name` is private and a message must not
    /// carry the report an event may hold.
    #[error("the download job did not accept {event}: {source}")]
    TransitionRefused {
        /// The event's name, without its report.
        event: String,
        /// Why the domain refused it.
        #[source]
        source: TransitionRejected,
    },

    /// No job holds that id, so there was nothing to move.
    #[error("that download job no longer exists")]
    NotFound,

    /// The row exists, but this process holds no engine handle for it.
    ///
    /// A restart loses every in-process handle, so a live-looking row may need its handoff completed
    /// before it can be paused or streamed. The sentence names the action that does that.
    #[error("that download is not running in this session; resume it to continue")]
    NotLive,

    /// A queue entry that has not finished cannot be cleared: it is still transferring, waiting in
    /// line, or blocked with a report the user has not answered.
    #[error("that download has not finished, so it stays in the queue")]
    NotFinished,

    /// The database refused a read or a write.
    #[error(transparent)]
    Storage(#[from] StorageError),

    /// A [`DownloadSource::TorrentFile`] job was started, and the bytes that go to the engine are
    /// not this slice's to read.
    #[error("this build cannot start a download from a .torrent file path yet")]
    UnsupportedSource,

    /// The operating system would not take a deleted file into its trash, so nothing was removed
    /// ([AGENTS R22]).
    #[error(transparent)]
    Trash(#[from] TrashError),

    /// A Usenet job was started before a provider was configured.
    #[error("no Usenet provider is configured")]
    UsenetUnavailable,

    /// The Usenet engine refused the add because the report was not an allowed one.
    #[error(transparent)]
    UsenetAddRefused(#[from] peregrine_usenet::AddRefused),

    /// The Usenet engine refused an operation.
    #[error(transparent)]
    UsenetEngine(#[from] peregrine_usenet::EngineError),

    /// The NZB could not be written beside the payload.
    #[error("the NZB could not be stored beside the payload: {0}")]
    Io(#[from] std::io::Error),
}

/// Why a volume's free space could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SpaceError {
    /// The path does not exist or cannot be reached.
    #[error("the destination volume is not reachable")]
    Unreachable,
    /// The operating system refused the query.
    #[error("the operating system refused to report free space")]
    Denied,
}

/// How much room a volume has for a path.
///
/// One method because one fact is read: the bytes a *new* write could use. Quotas, reserved blocks,
/// and the difference between "free" and "available to this user" are the adapter's problem, and the
/// adapter promises the conservative one — what this process can actually write.
pub trait VolumeSpace: Send + Sync {
    /// Available bytes on the volume holding `path`.
    ///
    /// # Errors
    ///
    /// [`SpaceError`] when the volume cannot be asked. Admission treats that as a refusal rather
    /// than as room, because adding a torrent of unknown size to a full disk is the failure the
    /// floor exists to prevent.
    fn available_bytes(&self, path: &std::path::Path) -> Result<u64, SpaceError>;
}

/// What one [`AcquisitionService::start`] is given.
///
/// The job's own columns arrive as a [`NewDownloadJob`] rather than as twenty parameters, which is
/// also what keeps the service's vocabulary the repository's: a second spelling of the same row
/// would be a second thing to keep in step. Two of its fields are overwritten rather than read —
/// `info_hash` comes from `info_hash` here and `added_at` from the injected clock — so a caller
/// cannot state a job's identity or its admission instant from memory.
#[derive(Debug)]
pub struct StartRequest {
    /// Which account is asking, and therefore what it may see (`AGENTS` R34).
    pub account: AccountScope,
    /// The job's columns, except the two the service fills in.
    pub job: NewDownloadJob,
    /// The torrent's file list, as the metadata fetch (`PG-E10-03`) produced it.
    pub description: PayloadDescription,
    /// The entries this download may write, when the caller narrows them.
    ///
    /// `None` selects every entry, which is the shape a single release and a title-wide acquisition
    /// both have. A season pack passes the entries whose own names place them in the requested
    /// season, so a torrent that carries two seasons does not download both.
    pub selection: Option<Vec<usize>>,
    /// The torrent's identity, which is also the dedupe key.
    pub info_hash: InfoHash,
}

/// What one [`AcquisitionService::start_usenet`] is given.
pub struct UsenetStartRequest {
    /// Which account is asking, and therefore what it may see (`AGENTS` R34).
    pub account: AccountScope,
    /// The job's columns, except the source path and admission instant the service fills in.
    pub job: NewDownloadJob,
    /// The parsed post, already bounded by the parser.
    pub document: NzbDocument,
    /// The NZB bytes, stored beside the payload so a restart can re-inspect it.
    pub nzb_bytes: Vec<u8>,
    /// The providers this job may use, primary first.
    pub servers: Vec<ServerConfig>,
}

/// What one [`AcquisitionService::start`] produced.
#[derive(Debug, Clone, PartialEq)]
pub struct Started {
    /// The job row as it stands after the call, re-read from the database.
    pub job: DownloadJobRow,
    /// The state the last transition produced.
    pub state: DownloadState,
    /// The verdict the gate reached.
    pub verdict: Verdict,
    /// Whether the torrent reached the engine. `false` for a job parked in `blocked`, which is the
    /// one fact the tests assert against the engine's call log.
    pub added: bool,
    /// The files the engine was told to write, ascending. Empty when nothing was added.
    pub selection: Vec<usize>,
}

/// The acquisition write path.
///
/// Held by the runtime once a real engine exists (`PG-E10-02`). Every dependency is injected, so a
/// test runs the whole path against a temporary database, a `FakeTorrentEngine`, and a `FakeClock`,
/// and no call in this type reads a wall clock, a random source, or the network.
pub struct AcquisitionService {
    database: Arc<Database>,
    engine: Arc<dyn TorrentEngine>,
    clock: Arc<dyn Clock + Send + Sync>,
    destination: PathGuard,
    volume: Arc<dyn VolumeSpace>,
    /// Bytes that must stay free on the volume. Both the admission check and the full-disk pause
    /// read it, so the floor is one number rather than two that can drift.
    free_space_floor: u64,
    bus: Arc<EventBus>,
    /// Where a deleted file goes. A port rather than a direct call because the trash is the
    /// operating system's and the domain may not reach for it ([AGENTS R8], [AGENTS R22]).
    trash: Arc<dyn Trash>,
    /// Where a finished or failed transfer is announced (PG-E13-08).
    notices: Arc<dyn DownloadNotices>,
    /// The handles this process holds, so an engine operation after an add can name its torrent
    /// without the row carrying a number. A job started by another process has no handle here and
    /// is `PG-E10-08`'s to reattach.
    handles: Mutex<HashMap<i64, TorrentHandle>>,
    /// The Usenet engine, when this session has a configured provider. `None` leaves the transfer
    /// protocol unavailable rather than failing the whole service.
    usenet: Option<Arc<dyn UsenetEngine>>,
    /// The Usenet jobs this process started, keyed by row id.
    usenet_handles: Mutex<HashMap<i64, UsenetHandle>>,
    /// The metadata-only fetch, when this session has one.
    ///
    /// Used only by recovery: a job interrupted before the engine accepted it has no file list, and
    /// the stored magnet is the one source that can produce it again (PG-E10-03's read, reused).
    metadata: Option<Arc<dyn TorrentMetadataFetch>>,
    /// The archive tools, when this build found them. None means an archive-only payload fails
    /// with a named refusal rather than reporting a completion with nothing imported.
    post: Option<crate::usenet_post::PostTools>,
    /// Where an imported episode with no still gets one from its own file, when this build can take
    /// one. `None` leaves every still as the provider wrote it, which is what a unit test that is not
    /// about captures sees.
    stills: Option<Arc<crate::episode_stills::EpisodeStills>>,
    /// The last instant each job's statistics reached the database, which is the whole of the
    /// coalescing state.
    written_at: Mutex<HashMap<i64, Timestamp>>,
    /// The stall history of every live row the pump has seen, so a peerless transfer is asked for
    /// peers again and eventually fails with a reason instead of parking for ever.
    stalls: Mutex<HashMap<i64, StallWatch>>,
    /// When the handoffs left in `metadata` were last retried.
    last_handoff_sweep: Mutex<Option<i64>>,
    /// When the pump may next try to re-add a live row it holds no handle for.
    reattach_at: Mutex<HashMap<i64, i64>>,
    /// The statistics pump, started by [`AcquisitionService::start_pump`] and `None` until then.
    ///
    /// The engine is pull-only: it transfers without telling anyone, so without this a live
    /// transfer writes nothing — the row stays at zero and `download://stats` never fires. The
    /// composition root starts it once a real engine exists and stops it before the bus; a test
    /// that drives [`AcquisitionService::statistics`] directly never starts one.
    pump: Mutex<Option<Pump>>,
}

/// Files belonging to a library title or episode that can be moved to the OS trash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaDeletionPlan {
    /// The files named by the confirmation step.
    pub entries: Vec<DeletionEntry>,
    /// The total size of those files.
    pub total_bytes: u64,
}

impl MediaDeletionPlan {
    fn new(entries: Vec<DeletionEntry>) -> Self {
        let total_bytes = entries.iter().map(|entry| entry.bytes).sum();
        Self {
            entries,
            total_bytes,
        }
    }
}

/// Told when a download reaches a state a person would want to hear about.
///
/// The port exists so acquisition never reaches for the notification service: the service owns the
/// centre and the toast, and a job's last transition is the one event it cannot raise for itself
/// (PG-E13-08). The shipping runtime's implementation writes the centre row and publishes the
/// refresh; a test's does nothing.
pub trait DownloadNotices: Send + Sync {
    /// Download `download_id` is finished, `failed` or not. Best effort: a notice that could not
    /// be written must never change the job's outcome.
    fn finished<'a>(
        &'a self,
        download_id: i64,
        failed: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>;
}

/// The notice port until a runtime supplies one.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoDownloadNotices;

impl DownloadNotices for NoDownloadNotices {
    fn finished<'a>(
        &'a self,
        _download_id: i64,
        _failed: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
}

/// What one observation of a live transfer asks the service to do about its peers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StallAction {
    /// The transfer is moving, so there is nothing a peer search would add.
    Moving,
    /// Stalled, but not for long enough to act, or waiting out the retry interval.
    Waiting,
    /// Ask the engine's swarm for peers again.
    Reannounce,
    /// The attempts are spent: the row has to be failed with the reason.
    GiveUp,
}

/// One live row's stall history.
///
/// The instant comes from the caller rather than a clock of this struct's own, so the rule is a
/// function of the observations and a test drives it with explicit times (AGENTS R14). An engine
/// that reports no peers is not an error — a swarm can be asleep — but it is also not progress, and
/// before this nothing in the app told the two apart: the row sat at `downloading` for ever.
#[derive(Debug, Default)]
struct StallWatch {
    /// When the current stall was first observed, or `None` while the transfer moves.
    since: Option<i64>,
    /// How many peer searches this stall has already caused.
    attempts: u32,
    /// When the last one was made.
    last_attempt_at: Option<i64>,
}

impl StallWatch {
    /// Folds in one observation and answers what it asks for.
    fn observe(&mut self, now: i64, stalled: bool) -> StallAction {
        if !stalled {
            *self = Self::default();
            return StallAction::Moving;
        }
        let Some(since) = self.since else {
            self.since = Some(now);
            return StallAction::Waiting;
        };
        let due = match self.last_attempt_at {
            None => now.saturating_sub(since) >= STALL_GRACE_MILLIS,
            Some(at) => now.saturating_sub(at) >= STALL_RETRY_MILLIS,
        };
        if !due {
            return StallAction::Waiting;
        }
        if self.attempts >= STALL_ATTEMPTS {
            return StallAction::GiveUp;
        }
        self.attempts = self.attempts.saturating_add(1);
        self.last_attempt_at = Some(now);
        StallAction::Reannounce
    }
}

/// The task that asks every live engine for its statistics on a tick.
///
/// The shape [`EventBus`] uses for its own tick: a token to stop it and the handle to await,
/// both owned by the thing that started it, so a stopped service cannot leave a task publishing
/// into a bus that has already shut down.
struct Pump {
    shutdown: CancellationToken,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl AcquisitionService {
    /// Plans removal of the present files for a title, optionally narrowed to one episode.
    pub async fn media_deletion_plan(
        &self,
        scope: AccountScope,
        title_id: i64,
        episode: Option<(u32, u32)>,
    ) -> Result<MediaDeletionPlan, AcquisitionError> {
        let episode_id = match episode {
            Some((season, number)) => {
                self.database
                    .episode_id_for(title_id, season, number)
                    .await?
            }
            None => None,
        };
        let evidence = self.database.title_evidence(scope, title_id).await?;
        Ok(MediaDeletionPlan::new(
            self.enumerate_media(&evidence, episode_id).await,
        ))
    }

    /// Moves title or episode files to the OS trash after re-enumerating them.
    pub async fn delete_media_files(
        &self,
        scope: AccountScope,
        title_id: i64,
        episode: Option<(u32, u32)>,
    ) -> Result<MediaDeletionPlan, AcquisitionError> {
        let episode_id = match episode {
            Some((season, number)) => {
                self.database
                    .episode_id_for(title_id, season, number)
                    .await?
            }
            None => None,
        };
        let evidence = self.database.title_evidence(scope, title_id).await?;
        let plan = MediaDeletionPlan::new(self.enumerate_media(&evidence, episode_id).await);
        for entry in &plan.entries {
            let candidate = std::path::PathBuf::from(entry.path.as_str());
            let safe = self
                .destination
                .resolve(&candidate)
                .map_err(|source| AcquisitionError::SavePath { source })?;
            let trash = Arc::clone(&self.trash);
            let path = safe.as_path().to_path_buf();
            tokio::task::spawn_blocking(move || trash.move_to_trash(&path))
                .await
                .map_err(|_| TrashError::Refused)??;
            self.database
                .delete_media_file_by_path(safe.as_path().to_string_lossy().as_ref())
                .await?;
        }
        Ok(plan)
    }

    /// Builds the service over its ports.
    ///
    /// `destination` is the guard for the configured download root, and `free_space_floor` is the
    /// room the floor reserves on it (`ARCHITECTURE` §10). Both are configuration the caller
    /// resolved rather than values this type derives, because a service that read a setting would be
    /// a second place the setting is spelled.
    #[must_use]
    #[allow(clippy::too_many_arguments)] // The service's dependencies, all resolved by its caller.
    pub fn new(
        database: Arc<Database>,
        engine: Arc<dyn TorrentEngine>,
        clock: Arc<dyn Clock + Send + Sync>,
        destination: PathGuard,
        volume: Arc<dyn VolumeSpace>,
        free_space_floor: u64,
        bus: Arc<EventBus>,
        trash: Arc<dyn Trash>,
    ) -> Self {
        Self {
            database,
            engine,
            clock,
            destination,
            volume,
            free_space_floor,
            bus,
            trash,
            notices: Arc::new(NoDownloadNotices),
            handles: Mutex::new(HashMap::new()),
            usenet: None,
            usenet_handles: Mutex::new(HashMap::new()),
            metadata: None,
            post: None,
            stills: None,
            written_at: Mutex::new(HashMap::new()),
            stalls: Mutex::new(HashMap::new()),
            last_handoff_sweep: Mutex::new(None),
            reattach_at: Mutex::new(HashMap::new()),
            pump: Mutex::new(None),
        }
    }

    /// Replaces the notice port, so the runtime can announce a finished transfer and a test can run
    /// the same path without a notification service.
    #[must_use]
    pub fn with_notices(mut self, notices: Arc<dyn DownloadNotices>) -> Self {
        self.notices = notices;
        self
    }

    /// Gives the service a Usenet engine. Without one, a Usenet start answers `Unavailable`
    /// rather than failing the torrent path this session still has.
    #[must_use]
    pub fn with_usenet(mut self, engine: Arc<dyn UsenetEngine>) -> Self {
        self.usenet = Some(engine);
        self
    }

    /// Gives the service the metadata-only fetch a restart recovery needs.
    ///
    /// Without one, a job interrupted before its file list was written stays where it is rather
    /// than being guessed at; a download that never wrote its selection has nothing to add.
    #[must_use]
    pub fn with_metadata_fetch(mut self, metadata: Arc<dyn TorrentMetadataFetch>) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Gives the service the archive tools a Usenet payload may need.
    #[must_use]
    pub fn with_postprocess(mut self, tools: crate::usenet_post::PostTools) -> Self {
        self.post = Some(tools);
        self
    }

    /// Gives the service the capture port a downloaded episode's missing still is filled from.
    ///
    /// Without one the import is unchanged and every episode keeps the still its provider wrote —
    /// the state a host with no ffmpeg, and a test that is not about captures, both run in.
    #[must_use]
    pub fn with_stills(mut self, stills: Arc<crate::episode_stills::EpisodeStills>) -> Self {
        self.stills = Some(stills);
        self
    }

    /// The live engine handle for a job this process started, when it still holds one.
    ///
    /// A job started by another process has no handle here — `PG-E10-08`'s reattach is what finds it
    /// again — so `None` is "not this session's to read", not "no such job".
    #[must_use]
    pub fn handle_for(&self, job_id: i64) -> Option<TorrentHandle> {
        self.handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&job_id)
            .copied()
    }

    /// Starts the statistics pump, once, for this service.
    ///
    /// [`AcquisitionService::statistics`] is a pull: it reads the engine and writes the row, and
    /// nothing calls it on its own. An engine transfers in silence, so a session that never polls
    /// shows `downloading` at 0 % while the bytes move. The composition root starts this only
    /// after a real engine is built, so a unit test that calls `statistics` directly is
    /// unaffected. Idempotent.
    pub fn start_pump(self: &Arc<Self>) {
        let mut slot = match self.pump.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        if slot.is_some() {
            return;
        }
        let service = Arc::clone(self);
        let shutdown = CancellationToken::new();
        let token = shutdown.clone();
        let mut ticker =
            tokio::time::interval_at(Instant::now() + SAMPLE_INTERVAL, SAMPLE_INTERVAL);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = token.cancelled() => break,
                    _ = ticker.tick() => {
                        // **A tick runs in its own task so a panic cannot end the pump.** One
                        // engine call that unwinds used to stop every later update in the app
                        // silently, and a queue that never moves is the failure this pump exists
                        // to prevent.
                        let tick = Arc::clone(&service);
                        if tokio::spawn(async move { tick.sample().await }).await.is_err() {
                            warn!("a statistics tick panicked; the pump continues");
                        }
                    }
                }
            }
        });
        *slot = Some(Pump {
            shutdown,
            task: Mutex::new(Some(task)),
        });
    }

    /// Stops the statistics pump. Idempotent, and safe from a shared reference.
    pub async fn stop_pump(&self) {
        let pump = match self.pump.lock() {
            Ok(mut slot) => slot.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        let Some(pump) = pump else {
            return;
        };
        pump.shutdown.cancel();
        let task = match pump.task.lock() {
            Ok(mut slot) => slot.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        if let Some(task) = task {
            let _ = task.await;
        }
    }

    /// Asks every live job for one statistics tick.
    ///
    /// **The journal is what this reads, not the handle map.** A handle is an in-memory fact: a
    /// restart loses it, and a map that disagrees with the journal used to stop every update in the
    /// app without a single log line — the pump simply found no ids and kept ticking. The live rows
    /// are the state (FLOWS SM1), so a row is ticked because it says it is running, and a row this
    /// session holds no handle for is re-added rather than skipped.
    ///
    /// A tick the domain refuses — a job that went terminal between the read and the sample — is
    /// logged and skipped rather than ending the sweep: one finished job must not stop every other
    /// transfer's progress.
    async fn sample(self: &Arc<Self>) {
        let live = match self
            .database
            .list_download_jobs(
                unrestricted(),
                &[DownloadState::Downloading, DownloadState::Seeding],
                PageRequest::first(peregrine_storage::MAX_PAGE_ROWS),
            )
            .await
        {
            Ok(rows) => rows,
            Err(error) => {
                debug!(%error, "the live downloads could not be read, so nothing was sampled");
                return;
            }
        };
        let now = self.clock.now().as_millisecond();
        for row in live {
            if self.handle(row.id).is_err() {
                self.reattach_in_background(row, now);
                continue;
            }
            if let Err(error) = self.statistics(row.id).await {
                debug!(%error, job = row.id, "a download statistics tick was refused");
            }
        }
        // Usenet handles are this process's own; their rows have no reattach path yet, so the map
        // is the only thing that can name them.
        let usenet: Vec<i64> = self
            .usenet_handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .copied()
            .collect();
        for id in usenet {
            if let Err(error) = self.statistics(id).await {
                debug!(%error, job = id, "a download statistics tick was refused");
            }
        }
        if let Err(error) = self.sweep_handoffs().await {
            debug!(%error, "the metadata handoff sweep failed");
        }
    }

    /// Re-adds a live row this session holds no handle for, off the pump's thread.
    ///
    /// The attempt is detached because a magnet with no peers costs a bounded but long wait, and
    /// one such row must not hold the tick that describes every other one. A row is retried at most
    /// once per [`REATTACH_INTERVAL_MILLIS`], so a hopeless swarm costs a background task rather
    /// than a storm.
    fn reattach_in_background(self: &Arc<Self>, row: DownloadJobRow, now: i64) {
        {
            let mut due = self
                .reattach_at
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if due
                .get(&row.id)
                .is_some_and(|at| now.saturating_sub(*at) < REATTACH_INTERVAL_MILLIS)
            {
                return;
            }
            due.insert(row.id, now);
        }
        let service = Arc::clone(self);
        tokio::spawn(async move {
            match service.reattach_row(&row).await {
                Ok(_) => info!(job = row.id, "a live download was reattached"),
                Err(error) => {
                    debug!(%error, job = row.id, "a live download could not be reattached");
                }
            }
        });
    }

    /// Adds one torrent, giving up when the engine cannot resolve it.
    ///
    /// librqbit resolves a magnet's metadata **inside `add`**, so an unbounded call is a task held
    /// for as long as the swarm stays silent: an IPC command behind a button, or the startup
    /// reattach. A dropped add leaves no torrent in the session, so trying again later is the same
    /// action rather than a second one.
    async fn add(&self, request: &AddRequest) -> Result<TorrentHandle, AcquisitionError> {
        let Ok(handle) = tokio::time::timeout(ADD_TIMEOUT, self.engine.add(request)).await else {
            warn!("a torrent was not resolved within the add budget, so the add was dropped");
            return Err(AcquisitionError::Engine(EngineError::Failed));
        };
        Ok(handle?)
    }

    /// Inspects one release and, if the gate allows it, hands it to the engine.
    ///
    /// A gate refusal is **not** an error: the job is parked in `blocked` with its report persisted
    /// and [`Started::added`] is `false`, because a block is a decision the user has to see rather
    /// than a failure to report.
    ///
    /// # Errors
    ///
    /// [`AcquisitionError::TitleUnavailable`] when the scope cannot see the title,
    /// [`AcquisitionError::SavePath`] when the destination does not resolve,
    /// [`AcquisitionError::Duplicate`] when a live job already holds the hash,
    /// [`AcquisitionError::DiskFull`] and [`AcquisitionError::SpaceUnknown`] when the volume does
    /// not admit the job, and [`AcquisitionError::Engine`] when the engine refused the add.
    pub async fn start(&self, request: StartRequest) -> Result<Started, AcquisitionError> {
        let StartRequest {
            account,
            mut job,
            description,
            selection,
            info_hash,
        } = request;
        // The caller's selection when it narrowed one, otherwise every entry: a single release and
        // a title-wide acquisition have no reason to name one.
        let selection: Vec<usize> =
            selection.unwrap_or_else(|| (0..description.entries().len()).collect());
        // 1. The scope gate. `None` covers both "no such title" and "not for you", and the refusal
        //    below is one sentence for both (AGENTS R34).
        if self
            .database
            .title_identity(account, job.title_id)
            .await?
            .is_none()
        {
            return Err(AcquisitionError::TitleUnavailable);
        }

        // 2. The destination, before a row exists to point at it.
        let save_path = self
            .destination
            .resolve(std::path::Path::new(job.save_path.as_str()))
            .map_err(|source| AcquisitionError::SavePath { source })?;

        // 3. The gate, over the description the caller already has. Its caps come from the same
        //    floor admission uses, so a torrent too large for the volume is refused by inspection
        //    rather than after it.
        let limits = self.limits_for(&save_path);
        let report = PayloadInspector::new(limits).inspect(
            &description,
            &self.destination,
            OptIn::NotGranted,
        );
        let verdict = report.verdict();

        // 4. The row, inserted `queued`. The state literal is the repository's, so no caller can
        //    admit a job anywhere else.
        job.info_hash = Some(info_hash);
        job.save_path = text_of(&save_path)?;
        job.added_at = self.clock.now();
        let row = match self.database.create_download_job(job.clone()).await {
            Ok(row) => row,
            Err(StorageError::Conflict { .. }) => return Err(AcquisitionError::Duplicate),
            Err(error) => return Err(AcquisitionError::Storage(error)),
        };

        // **The file list is written before the handoff, not after it.** A process killed between
        // the engine's add and the selection write would leave a `metadata` row with nothing to
        // reattach; writing it here means every metadata row already carries the list its recovery
        // needs. The handoff writes it again, which is the same upsert.
        self.database
            .select_download_files(row.id, description.clone(), &selection)
            .await?;

        // 5. Admission's free-space floor, for a job that could actually run. A job the volume
        //    cannot hold stays `queued`, which is what SM1 makes of it: it was admitted to the
        //    queue and never to a transfer, so nothing has to be undone. A torrent the gate has
        //    already refused is not admission's to mask — the verdict was computed in step 3, and
        //    answering "free some space" would send the user to do that for a download that still
        //    cannot start.
        if verdict == Verdict::Allowed {
            self.admit_space(&save_path, description.total_bytes())?;
        } else if report.total_bytes() > limits.max_total_bytes() {
            // The cap is the volume's own room, so a size refusal is the same fact admission
            // reports with numbers; \`DiskFull\` is the reason a person can act on, and it leaves
            // the row \`queued\` rather than parked in \`blocked\`.
            self.admit_space(&save_path, description.total_bytes())?;
        }

        // 6. The two transitions that persist the judgement.
        self.apply(row.id, DownloadEvent::Admitted).await?;
        self.apply(row.id, DownloadEvent::Inspected(report.clone()))
            .await?;
        let row = self.reload(row.id).await?;

        if verdict != Verdict::Allowed {
            return Ok(Started {
                job: row,
                state: DownloadState::Blocked,
                verdict,
                added: false,
                selection: Vec::new(),
            });
        }

        // 7. The handoff, built from the report the transition persisted, so a verdict the engine
        //    would refuse cannot be spelled (AddRequest::new).
        self.hand_off(row.id, &save_path, &selection, &description, &report)
            .await?;

        let row = self.reload(row.id).await?;
        Ok(Started {
            state: row.state,
            job: row,
            verdict,
            added: true,
            selection,
        })
    }

    /// Starts one Usenet job: writes the NZB beside the payload, runs the gate, and hands the
    /// parsed post to the engine.
    ///
    /// The order is the torrent path's order. The NZB is stored before the row is inserted so the
    /// source column names a file that exists, the report is persisted before the engine is handed
    /// the post, and a refused job is parked in `blocked` with nothing added.
    ///
    /// # Errors
    ///
    /// The same refusals as `start`, plus `UsenetUnavailable` when no provider engine is composed.
    pub async fn start_usenet(
        &self,
        request: UsenetStartRequest,
    ) -> Result<Started, AcquisitionError> {
        let engine = self
            .usenet
            .as_ref()
            .ok_or(AcquisitionError::UsenetUnavailable)?;
        if self
            .database
            .title_identity(request.account, request.job.title_id)
            .await?
            .is_none()
        {
            return Err(AcquisitionError::TitleUnavailable);
        }
        let save_path = self
            .destination
            .resolve(std::path::Path::new(request.job.save_path.as_str()))
            .map_err(|source| AcquisitionError::SavePath { source })?;
        tokio::fs::create_dir_all(save_path.as_path()).await?;
        let nzb_path = save_path.as_path().join("source.nzb");
        tokio::fs::write(&nzb_path, &request.nzb_bytes).await?;
        let source = NonEmptyText::new(&nzb_path.to_string_lossy())
            .map_err(|_| AcquisitionError::UnsupportedSource)?;

        let description = request.document.description();
        let limits = self.limits_for(&save_path);
        let opt_in = if self.database.usenet_setting().await?.allow_archives {
            OptIn::Granted
        } else {
            OptIn::NotGranted
        };
        let report = PayloadInspector::new(limits).inspect(&description, &self.destination, opt_in);
        let verdict = report.verdict();
        let selection: Vec<usize> = (0..description.entries().len()).collect();

        let mut job = request.job;
        job.info_hash = None;
        job.protocol = Protocol::Usenet;
        job.source = DownloadSource::NzbFile(source);
        job.save_path = text_of(&save_path)?;
        job.added_at = self.clock.now();
        let row = match self.database.create_download_job(job).await {
            Ok(row) => row,
            Err(StorageError::Conflict { .. }) => return Err(AcquisitionError::Duplicate),
            Err(error) => return Err(AcquisitionError::Storage(error)),
        };
        if verdict == Verdict::Allowed {
            self.admit_space(&save_path, description.total_bytes())?;
        }
        self.apply(row.id, DownloadEvent::Admitted).await?;
        self.apply(row.id, DownloadEvent::Inspected(report.clone()))
            .await?;
        let row = self.reload(row.id).await?;
        if verdict != Verdict::Allowed {
            return Ok(Started {
                job: row,
                state: DownloadState::Blocked,
                verdict,
                added: false,
                selection: Vec::new(),
            });
        }

        let add = AddNzbRequest::new(
            request.document,
            save_path,
            FileSelection::new(selection.iter().copied()),
            request.servers,
            &report,
        )?;
        let handle = engine.add(add).await?;
        self.usenet_handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(row.id, handle);
        self.apply(row.id, DownloadEvent::EngineAccepted).await?;
        self.database
            .select_download_files(row.id, description, &selection)
            .await?;
        let row = self.reload(row.id).await?;
        Ok(Started {
            state: row.state,
            job: row,
            verdict,
            added: true,
            selection,
        })
    }

    /// Applies one event through the repository's single writer, mapping its refusals.
    async fn apply(
        &self,
        id: i64,
        event: DownloadEvent,
    ) -> Result<DownloadState, AcquisitionError> {
        let name = event.to_string();
        match self.database.transition_download_job(id, event).await {
            Ok(Some(state)) => {
                // The one moment a job cannot announce for itself: the state is the scheduler's,
                // and nothing on the completion path awaits its handle.
                if matches!(state, DownloadState::Completed | DownloadState::Failed) {
                    // The download is the subject, not the title: the queue is the surface a click
                    // opens, and a row that named a title would point at a different record.
                    self.notices
                        .finished(id, state == DownloadState::Failed)
                        .await;
                }
                Ok(state)
            }
            Ok(None) => Err(AcquisitionError::NotFound),
            Err(DownloadTransitionError::Rejected(source)) => {
                Err(AcquisitionError::TransitionRefused {
                    event: name,
                    source,
                })
            }
            Err(DownloadTransitionError::Storage(error)) => Err(AcquisitionError::Storage(error)),
        }
    }

    /// The inspection caps for a destination: the volume's free space above the reservation.
    ///
    /// An unreadable volume cannot answer a size question, so it gets no size cap and admission
    /// reports the refusal instead of this gate inventing a number.
    fn limits_for(&self, save_path: &SafePath) -> InspectionLimits {
        match self.volume.available_bytes(save_path.as_path()) {
            // A volume with room above its reservation caps a release at exactly that room. One at
            // or below the reservation is admission's to refuse with `DiskFull`: a size cap of
            // zero here would answer a question the volume has already answered, and with the
            // wrong reason.
            Ok(available) if available > self.free_space_floor => {
                InspectionLimits::with_available(available, self.free_space_floor)
            }
            Ok(_) | Err(_) => InspectionLimits::new(InspectionLimits::DEFAULT_MAX_FILES, u64::MAX),
        }
    }

    /// The gate's verdict for a candidate, without creating a job.
    ///
    /// `start` persists a row even for a refusal, so a caller walking several candidates asks
    /// this first: a release the gate will not pass then costs a candidate rather than a blocked
    /// row.
    ///
    /// # Errors
    ///
    /// [`AcquisitionError::SavePath`] when the destination does not resolve.
    pub fn verdict_for(
        &self,
        save_path: &str,
        description: &PayloadDescription,
    ) -> Result<Verdict, AcquisitionError> {
        let resolved = self
            .destination
            .resolve(std::path::Path::new(save_path))
            .map_err(|source| AcquisitionError::SavePath { source })?;
        Ok(PayloadInspector::new(self.limits_for(&resolved))
            .inspect(description, &self.destination, OptIn::NotGranted)
            .verdict())
    }

    /// Checks the free-space floor before an add.
    ///
    /// Refusing *before* `engine.add` is the point: a torrent that cannot fit must not have begun
    /// writing, and SM1 has no edge from `downloading` back to "never started". The job has already
    /// been admitted to the queue at this point, so a refusal leaves it `queued` with nothing to
    /// undo.
    fn admit_space(
        &self,
        save_path: &SafePath,
        required_bytes: u64,
    ) -> Result<(), AcquisitionError> {
        let available = match self.volume.available_bytes(save_path.as_path()) {
            Ok(available) => available,
            Err(source) => return Err(AcquisitionError::SpaceUnknown { source }),
        };
        let usable = available.saturating_sub(self.free_space_floor);
        if usable >= required_bytes {
            return Ok(());
        }
        Err(AcquisitionError::DiskFull {
            required_bytes,
            available_bytes: usable,
        })
    }

    /// Hands one allowed torrent to the engine, records the handle, and stamps what the session
    /// now holds.
    ///
    /// This is the handoff and nothing else: the caller has already admitted the job to the volume
    /// and moved it to `metadata`, the state `download.rs` describes as "being handed to the
    /// engine". It is shared by a first add and by the add a consent completes, because F6 merges
    /// both branches back into the same `add` step. Identity and source come from the row the
    /// caller has just moved, so no caller can hand the engine a torrent the database does not
    /// record it as adding.
    ///
    /// # Errors
    ///
    /// [`AcquisitionError::UnsupportedSource`] when the row names a source this build cannot hand
    /// the engine or records no identity, [`AcquisitionError::AddRefused`] when the report is not
    /// `Allowed` — which the caller has already checked, so it is an invariant rather than a path —
    /// and the engine's own refusals.
    async fn hand_off(
        &self,
        id: i64,
        save_path: &SafePath,
        selection: &[usize],
        description: &PayloadDescription,
        report: &SafetyReport,
    ) -> Result<TorrentHandle, AcquisitionError> {
        let row = self.reload(id).await?;
        let source = match row.source.as_ref() {
            Some(DownloadSource::Magnet(uri)) => TorrentSource::Magnet(
                NonEmptyText::new(uri.as_str()).map_err(|_| AcquisitionError::UnsupportedSource)?,
            ),
            // The engine is handed contents and never a path outside the download root
            // (ADR 0005), so a row that stores a path needs the adapter that reads it.
            Some(DownloadSource::TorrentFile(_) | DownloadSource::NzbFile(_)) | None => {
                return Err(AcquisitionError::UnsupportedSource);
            }
        };
        let request = AddRequest::new(
            row.info_hash.ok_or(AcquisitionError::UnsupportedSource)?,
            source,
            save_path.clone(),
            FileSelection::new(selection.iter().copied()),
            true,
            AddIntent::Download,
            report,
        )?;
        let handle = self.add(&request).await?;
        self.handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, handle);
        self.apply(id, DownloadEvent::EngineAccepted).await?;
        self.database
            .select_download_files(id, description.clone(), selection)
            .await?;
        let engine_state = self.engine.state(handle).await?;
        self.database
            .record_download_engine_state(id, engine_state.as_bytes())
            .await?;
        Ok(handle)
    }

    /// Re-inspects the same description with the user's opt-in, lifts the block, and completes the
    /// add.
    ///
    /// Separate from [`AcquisitionService::start`] because a grant covers **one torrent and one
    /// prompt** (`peregrine_domain::inspect`): the second inspection is the only thing that can
    /// produce a report a blocked job may advance on, and the domain refuses a consent that did not
    /// re-inspect rather than believing the caller.
    ///
    /// F6 merges this branch back into the `add` step the other one takes — the user's confirmation
    /// is the last thing that happens before the transfer starts — so this finishes the handoff
    /// instead of leaving the job in `metadata`. `metadata` means a handoff is under way
    /// (`download.rs`), and SM1's only other edge out of it is a metadata timeout, so a job parked
    /// there is a download that never began.
    ///
    /// # Errors
    ///
    /// [`AcquisitionError::OptInStillRequired`] and [`AcquisitionError::NoOverride`] when the
    /// re-inspection did not change the verdict — a traversal, a link, or a program re-inspects to
    /// `Blocked` however many times a user agrees, and a report built with `OptIn::Granted` that
    /// still requires the opt-in has not been re-inspected at all. [`AcquisitionError::DiskFull`]
    /// is checked **before** the block is lifted, so a full volume leaves the job `blocked` — where
    /// the same consent can be given again once there is room — rather than in `metadata`.
    pub async fn consent(
        &self,
        id: i64,
        description: &PayloadDescription,
    ) -> Result<DownloadState, AcquisitionError> {
        let row = self.reload(id).await?;
        let save_path = self
            .destination
            .resolve(std::path::Path::new(row.save_path.as_str()))
            .map_err(|source| AcquisitionError::SavePath { source })?;
        let limits = self.limits_for(&save_path);
        let report =
            PayloadInspector::new(limits).inspect(description, &self.destination, OptIn::Granted);
        if report.total_bytes() > limits.max_total_bytes() {
            self.admit_space(&save_path, description.total_bytes())?;
        }
        match report.verdict() {
            Verdict::Allowed => {}
            Verdict::OptInRequired => return Err(AcquisitionError::OptInStillRequired),
            Verdict::Blocked => return Err(AcquisitionError::NoOverride),
        }

        self.admit_space(&save_path, description.total_bytes())?;

        self.apply(id, DownloadEvent::Consented(report.clone()))
            .await?;
        let selection: Vec<usize> = (0..description.entries().len()).collect();
        self.hand_off(id, &save_path, &selection, description, &report)
            .await?;
        Ok(self.reload(id).await?.state)
    }

    /// Stops one job's transfer, keeping everything it has written.
    ///
    /// A resume continues from those bytes. The engine's pause is idempotent and the job's is not: a
    /// second `pause` on a paused job is [`AcquisitionError::TransitionRefused`], so a UI toggle dispatched
    /// twice is told rather than silently accepted.
    ///
    /// # Errors
    ///
    /// [`AcquisitionError::NotLive`] when this process holds no handle for the job — a job started
    /// before a restart is resumed, not paused — and the engine and domain refusals.
    pub async fn pause(&self, id: i64) -> Result<DownloadState, AcquisitionError> {
        if let Some(handle) = self.usenet_handle(id) {
            let engine = self
                .usenet
                .as_ref()
                .ok_or(AcquisitionError::UsenetUnavailable)?;
            let state = self.apply(id, DownloadEvent::Paused).await?;
            engine.pause(handle).await?;
            return Ok(state);
        }
        let handle = self.handle(id).map_err(|_| AcquisitionError::NotLive)?;
        let state = self.apply(id, DownloadEvent::Paused).await?;
        self.engine.pause(handle).await?;
        Ok(state)
    }

    /// Restarts one stopped transfer from the bytes already on disk.
    ///
    /// The engine call comes first here, so a resume the engine cannot carry out cannot leave a row
    /// that says `downloading` while nothing transfers — which is the failure a user cannot
    /// diagnose. A job with no handle is reattached from its row first: a `metadata` row completes
    /// its handoff, and a `paused` one resumes in place. A resume of a running job is still
    /// [`AcquisitionError::TransitionRefused`].
    pub async fn resume(&self, id: i64) -> Result<DownloadState, AcquisitionError> {
        if let Some(handle) = self.usenet_handle(id) {
            let engine = self
                .usenet
                .as_ref()
                .ok_or(AcquisitionError::UsenetUnavailable)?;
            engine.resume(handle).await?;
            return self.apply(id, DownloadEvent::Resumed).await;
        }
        // **A restart loses every handle, and Resume is the action that completes the handoff.**
        // The row is the state: its magnet, report, and file list (or the metadata fetched again
        // from that magnet) are enough to add it with reattach semantics. A row that is still
        // `metadata` is accepted by the engine and moves to `downloading`; a row that was
        // `paused` resumes from the bytes already on disk.
        let Ok(handle) = self.handle(id) else {
            let row = self
                .database
                .download_job(unrestricted(), id)
                .await?
                .ok_or(AcquisitionError::NotFound)?;
            let _ = self.reattach_row(&row).await?;
            return match row.state {
                DownloadState::Metadata => self.apply(id, DownloadEvent::EngineAccepted).await,
                DownloadState::Paused => self.apply(id, DownloadEvent::Resumed).await,
                // A downloading row's handle was lost; the add above restarts it, and the state
                // already says what it is.
                DownloadState::Downloading => Ok(DownloadState::Downloading),
                _ => Err(AcquisitionError::NotLive),
            };
        };
        self.engine.resume(handle).await?;
        self.apply(id, DownloadEvent::Resumed).await
    }

    /// Asks the torrent's swarm for peers again, now.
    ///
    /// **The engine has no reannounce call**, and a torrent announces when its task starts: a live
    /// transfer is paused and unpaused, which restarts that task, and a row this process holds no
    /// handle for — a restart's leftover, or a handoff that never finished — is re-added from its
    /// stored magnet with the bytes already on disk. Nothing about the row changes, which is why no
    /// event is applied: a transfer that found peers is still `downloading`.
    ///
    /// A paused transfer is refused rather than served: restarting its task is a Resume, and this
    /// call must not unpause what a person stopped. The stall guard uses the same two moves, so a
    /// person asking by hand spends the attempts again rather than inheriting the guard's.
    ///
    /// # Errors
    ///
    /// [`AcquisitionError::NotLive`] for a row a peer search cannot move, and the engine, storage,
    /// and metadata refusals the re-add can produce.
    pub async fn find_peers(&self, id: i64) -> Result<DownloadState, AcquisitionError> {
        let row = self.reload(id).await?;
        if row.protocol != Protocol::Torrent {
            return Err(AcquisitionError::NotLive);
        }
        match row.state {
            DownloadState::Metadata => {
                self.reattach_row(&row).await?;
                self.apply(id, DownloadEvent::EngineAccepted).await
            }
            DownloadState::Downloading | DownloadState::Seeding => {
                match self.handle(id) {
                    Ok(handle) => {
                        self.forget_stall(id);
                        self.announce_again(handle).await?;
                    }
                    // The handle is gone but the row is live: a restart's leftover, which the
                    // reattach path adds again with the bytes this session already has.
                    Err(_) => {
                        self.reattach_row(&row).await?;
                    }
                }
                Ok(row.state)
            }
            _ => Err(AcquisitionError::NotLive),
        }
    }

    /// Restarts a torrent's announcer, which is the closest the engine has to a reannounce.
    async fn announce_again(&self, handle: TorrentHandle) -> Result<(), AcquisitionError> {
        self.engine.pause(handle).await?;
        self.engine.resume(handle).await?;
        Ok(())
    }

    /// Forgets what the stall guard has spent on one row.
    fn forget_stall(&self, id: i64) {
        self.stalls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id);
    }

    /// Watches one transfer for a stall and acts on it.
    ///
    /// The engine is a pull: a peerless torrent never changes its own row, and a swarm that is
    /// asleep when the add happened is never asked again. This is the caller that was missing —
    /// a stall past [`STALL_GRACE_MILLIS`] re-announces, and a row that has spent
    /// [`STALL_ATTEMPTS`] announce attempts fails with the reason the queue can draw.
    ///
    /// It is fed by the statistics tick, which has already read the engine, so a tick still costs
    /// one read per transfer. Only a `downloading` row is watched: a paused one is stopped on
    /// purpose and restarting its task would undo that, and a seeding one having nobody leeching
    /// from it is not a stall in the sense this guards against.
    async fn guard_stall(
        &self,
        id: i64,
        handle: TorrentHandle,
        state: DownloadState,
        health: TorrentHealth,
        now: i64,
    ) -> Result<(), AcquisitionError> {
        if state != DownloadState::Downloading {
            self.forget_stall(id);
            return Ok(());
        }
        let stalled = matches!(health, TorrentHealth::NoPeers | TorrentHealth::Stalled);
        let action = self
            .stalls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(id)
            .or_default()
            .observe(now, stalled);
        match action {
            StallAction::Moving | StallAction::Waiting => Ok(()),
            StallAction::Reannounce => {
                info!(
                    job = id,
                    "a stalled download is asking its swarm for peers again"
                );
                self.announce_again(handle).await
            }
            StallAction::GiveUp => self.abandon(id, handle).await,
        }
    }

    /// Fails a stalled row with the sentence that says why, and stops reading its transfer.
    ///
    /// The reason is written first, because the transition is what makes the row terminal and a
    /// terminal row keeps the reason it stopped for. The engine handle is dropped so the pump stops
    /// reading a transfer that will not move; the bytes on disk are untouched, and the row's own
    /// magnet is what a later download re-adds.
    async fn abandon(&self, id: i64, handle: TorrentHandle) -> Result<(), AcquisitionError> {
        let message = format!(
            "no peers answered after {STALL_ATTEMPTS} attempts; the release may have no seeders left"
        );
        if let Err(error) = self
            .database
            .record_download_failure(id, "NO_PEERS", &message)
            .await
        {
            warn!(%error, job = id, "the reason a stalled download stopped could not be written");
        }
        let _ = self.engine.pause(handle).await;
        self.handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id);
        self.forget_stall(id);
        self.apply(id, DownloadEvent::EngineFailed).await?;
        Ok(())
    }

    /// Retries the handoffs a previous process left in `metadata`.
    ///
    /// The state machine has one ordinary edge out of `metadata` — the engine's acceptance — and a
    /// handoff that lost its process had nothing walking it: a real row sat there for hours, through
    /// two restarts, with no peer and no error. The sweep runs on a timer rather than once at
    /// startup because a magnet's metadata may not arrive on the first attempt, and it gives up
    /// through the domain's own `metadata timed out` edge so the queue says why instead of waiting.
    async fn sweep_handoffs(&self) -> Result<(), AcquisitionError> {
        let now = self.clock.now().as_millisecond();
        {
            let mut last = self
                .last_handoff_sweep
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(at) = *last
                && now.saturating_sub(at) < HANDOFF_SWEEP_MILLIS
            {
                return Ok(());
            }
            *last = Some(now);
        }
        let rows = self
            .database
            .list_download_jobs(
                unrestricted(),
                &[DownloadState::Metadata],
                PageRequest::first(peregrine_storage::MAX_PAGE_ROWS),
            )
            .await?;
        for row in rows {
            if row.protocol != Protocol::Torrent {
                continue;
            }
            let action = self
                .stalls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entry(row.id)
                .or_default()
                .observe(now, true);
            match action {
                StallAction::Reannounce => match self.reattach_row(&row).await {
                    Ok(_) => {
                        if let Err(error) = self.apply(row.id, DownloadEvent::EngineAccepted).await
                        {
                            debug!(%error, job = row.id, "a recovered handoff could not be marked downloading");
                        }
                    }
                    Err(error) => {
                        debug!(%error, job = row.id, "a metadata handoff could not be retried");
                    }
                },
                StallAction::GiveUp => {
                    let message = format!(
                        "the torrent's metadata did not arrive after {STALL_ATTEMPTS} attempts; the release may have no seeders left"
                    );
                    if let Err(error) = self
                        .database
                        .record_download_failure(row.id, "NO_PEERS", &message)
                        .await
                    {
                        warn!(%error, job = row.id, "the reason a stalled handoff stopped could not be written");
                    }
                    self.forget_stall(row.id);
                    if let Err(error) = self.apply(row.id, DownloadEvent::MetadataTimedOut).await {
                        debug!(%error, job = row.id, "a timed-out handoff could not be failed");
                    }
                }
                StallAction::Moving | StallAction::Waiting => {}
            }
        }
        Ok(())
    }

    /// Removes one job, from any non-terminal state.
    ///
    /// `FLOWS` SM1 gives `removed` an edge from every state a user can still be waiting in, so a job
    /// parked behind the active set is cancellable too. The engine's removal is idempotent and runs
    /// only for a job this process actually added; the domain's transition is what refuses a second
    /// removal after the fact.
    pub async fn remove(&self, id: i64) -> Result<DownloadState, AcquisitionError> {
        let usenet_handle = self
            .usenet_handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id);
        if let Some(handle) = usenet_handle
            && let Some(engine) = self.usenet.as_ref()
        {
            engine.remove(handle).await?;
        }
        let handle = self
            .handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id);
        if let Some(handle) = handle {
            self.engine.remove(handle).await?;
        }
        self.written_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id);
        self.apply(id, DownloadEvent::Removed).await
    }

    /// Clears one finished queue entry -- `failed` or `removed`, never a live one.
    ///
    /// The bytes already written are untouched: this removes the record, not the files. A live row
    /// is [`AcquisitionError::NotFinished`], and an id nothing holds is `Ok(false)`, so a double
    /// click on an entry that is already gone is harmless.
    pub async fn dismiss(&self, id: i64) -> Result<bool, AcquisitionError> {
        if self.database.delete_download_job(id).await? {
            self.handles
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&id);
            self.written_at
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&id);
            return Ok(true);
        }
        // Nothing went: either the row is not there, or it is not finished. The two are different
        // answers to the caller, so a live row is a refusal rather than a silent no-op.
        match self.database.download_job(unrestricted(), id).await? {
            None => Ok(false),
            Some(_) => Err(AcquisitionError::NotFinished),
        }
    }

    /// Clears every finished queue entry, answering how many went.
    pub async fn clear_finished(&self) -> Result<u64, AcquisitionError> {
        Ok(self.database.clear_finished_download_jobs().await?)
    }

    /// Persists one coalesced statistics tick and publishes it at the same budget.
    ///
    /// The domain's [`DownloadEvent::Progress`] is applied first, so a tick that arrives after a job
    /// finished is [`AcquisitionError::TransitionRefused`] rather than a write to a terminal row
    /// (`FLOWS` SM1). A tick inside the one-second budget is `Ok(None)`: nothing was read from the
    /// engine, nothing was written, and nothing was published, which is what coalescing means and
    /// not a failure.
    ///
    /// The full disk is a *level*, so it is checked on every accepted tick: free space below the
    /// floor pauses the transfer through the same event `ARCHITECTURE` §15 names, and the resume
    /// affordance is the ordinary one.
    ///
    /// # Errors
    ///
    /// [`AcquisitionError::TransitionRefused`] for a tick after a terminal state, and the engine and
    /// storage refusals the reads and writes can produce.
    pub async fn statistics(
        &self,
        id: i64,
    ) -> Result<Option<DownloadStatistics>, AcquisitionError> {
        if let Some(handle) = self.usenet_handle(id) {
            return self.statistics_usenet(id, handle).await;
        }
        let handle = self.handle(id)?;
        let now = self.clock.now();
        {
            let written = self
                .written_at
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(last) = written.get(&id)
                && now.as_millisecond().saturating_sub(last.as_millisecond()) < TICK_INTERVAL_MILLIS
            {
                return Ok(None);
            }
        }

        let row = self.reload(id).await?;
        self.apply(id, DownloadEvent::Progress).await?;
        let stats = self.engine.stats(handle).await?;
        let statistics = statistics_of(&stats);

        if let Ok(available) = self
            .volume
            .available_bytes(std::path::Path::new(row.save_path.as_str()))
            && available < self.free_space_floor
        {
            self.apply(id, DownloadEvent::DiskFull).await?;
            self.engine.pause(handle).await?;
        }

        if !self
            .database
            .record_download_statistics(id, statistics)
            .await?
        {
            return Err(AcquisitionError::NotFound);
        }
        self.written_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, now);
        self.bus.publish(Event::new(
            DOWNLOAD_STATS,
            id.to_string(),
            payload(id, &statistics),
        ));
        // A payload that has reached its whole size is finished, and the domain's two edges turn
        // that fact into the state the queue and the title page read: downloading -> seeding ->
        // completed. Nothing else in this build emits either event, so without this hook a finished
        // transfer stays downloading at 100 % for ever.
        if statistics.progress_permille >= 1_000 && row.state == DownloadState::Downloading {
            self.finish_download(id, handle).await?;
        } else if let Err(error) = self
            .guard_stall(id, handle, row.state, stats.health(), now.as_millisecond())
            .await
        {
            // The tick's own write and publish already happened, so a peer search that failed is
            // logged rather than reported as a statistics read that could not be taken.
            debug!(%error, job = id, "the stall guard could not act");
        }
        Ok(Some(statistics))
    }

    /// Re-inspects a blocked Usenet job with the archive opt-in and hands it to the engine.
    ///
    /// The NZB is read back from the job's own source file, not from a caller, so the consent
    /// cannot describe a different post. The setting is written first, so a later acquisition of
    /// the same kind passes the gate without a second prompt.
    ///
    /// # Errors
    ///
    /// The same refusals as consent for a torrent, plus a missing source and a parse failure.
    pub async fn consent_usenet(
        &self,
        id: i64,
        servers: Vec<ServerConfig>,
    ) -> Result<DownloadState, AcquisitionError> {
        let engine = self
            .usenet
            .as_ref()
            .ok_or(AcquisitionError::UsenetUnavailable)?;
        let row = self.reload(id).await?;
        let Some(DownloadSource::NzbFile(path)) = row.source.as_ref() else {
            return Err(AcquisitionError::UnsupportedSource);
        };
        let bytes = tokio::fs::read(path.as_str()).await?;
        let document =
            NzbDocument::parse(&bytes).map_err(|_| AcquisitionError::UnsupportedSource)?;
        let description = document.description();
        let save_path = self
            .destination
            .resolve(std::path::Path::new(row.save_path.as_str()))
            .map_err(|source| AcquisitionError::SavePath { source })?;
        let limits = self.limits_for(&save_path);
        let report =
            PayloadInspector::new(limits).inspect(&description, &self.destination, OptIn::Granted);
        match report.verdict() {
            Verdict::Allowed => {}
            Verdict::OptInRequired => return Err(AcquisitionError::OptInStillRequired),
            Verdict::Blocked => return Err(AcquisitionError::NoOverride),
        }
        self.admit_space(&save_path, description.total_bytes())?;
        let mut setting = self.database.usenet_setting().await?;
        setting.allow_archives = true;
        self.database
            .set_usenet_setting(setting, self.clock.now())
            .await?;
        self.apply(id, DownloadEvent::Consented(report.clone()))
            .await?;
        let selection: Vec<usize> = (0..description.entries().len()).collect();
        let add = AddNzbRequest::new(
            document,
            save_path,
            FileSelection::new(selection.iter().copied()),
            servers,
            &report,
        )?;
        let handle = engine.add(add).await?;
        self.usenet_handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, handle);
        self.apply(id, DownloadEvent::EngineAccepted).await?;
        self.database
            .select_download_files(id, description, &selection)
            .await?;
        Ok(self.reload(id).await?.state)
    }

    /// Completes a handoff this session did not witness.
    ///
    /// **The row is the state.** A job interrupted between its safety report and the engine's
    /// acceptance is left `metadata` with a magnet and a persisted report; one that lost its handle
    /// with a restart is `downloading` or `paused`. Both are added again with reattach semantics, so
    /// the bytes already on disk are verified rather than fetched twice.
    ///
    /// **A file list is not optional.** The engine's add names a selection, and a job that died before
    /// its rows were written has none. The stored magnet is then read through the metadata-only fetch
    /// this session was given, the hash is checked against the row, and every entry is selected —
    /// the same list the gate judged, because the report is the row's own. A session without that
    /// fetch leaves the row for the next start rather than guessing at a selection.
    async fn reattach_row(&self, row: &DownloadJobRow) -> Result<TorrentHandle, AcquisitionError> {
        let Some(info_hash) = row.info_hash else {
            return Err(AcquisitionError::NotFound);
        };
        let Some(report) = row.safety_report.clone() else {
            return Err(AcquisitionError::NotFound);
        };
        let source = match row.source.as_ref() {
            Some(DownloadSource::Magnet(uri)) => TorrentSource::Magnet(
                NonEmptyText::new(uri.as_str()).map_err(|_| AcquisitionError::UnsupportedSource)?,
            ),
            Some(DownloadSource::TorrentFile(path)) => {
                TorrentSource::TorrentFile(tokio::fs::read(path.as_str()).await?)
            }
            Some(DownloadSource::NzbFile(_)) | None => {
                return Err(AcquisitionError::UnsupportedSource);
            }
        };
        let save_path = self
            .destination
            .resolve(std::path::Path::new(row.save_path.as_str()))
            .map_err(|source| AcquisitionError::SavePath { source })?;
        let mut files = self.database.download_files(unrestricted(), row.id).await?;
        if !files.iter().any(|file| file.selected) {
            let Some(metadata) = self.metadata.as_ref() else {
                return Err(AcquisitionError::NotLive);
            };
            let fetched =
                tokio::time::timeout(METADATA_RECOVERY_TIMEOUT, metadata.fetch_metadata(&source))
                    .await
                    .map_err(|_| AcquisitionError::Engine(EngineError::Failed))??;
            if fetched.info_hash() != info_hash {
                // The magnet no longer names the torrent the report judged. Adding it would be a
                // different download, so it is refused rather than re-inspected here.
                return Err(AcquisitionError::Engine(EngineError::Failed));
            }
            let description = fetched.description().clone();
            let selection: Vec<usize> = (0..description.entries().len()).collect();
            if selection.is_empty() {
                return Err(AcquisitionError::UnsupportedSource);
            }
            self.database
                .select_download_files(row.id, description, &selection)
                .await?;
            files = self.database.download_files(unrestricted(), row.id).await?;
        }
        let selection: Vec<usize> = files
            .iter()
            .filter(|file| file.selected)
            .filter_map(|file| usize::try_from(file.file_index).ok())
            .collect();
        if selection.is_empty() {
            return Err(AcquisitionError::UnsupportedSource);
        }
        let engine_state = self
            .database
            .download_engine_state(unrestricted(), row.id)
            .await
            .ok()
            .flatten()
            .filter(|bytes| !bytes.is_empty())
            .map(EngineState::new);
        let request = AddRequest::new(
            info_hash,
            source,
            save_path,
            FileSelection::new(selection),
            true,
            AddIntent::Reattach {
                state: engine_state,
            },
            &report,
        )?;
        let handle = self.add(&request).await?;
        self.handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(row.id, handle);
        if let Ok(state) = self.engine.state(handle).await {
            let _ = self
                .database
                .record_download_engine_state(row.id, state.as_bytes())
                .await;
        }
        Ok(handle)
    }

    /// Completes every `metadata` job a previous process left behind.
    ///
    /// The one state that is neither terminal nor live: SM1's only ordinary edge out of it is the
    /// engine's acceptance, so a row found there after a restart is a handoff that never finished.
    /// One row's failure is logged and does not stop the rest, and a successful one is moved to
    /// `downloading` so the pump starts reporting it.
    ///
    /// # Errors
    ///
    /// Storage refusals while listing the rows.
    pub async fn recover_metadata_jobs(&self) -> Result<usize, AcquisitionError> {
        let jobs = self
            .database
            .list_download_jobs(
                unrestricted(),
                &[DownloadState::Metadata],
                PageRequest::first(200),
            )
            .await?;
        let mut recovered = 0;
        for row in jobs {
            if row.protocol != Protocol::Torrent {
                continue;
            }
            match self.reattach_row(&row).await {
                Ok(_) => match self.apply(row.id, DownloadEvent::EngineAccepted).await {
                    Ok(_) => recovered += 1,
                    Err(error) => {
                        warn!(job = row.id, %error, "a recovered download could not be marked downloading");
                    }
                },
                Err(error) => {
                    warn!(job = row.id, %error, "a metadata download could not be resumed");
                }
            }
        }
        Ok(recovered)
    }

    /// Reattaches the torrent jobs a previous process left live.
    ///
    /// The row's magnet or torrent file, safety report, selected files, and save path are enough
    /// to add it again with `AddIntent::Reattach`: the engine verifies the payload already on disk
    /// and transfers only what fails, so a job interrupted at 62 percent resumes rather than
    /// starting over. A job whose source, report, or selection is gone is left alone; the next
    /// statistics tick will not invent progress for it.
    ///
    /// # Errors
    ///
    /// Storage refusals while listing the jobs. One engine add failure is logged and does not
    /// prevent the remaining rows from being reattached.
    pub async fn reattach_torrents(&self) -> Result<usize, AcquisitionError> {
        let jobs = self
            .database
            .list_download_jobs(
                unrestricted(),
                &[DownloadState::Downloading, DownloadState::Paused],
                PageRequest::first(1_000),
            )
            .await?;
        let mut reattached = 0;
        for row in jobs {
            if row.protocol != Protocol::Torrent {
                continue;
            }
            let Some(info_hash) = row.info_hash else {
                continue;
            };
            let Some(report) = row.safety_report.clone() else {
                continue;
            };
            let source = match row.source.as_ref() {
                Some(DownloadSource::Magnet(uri)) => {
                    let Ok(uri) = NonEmptyText::new(uri.as_str()) else {
                        continue;
                    };
                    TorrentSource::Magnet(uri)
                }
                Some(DownloadSource::TorrentFile(path)) => {
                    let Ok(bytes) = tokio::fs::read(path.as_str()).await else {
                        continue;
                    };
                    TorrentSource::TorrentFile(bytes)
                }
                Some(DownloadSource::NzbFile(_)) | None => continue,
            };
            let Ok(save_path) = self
                .destination
                .resolve(std::path::Path::new(row.save_path.as_str()))
            else {
                continue;
            };
            let Ok(files) = self.database.download_files(unrestricted(), row.id).await else {
                continue;
            };
            let selection: Vec<usize> = files
                .iter()
                .filter(|file| file.selected)
                .filter_map(|file| usize::try_from(file.file_index).ok())
                .collect();
            if selection.is_empty() {
                continue;
            }
            let state = self
                .database
                .download_engine_state(unrestricted(), row.id)
                .await
                .ok()
                .flatten()
                .filter(|bytes| !bytes.is_empty())
                .map(EngineState::new);
            let Ok(request) = AddRequest::new(
                info_hash,
                source,
                save_path,
                FileSelection::new(selection),
                true,
                AddIntent::Reattach { state },
                &report,
            ) else {
                continue;
            };
            match self.add(&request).await {
                Ok(handle) => {
                    if row.state == DownloadState::Paused {
                        let _ = self.engine.pause(handle).await;
                    }
                    self.handles
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .insert(row.id, handle);
                    if let Ok(engine_state) = self.engine.state(handle).await {
                        let _ = self
                            .database
                            .record_download_engine_state(row.id, engine_state.as_bytes())
                            .await;
                    }
                    reattached += 1;
                }
                Err(error) => {
                    warn!(job = row.id, %error, "a torrent job could not be reattached");
                }
            }
        }
        Ok(reattached)
    }

    /// Reattaches the Usenet jobs a previous process left live.
    ///
    /// The NZB and the persisted safety report are the state; there is no engine blob to recover.
    /// A job that was paused is added and paused again. A job whose source is gone is left alone
    /// and will fail on the next tick rather than being silently completed.
    ///
    /// # Errors
    ///
    /// Storage refusals, a missing or unreadable NZB, and the engine's own refusals.
    pub async fn reattach_usenet(
        &self,
        servers: Vec<ServerConfig>,
    ) -> Result<usize, AcquisitionError> {
        let engine = self
            .usenet
            .as_ref()
            .ok_or(AcquisitionError::UsenetUnavailable)?;
        if servers.is_empty() {
            return Ok(0);
        }
        let jobs = self
            .database
            .list_download_jobs(
                unrestricted(),
                &[DownloadState::Downloading, DownloadState::Paused],
                PageRequest::first(1_000),
            )
            .await?;
        let mut reattached = 0;
        for row in jobs {
            if row.protocol != Protocol::Usenet {
                continue;
            }
            let Some(DownloadSource::NzbFile(path)) = row.source.as_ref() else {
                continue;
            };
            let Ok(bytes) = tokio::fs::read(path.as_str()).await else {
                continue;
            };
            let Ok(document) = NzbDocument::parse(&bytes) else {
                continue;
            };
            let Some(report) = row.safety_report.clone() else {
                continue;
            };
            let Ok(save_path) = self
                .destination
                .resolve(std::path::Path::new(row.save_path.as_str()))
            else {
                continue;
            };
            let selection: Vec<usize> = (0..document.files().len()).collect();
            let Ok(add) = AddNzbRequest::new(
                document,
                save_path,
                FileSelection::new(selection),
                servers.clone(),
                &report,
            ) else {
                continue;
            };
            let handle = engine.add(add).await?;
            if row.state == DownloadState::Paused {
                engine.pause(handle).await?;
            }
            self.usenet_handles
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(row.id, handle);
            reattached += 1;
        }
        Ok(reattached)
    }

    /// The Usenet handle this process holds for a job, if any.
    fn usenet_handle(&self, id: i64) -> Option<UsenetHandle> {
        self.usenet_handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&id)
            .copied()
    }

    /// Persists one Usenet tick at the same one-second budget as a torrent tick.
    async fn statistics_usenet(
        &self,
        id: i64,
        handle: UsenetHandle,
    ) -> Result<Option<DownloadStatistics>, AcquisitionError> {
        let engine = self
            .usenet
            .as_ref()
            .ok_or(AcquisitionError::UsenetUnavailable)?;
        let now = self.clock.now();
        {
            let written = self
                .written_at
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(last) = written.get(&id)
                && now.as_millisecond().saturating_sub(last.as_millisecond()) < TICK_INTERVAL_MILLIS
            {
                return Ok(None);
            }
        }
        let row = self.reload(id).await?;
        self.apply(id, DownloadEvent::Progress).await?;
        let stats = engine.stats(handle).await?;
        let statistics = DownloadStatistics {
            downloaded_bytes: stats.downloaded_bytes,
            uploaded_bytes: 0,
            progress_permille: stats.progress_permille(),
            download_rate_bps: stats.rate_bps,
            upload_rate_bps: 0,
            peers_connected: 0,
            peers_swarm: 0,
            eta_seconds: stats.eta_seconds,
            ratio: None,
        };
        if let Ok(available) = self
            .volume
            .available_bytes(std::path::Path::new(row.save_path.as_str()))
            && available < self.free_space_floor
        {
            self.apply(id, DownloadEvent::DiskFull).await?;
            engine.pause(handle).await?;
        }
        if !self
            .database
            .record_download_statistics(id, statistics)
            .await?
        {
            return Err(AcquisitionError::NotFound);
        }
        self.written_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, now);
        self.bus.publish(Event::new(
            DOWNLOAD_STATS,
            id.to_string(),
            payload(id, &statistics),
        ));
        if stats.error.is_some() {
            self.usenet_handles
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&id);
            self.apply(id, DownloadEvent::EngineFailed).await?;
            return Ok(Some(statistics));
        }
        if stats.complete && row.state == DownloadState::Downloading {
            self.finish_usenet(id, handle).await?;
        }
        Ok(Some(statistics))
    }

    /// Finishes one Usenet job: the post is whole, so it is imported and the engine forgets it.
    async fn finish_usenet(&self, id: i64, handle: UsenetHandle) -> Result<(), AcquisitionError> {
        self.usenet_handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id);
        self.written_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id);
        let result = self.complete_usenet(id).await;
        if let Some(engine) = self.usenet.as_ref()
            && let Err(error) = engine.remove(handle).await
        {
            warn!(%error, job = id, "a completed Usenet job could not be stopped");
        }
        result
    }

    /// The half of completion that does not need a live engine handle.
    ///
    /// A restart reconciles a row that already reached 100 %, and a job whose engine handle died
    /// with the previous process must still run the post-process and import. The caller owns handle
    /// removal, because the restart path has no handle to remove.
    async fn complete_usenet(&self, id: i64) -> Result<(), AcquisitionError> {
        let mut files = self.database.download_files(unrestricted(), id).await?;
        let mut has_media = files.iter().any(|file| file.selected && file.is_media);
        if !has_media && let Some(tools) = self.post.as_ref() {
            let row = self.reload(id).await?;
            let _ = self
                .database
                .set_download_stage(id, peregrine_domain::DownloadStage::Extracting)
                .await;
            let max_bytes = self
                .volume
                .available_bytes(std::path::Path::new(row.save_path.as_str()))
                .unwrap_or(0)
                .saturating_sub(self.free_space_floor);
            match crate::usenet_post::process(
                tools,
                &self.database,
                &self.destination,
                &row,
                &files,
                max_bytes,
            )
            .await
            {
                Ok(_) => {
                    files = self.database.download_files(unrestricted(), id).await?;
                    has_media = files.iter().any(|file| file.selected && file.is_media);
                }
                Err(error) => {
                    warn!(%error, job = id, "a Usenet archive could not be post-processed");
                }
            }
            let _ = self
                .database
                .set_download_stage(id, peregrine_domain::DownloadStage::Transfer)
                .await;
        }
        if !has_media {
            // A silent `completed` with nothing in the library would be worse than an honest
            // failure.
            let _ = self.apply(id, DownloadEvent::EngineFailed).await;
            warn!(
                job = id,
                "a Usenet payload held no media entry after post-processing"
            );
            return Ok(());
        }
        self.apply(id, DownloadEvent::PayloadComplete).await?;
        let row = self.reload(id).await?;
        if let Err(error) = self.import_completed(&row).await {
            warn!(%error, job = id, "a completed Usenet download could not be added to the library");
        }
        if self
            .database
            .usenet_setting()
            .await?
            .delete_archives_after_import
        {
            let job_dir = std::path::Path::new(row.save_path.as_str());
            for file in files.iter().filter(|file| file.selected) {
                let archive = crate::usenet_post::is_archive(file.path.as_str());
                let par2 = crate::usenet_post::is_par2(file.path.as_str());
                if !archive && !par2 {
                    continue;
                }
                let candidate = job_dir.join(file.path.as_str());
                if let Ok(safe) = self.destination.resolve(&candidate)
                    && let Err(error) = self.trash.move_to_trash(safe.as_path())
                {
                    warn!(%error, job = id, "a Usenet archive could not be moved to the trash");
                }
            }
        }
        Ok(())
    }

    /// Finishes one job whose payload is whole: the two domain edges, the import, and the stop.
    ///
    /// This session handle is dropped first, because this tick is the last one whatever happens
    /// after it: a second sample would apply Progress to a terminal row and be refused, and the
    /// refusal would be logged every second for ever.
    ///
    /// The import and the engine removal are best-effort and logged rather than propagated. Both
    /// are recoverable -- a later library scan adopts the file, and the torrent stops with the
    /// process -- and neither can be retried here because the handle is already gone. The state
    /// machine's two transitions are not best-effort: they are propagated.
    async fn finish_download(
        &self,
        id: i64,
        handle: TorrentHandle,
    ) -> Result<(), AcquisitionError> {
        self.handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id);
        self.written_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id);

        self.apply(id, DownloadEvent::PayloadComplete).await?;
        self.apply(id, DownloadEvent::SeedingSatisfied).await?;
        let row = self.reload(id).await?;

        if let Err(error) = self.import_completed(&row).await {
            warn!(%error, job = id, "a completed download could not be added to the library");
        }
        // No seeding policy is configured yet (PG-E10-09), so a finished payload stops here: the
        // torrent leaves the session and its files stay where they were written.
        if let Err(error) = self.engine.remove(handle).await {
            warn!(%error, job = id, "a completed download could not be stopped");
        }
        Ok(())
    }

    /// Finishes the jobs a previous process left at a whole payload.
    ///
    /// A restart loses the engine handles this session never had, so the pump cannot see a
    /// transfer that completed while the app was closed (`PG-E10-08` owns reattaching it). What the
    /// row still records is the answer: a job in `downloading` at 100 % is a payload that is whole,
    /// and this applies the same two edges and import a live tick would. The composition root calls
    /// it once before the pump starts.
    pub(crate) async fn reconcile_finished(&self) -> Result<usize, AcquisitionError> {
        let jobs = self
            .database
            .list_download_jobs(
                unrestricted(),
                DownloadState::ALL,
                PageRequest::first(1_000),
            )
            .await?;
        let mut finished = 0;
        for job in jobs {
            if job.state != DownloadState::Downloading || job.progress_permille < 1_000 {
                continue;
            }
            if job.protocol == Protocol::Usenet {
                self.complete_usenet(job.id).await?;
                finished += 1;
                continue;
            }
            self.apply(job.id, DownloadEvent::PayloadComplete).await?;
            self.apply(job.id, DownloadEvent::SeedingSatisfied).await?;
            let row = self.reload(job.id).await?;
            if let Err(error) = self.import_completed(&row).await {
                warn!(%error, job = job.id, "a completed download could not be added to the library");
            }
            finished += 1;
        }
        Ok(finished)
    }

    /// Removes source NZBs whose job is gone, so a cancelled job does not leave a directory behind.
    ///
    /// Only a file named exactly `source.nzb` is considered, and only under the download root. A
    /// directory named by a live job is left alone; a symlink is not followed.
    ///
    /// # Errors
    ///
    /// Storage when the job list cannot be read.
    pub async fn sweep_orphan_nzbs(&self) -> Result<usize, AcquisitionError> {
        let jobs = self
            .database
            .list_download_jobs(
                unrestricted(),
                DownloadState::ALL,
                PageRequest::first(10_000),
            )
            .await?;
        let live: std::collections::HashSet<std::path::PathBuf> = jobs
            .iter()
            .map(|job| std::path::PathBuf::from(job.save_path.as_str()))
            .collect();
        let mut stack = vec![(self.destination.root().to_path_buf(), 0_u8)];
        let mut swept = 0usize;
        while let Some((directory, depth)) = stack.pop() {
            if depth > 4 {
                continue;
            }
            let Ok(mut entries) = tokio::fs::read_dir(&directory).await else {
                continue;
            };
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                let Ok(metadata) = entry.metadata().await else {
                    continue;
                };
                if metadata.file_type().is_symlink() {
                    continue;
                }
                if metadata.is_dir() {
                    stack.push((path, depth + 1));
                } else if entry.file_name() == "source.nzb"
                    && !live.contains(&directory)
                    && self.trash.move_to_trash(&path).is_ok()
                {
                    swept += 1;
                }
            }
        }
        Ok(swept)
    }

    /// The episode a payload file belongs to, read from its own name.
    ///
    /// One season and one episode, and nothing else: a multi-episode span, a season pack's folder,
    /// an extra, and a cartoon numbered absolutely all answer `None`, because a file placed on the
    /// wrong episode is worse than one the title keeps.
    #[must_use]
    fn episode_number_in(path: &str) -> Option<(u32, u32)> {
        let name = std::path::Path::new(path).file_name().map_or_else(
            || path.to_owned(),
            |file| file.to_string_lossy().into_owned(),
        );
        let parsed = ReleaseName::parse(&name);
        let season = parsed.season()?;
        let span = parsed.episodes()?;
        (!span.is_multi()).then(|| (season, span.first()))
    }

    /// Gives the library the media files a completed download wrote.
    ///
    /// The full import (PG-E14-19) owns probing, naming, and episode placement. This is the part a
    /// completed job can do without any of them: the title the job was started for is already
    /// known, so the item is that title, and each selected media entry that is on disk becomes a
    /// `media_file` row. A file the engine has not flushed is skipped rather than failing the
    /// import, and a path that is already recorded is left alone (`media_file.path` is UNIQUE, I3).
    ///
    /// **An episode with no still gets one from the file just imported.** A provider may publish an
    /// episode's still after the payload exists, and a season synced early keeps a NULL column with
    /// nothing that re-syncs it; the bytes are here, so a frame is. It is attempted once per episode
    /// even when a pack carries several files for one, and a capture that fails is a logged warning
    /// rather than a failed import (`crate::episode_stills` owns the rule).
    async fn import_completed(&self, row: &DownloadJobRow) -> Result<(), AcquisitionError> {
        let files = self.database.download_files(unrestricted(), row.id).await?;
        let at = self.clock.now();
        let folder = std::path::Path::new(row.save_path.as_str());
        let mut captured: HashSet<i64> = HashSet::new();

        for file in files.iter().filter(|file| file.selected && file.is_media) {
            let candidate = folder.join(file.path.as_str());
            // A metadata path the guard refuses now is not a file to import; the inspection gate
            // already refused traversal, so this is belt and braces rather than a live path.
            let Ok(safe) = self.destination.resolve(&candidate) else {
                continue;
            };
            let Ok(metadata) = tokio::fs::metadata(safe.as_path()).await else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            // **A pack's files belong to their own episodes.** A job that named one episode owns
            // every file it wrote; one that named none — a season pack — carries the episode in
            // each file's own name, and a file that names no single episode stays on the title
            // rather than being guessed onto one.
            let episode_id = match row.episode_id {
                Some(id) => Some(id),
                None => match Self::episode_number_in(file.path.as_str()) {
                    Some((season, episode)) => {
                        self.database
                            .episode_id_for(row.title_id, season, episode)
                            .await?
                    }
                    None => None,
                },
            };
            let id = match self.database.item_id_for(row.title_id, episode_id).await? {
                Some(id) => id,
                None => {
                    self.database
                        .insert_media_item(MediaItem {
                            id: 0,
                            title_id: row.title_id,
                            episode_id,
                            state: MediaItemState::Available,
                            origin: match row.protocol {
                                Protocol::Torrent => MediaOrigin::Torrent,
                                Protocol::Usenet => MediaOrigin::Usenet,
                            },
                            preferred_file_id: None,
                            quality_label: None,
                            created_at: at,
                            updated_at: at,
                        })
                        .await?
                }
            };
            let Ok(path) = NonEmptyText::new(safe.as_path().to_string_lossy().as_ref()) else {
                continue;
            };
            // A recorded path is not inserted twice, but it is still a file this episode holds: the
            // capture below is what a re-import of an older job backfills, so the skip cannot be a
            // `continue`.
            if self
                .database
                .media_file_by_path(unrestricted(), path.as_str())
                .await?
                .is_none()
            {
                self.database
                    .insert_media_file(MediaFile {
                        id: 0,
                        media_item_id: id,
                        path,
                        // No root owns a download: it lives under the app own download directory,
                        // which is not a configured library root. A scan that later adopts the file
                        // refreshes the root id.
                        root_id: None,
                        size_bytes: metadata.len(),
                        mtime_ms: Timestamp::from_millisecond(modified_millis(&metadata))
                            .unwrap_or(at),
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
            }
            if let Some(episode_id) = episode_id {
                self.capture_missing_still(episode_id, safe.as_path(), &mut captured)
                    .await;
            }
        }
        Ok(())
    }

    /// Fills one downloaded episode's missing still, at most once per episode in one import.
    ///
    /// A pack can carry several files for one episode, and a capture that failed must not be retried
    /// per file; the set is what makes "logs once" true. The failure is logged and swallowed because
    /// the import it belongs to is still a successful import — the episode simply keeps its NULL
    /// still until a later one can capture.
    async fn capture_missing_still(
        &self,
        episode_id: i64,
        source: &std::path::Path,
        attempted: &mut HashSet<i64>,
    ) {
        let Some(stills) = self.stills.as_ref() else {
            return;
        };
        if !attempted.insert(episode_id) {
            return;
        }
        if let Err(error) = stills.capture_if_missing(episode_id, source).await {
            warn!(
                %error,
                episode = episode_id,
                "an episode still could not be captured from its downloaded file"
            );
        }
    }

    /// Enumerates exactly what deleting one download would remove, without removing anything.
    ///
    /// AGENTS R22: the plan is taken first and the user confirms the list; `delete_files` resolves
    /// every path again. A file the guard refuses, or that is no longer on disk, is not in the plan.
    pub async fn deletion_plan(&self, id: i64) -> Result<DeletionPlan, AcquisitionError> {
        let row = self.reload(id).await?;
        let files = self.database.download_files(unrestricted(), id).await?;
        Ok(DeletionPlan::new(id, self.enumerate(&row, &files).await))
    }

    /// Moves one download files to the operating system trash and clears its rows.
    ///
    /// The transfer is stopped before anything is moved, so the engine is never writing to a file
    /// that is being taken away. Every path is resolved through the guard again between the plan
    /// and the move (AGENTS R22); a path that no longer resolves fails the whole act rather than
    /// skipping one file, because a partial deletion the user did not ask for is worse than a
    /// refusal they can retry.
    ///
    /// The library rows the import wrote go with the bytes, so the title reports the payload as
    /// deleted rather than missing. The item row is kept: it is the history, and the watch
    /// progress hangs off it.
    pub async fn delete_files(&self, id: i64) -> Result<DeletionPlan, AcquisitionError> {
        let row = self.reload(id).await?;
        // The guard is dropped before the await: a mutex guard held across one makes the whole
        // future non-Send, and this service is driven from Tauri's async command surface.
        let handle = self
            .handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id);
        if let Some(handle) = handle {
            self.engine.remove(handle).await?;
        }
        if !row.state.is_terminal() {
            self.apply(id, DownloadEvent::Removed).await?;
        }
        self.written_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id);

        let files = self.database.download_files(unrestricted(), id).await?;
        let plan = DeletionPlan::new(id, self.enumerate(&row, &files).await);

        for entry in &plan.entries {
            let candidate = std::path::PathBuf::from(entry.path.as_str());
            let safe = self
                .destination
                .resolve(&candidate)
                .map_err(|source| AcquisitionError::SavePath { source })?;
            let trash = Arc::clone(&self.trash);
            let path = safe.as_path().to_path_buf();
            tokio::task::spawn_blocking(move || trash.move_to_trash(&path))
                .await
                .map_err(|_| TrashError::Refused)??;
            self.database
                .delete_media_file_by_path(safe.as_path().to_string_lossy().as_ref())
                .await?;
        }

        // The row is completed or removed now, which is exactly what the repository own guard
        // admits to deletion.
        self.database.delete_download_job(id).await?;
        Ok(plan)
    }

    /// The on-disk files a job selected entries name, with their sizes.
    ///
    /// Shared by the plan and the act, so the two cannot enumerate different sets. The path is
    /// passed through the guard, and a path with no file behind it is left out rather than planned.
    async fn enumerate(
        &self,
        row: &DownloadJobRow,
        files: &[DownloadFileRow],
    ) -> Vec<DeletionEntry> {
        let folder = std::path::Path::new(row.save_path.as_str());
        let mut entries = Vec::new();
        for file in files.iter().filter(|file| file.selected) {
            let candidate = folder.join(file.path.as_str());
            let Ok(safe) = self.destination.resolve(&candidate) else {
                continue;
            };
            let Ok(metadata) = tokio::fs::metadata(safe.as_path()).await else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let Ok(path) = NonEmptyText::new(safe.as_path().to_string_lossy().as_ref()) else {
                continue;
            };
            entries.push(DeletionEntry::new(path, metadata.len()));
        }
        entries
    }

    async fn enumerate_media(
        &self,
        evidence: &peregrine_storage::TitleEvidence,
        episode_id: Option<i64>,
    ) -> Vec<DeletionEntry> {
        let mut entries = Vec::new();
        let mut paths = HashSet::new();
        for item in evidence
            .items
            .iter()
            .filter(|item| episode_id.is_none() || item.item.episode_id == episode_id)
        {
            for file in &item.files {
                let candidate = std::path::Path::new(file.path.as_str());
                let Ok(safe) = self.destination.resolve(candidate) else {
                    continue;
                };
                let Ok(metadata) = tokio::fs::metadata(safe.as_path()).await else {
                    continue;
                };
                if !metadata.is_file() || !paths.insert(safe.as_path().to_path_buf()) {
                    continue;
                }
                let Ok(path) = NonEmptyText::new(safe.as_path().to_string_lossy().as_ref()) else {
                    continue;
                };
                entries.push(DeletionEntry::new(path, metadata.len()));
            }
        }
        entries
    }

    /// The handle this process holds for a job.
    fn handle(&self, id: i64) -> Result<TorrentHandle, AcquisitionError> {
        self.handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&id)
            .copied()
            .ok_or(AcquisitionError::NotFound)
    }

    /// Re-reads one job after a write, so a caller is answered with what the database holds.
    ///
    /// Unrestricted rather than scoped: the caller has just been through [`AcquisitionService::start`]'s
    /// scope gate or is the queue acting on a row it listed, and `start` re-reads its own
    /// just-written row before any account question could have a second answer.
    async fn reload(&self, id: i64) -> Result<DownloadJobRow, AcquisitionError> {
        self.database
            .download_job(unrestricted(), id)
            .await?
            .ok_or(AcquisitionError::NotFound)
    }
}

/// One tick's payload, shaped for the queue screen that consumes it (`PG-E13-01`).
fn payload(id: i64, statistics: &DownloadStatistics) -> serde_json::Value {
    serde_json::json!({
        "job_id": id,
        "downloaded_bytes": statistics.downloaded_bytes,
        "uploaded_bytes": statistics.uploaded_bytes,
        "progress_permille": statistics.progress_permille,
        "download_rate_bps": statistics.download_rate_bps,
        "upload_rate_bps": statistics.upload_rate_bps,
        "peers_connected": statistics.peers_connected,
        "peers_swarm": statistics.peers_swarm,
        "eta_seconds": statistics.eta_seconds,
        "ratio": statistics.ratio,
    })
}

/// One engine reading, in the repository's vocabulary.
fn statistics_of(stats: &peregrine_torrent::TorrentStats) -> DownloadStatistics {
    let transfer = stats.transfer();
    let downloaded = transfer.downloaded().get();
    let total = stats.total().get();
    // Completion in permille, and a torrent whose metadata reports no size is at zero rather than
    // at a fraction of nothing.
    let permille = downloaded
        .saturating_mul(1_000)
        .checked_div(total)
        .and_then(|permille| i64::try_from(permille).ok())
        .unwrap_or(0);
    DownloadStatistics {
        downloaded_bytes: downloaded,
        uploaded_bytes: transfer.uploaded().get(),
        progress_permille: permille.min(1_000),
        download_rate_bps: transfer.download_rate().get(),
        upload_rate_bps: transfer.upload_rate().get(),
        peers_connected: stats.peers().connected(),
        peers_swarm: stats.peers().seeds(),
        eta_seconds: stats.eta().map(|eta| eta.as_secs()),
        ratio: transfer
            .ratio()
            .hundredths()
            .map(|hundredths| f64::from(hundredths) / 100.0),
    }
}

/// A resolved path as the text the schema stores.
fn text_of(path: &SafePath) -> Result<NonEmptyText, AcquisitionError> {
    NonEmptyText::new(&path.as_path().to_string_lossy())
        .map_err(|_| AcquisitionError::UnsupportedSource)
}

/// The scope a read of a row this process just wrote runs under.
///
/// The account gate is [`AcquisitionService::start`]'s scoped title read and `PG-E35-01`'s
/// capability model; this is a read-back of a row this call wrote, not a user-facing list, and
/// `Runtime::installation_scope` is the same unrestricted scope the rest of this build reads with.
fn unrestricted() -> AccountScope {
    AccountScope::unrestricted(
        peregrine_domain::AccountId::new(1).expect("account one is a positive id"),
    )
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use peregrine_domain::{AccountId, EntryKind, InfoHash, PayloadEntry};
    use peregrine_storage::{AddedBy, MagnetUri, PageRequest};
    use peregrine_testkit::{
        DEFAULT_EPOCH_MILLISECONDS, EngineCall, EngineOperation, FakeClock, FakeTorrentEngine,
        FakeUsenetEngine,
    };
    use peregrine_torrent::{
        Bytes, EngineError, EngineFuture, FetchedMetadata, TorrentMetadataFetch, TorrentSource,
    };
    use tempfile::TempDir;

    use std::future::Future;
    use std::pin::Pin;

    use peregrine_domain::catalogue::CAPTURED_STILL_PROVIDER;
    use peregrine_sync::{ImageCache, ImageKey};

    use super::*;
    use crate::episode_stills::{CaptureError, EpisodeStills, FrameCapture};
    use crate::events::EventSink;
    use crate::testing::RecordingSink;

    /// A volume whose free space the test decides, including between calls.
    #[derive(Debug)]
    struct Volume(AtomicU64);

    impl Volume {
        fn of(bytes: u64) -> Arc<Self> {
            Arc::new(Self(AtomicU64::new(bytes)))
        }

        fn set(&self, bytes: u64) {
            self.0.store(bytes, Ordering::Relaxed);
        }
    }

    impl VolumeSpace for Volume {
        fn available_bytes(&self, _path: &Path) -> Result<u64, SpaceError> {
            Ok(self.0.load(Ordering::Relaxed))
        }
    }

    /// A trash that records what it took and really removes the file, so a deletion test sees the
    /// same postcondition a user would: the bytes are gone and the call is remembered.
    #[derive(Debug, Default)]
    struct TrashLog(Mutex<Vec<PathBuf>>);

    impl TrashLog {
        fn new() -> Arc<Self> {
            Arc::new(Self(Mutex::new(Vec::new())))
        }

        fn taken(&self) -> Vec<PathBuf> {
            self.0
                .lock()
                .expect("the trash log is not poisoned")
                .clone()
        }
    }

    impl Trash for TrashLog {
        fn move_to_trash(&self, path: &Path) -> Result<(), TrashError> {
            std::fs::remove_file(path).map_err(|_| TrashError::Refused)?;
            self.0
                .lock()
                .expect("the trash log is not poisoned")
                .push(path.to_path_buf());
            Ok(())
        }
    }

    /// A trash the service can hold, for a test that does not inspect it.
    fn trash() -> Arc<dyn Trash> {
        TrashLog::new()
    }

    /// Records what a finished transfer would have announced.
    struct NoticeLog(Mutex<Vec<(i64, bool)>>);

    impl NoticeLog {
        fn new() -> Arc<Self> {
            Arc::new(Self(Mutex::new(Vec::new())))
        }

        fn seen(&self) -> Vec<(i64, bool)> {
            self.0
                .lock()
                .expect("the notice log is not poisoned")
                .clone()
        }
    }

    impl DownloadNotices for NoticeLog {
        fn finished<'a>(
            &'a self,
            download_id: i64,
            failed: bool,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
            Box::pin(async move {
                self.0
                    .lock()
                    .expect("the notice log is not poisoned")
                    .push((download_id, failed));
            })
        }
    }

    /// The metadata fetch a recovery test needs: the fixture's own torrent, whatever the magnet.
    struct FixedMetadata;

    impl TorrentMetadataFetch for FixedMetadata {
        fn fetch_metadata(&self, _source: &TorrentSource) -> EngineFuture<'_, FetchedMetadata> {
            Box::pin(async { Ok(FetchedMetadata::new(hash(42), description())) })
        }
    }

    /// The floor every fixture reserves.
    const FLOOR: u64 = 1_000;

    /// One torrent of two entries, which is what the fake engine is told it holds.
    fn description() -> PayloadDescription {
        PayloadDescription::new(vec![
            PayloadEntry::new("Movie.2024.1080p.mkv", 1_000, EntryKind::File),
            PayloadEntry::new("Movie.2024.1080p.srt", 1_000, EntryKind::File),
        ])
    }

    /// A hash no other fixture in one test holds, spelled from one byte.
    fn hash(byte: u8) -> InfoHash {
        InfoHash::from_bytes([byte; 20])
    }

    fn text(value: &str) -> NonEmptyText {
        NonEmptyText::new(value).expect("fixture text is not empty")
    }

    fn account() -> AccountScope {
        AccountScope::unrestricted(AccountId::new(1).expect("account one is a positive id"))
    }

    /// The job columns one start is given, with the two the service overwrites filled in anyway.
    fn new_job(title_id: i64) -> NewDownloadJob {
        NewDownloadJob {
            title_id,
            episode_id: None,
            season_number: None,
            release_title: "Movie.2024.1080p.BluRay.x264-GROUP".to_owned(),
            release_source: Some("BluRay".to_owned()),
            release_group: Some("GROUP".to_owned()),
            resolution: Some("1080p".to_owned()),
            video_codec: Some("h264".to_owned()),
            hdr_format: None,
            audio_summary: Some("DTS 5.1".to_owned()),
            reported_size_bytes: Some(2_000),
            info_hash: None,
            protocol: peregrine_domain::Protocol::Torrent,
            nzb_sha256: None,
            source: DownloadSource::Magnet(MagnetUri::new(text(
                "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567",
            ))),
            save_path: text("Movies"),
            priority: 100,
            added_by: AddedBy::User,
            added_at: Timestamp::from_millisecond(DEFAULT_EPOCH_MILLISECONDS)
                .expect("the fixture instant is in range"),
        }
    }

    fn usenet_job(title_id: i64) -> NewDownloadJob {
        let mut job = new_job(title_id);
        job.release_title = "Movie.2024.1080p.WEB-DL-GROUP".to_owned();
        job.info_hash = None;
        job.protocol = peregrine_domain::Protocol::Usenet;
        job.nzb_sha256 = peregrine_domain::NzbHash::parse(&"a".repeat(64));
        job.source = DownloadSource::NzbFile(text("/tmp/source.nzb"));
        job.reported_size_bytes = Some(5);
        job
    }

    fn request(title_id: i64, byte: u8, description: PayloadDescription) -> StartRequest {
        StartRequest {
            account: account(),
            job: new_job(title_id),
            description,
            selection: None,
            info_hash: hash(byte),
        }
    }

    /// A file's own name is what places a pack's episode, and only an unambiguous name does.
    ///
    /// A pack's folder or an extra names no episode; a span names several; absolute numbering says
    /// nothing about the season. Each has to answer nothing rather than a guess.
    #[test]
    fn only_a_file_that_names_one_episode_is_placed_on_it() {
        for (path, expected) in [
            ("Peppa.Pig.S01E07.720p.WEB-DL.mkv", Some((1, 7))),
            ("Peppa Pig S02E52 HDTV x264.mkv", Some((2, 52))),
            ("Show/Season 3/Show.S03E04.1080p.mkv", Some((3, 4))),
            ("Peppa Pig S01E01-E02.mkv", None),
            ("Peppa Pig S01E01-52 Complete.mkv", None),
            ("Peppa Pig - 07 - Muddy Puddles.mkv", None),
            ("extras/Behind the Scenes.mkv", None),
            ("Show.S01.Complete.mkv", None),
        ] {
            assert_eq!(
                AcquisitionService::episode_number_in(path),
                expected,
                "{path} must place exactly {expected:?}"
            );
        }
    }

    /// The service under test, and the things a test inspects it through.
    struct Harness {
        service: Arc<AcquisitionService>,
        engine: Arc<FakeTorrentEngine>,
        clock: Arc<FakeClock>,
        volume: Arc<Volume>,
        bus: Arc<EventBus>,
        sink: Arc<RecordingSink>,
        database: Arc<Database>,
        directory: TempDir,
        title_id: i64,
    }

    /// The bus the harness publishes through, built in one place so the extra services a test
    /// constructs subscribe to the same sink as the harness's own.
    fn bus(sink: &Arc<RecordingSink>) -> Arc<EventBus> {
        Arc::new(EventBus::new(Arc::clone(sink) as Arc<dyn EventSink>))
    }

    async fn harness() -> Harness {
        harness_with(false).await
    }

    async fn usenet_harness() -> Harness {
        harness_with(true).await
    }

    async fn harness_with(usenet: bool) -> Harness {
        let directory = TempDir::new().expect("a temporary directory");
        let database = Arc::new(
            Database::open(directory.path().join("peregrine.sqlite"))
                .await
                .expect("the database opens"),
        );
        let title_id = database
            .insert_stub_title(peregrine_storage::StubTitle {
                kind: peregrine_domain::TitleKind::Movie,
                subtype: peregrine_domain::TitleSubtype::Standard,
                name: "Movie".to_owned(),
                original_name: None,
                sort_name: "Movie".to_owned(),
                at: DEFAULT_EPOCH_MILLISECONDS,
            })
            .await
            .expect("the title is inserted");
        let engine = Arc::new(FakeTorrentEngine::new().with_files(2, Bytes::new(1_000)));
        let clock = Arc::new(FakeClock::default());
        let volume = Volume::of(1_000_000);
        let sink = Arc::new(RecordingSink::default());
        let bus = bus(&sink);
        let guard = PathGuard::new(directory.path().join("downloads"))
            .expect("the fixture root is absolute");
        let trash = TrashLog::new();
        let service = AcquisitionService::new(
            Arc::clone(&database),
            Arc::clone(&engine) as Arc<dyn TorrentEngine>,
            Arc::clone(&clock) as Arc<dyn Clock + Send + Sync>,
            guard,
            Arc::clone(&volume) as Arc<dyn VolumeSpace>,
            FLOOR,
            Arc::clone(&bus),
            Arc::clone(&trash) as Arc<dyn Trash>,
        );
        let service = service.with_metadata_fetch(Arc::new(FixedMetadata));
        let service = if usenet {
            service.with_usenet(Arc::new(FakeUsenetEngine::new()))
        } else {
            service
        };
        Harness {
            service: Arc::new(service),
            engine,
            clock,
            volume,
            bus,
            sink,
            database,
            directory,
            title_id,
        }
    }

    impl Harness {
        async fn start(
            &self,
            byte: u8,
            description: PayloadDescription,
        ) -> Result<Started, AcquisitionError> {
            self.service
                .start(request(self.title_id, byte, description))
                .await
        }

        /// Moves the clock past the statistics budget, so the next tick is not coalesced.
        fn advance(&self) {
            self.clock.advance(TICK_INTERVAL_MILLIS);
        }

        /// Every job row this test has written, which is how "no row was inserted" is asserted.
        async fn rows(&self) -> Vec<DownloadJobRow> {
            self.database
                .list_download_jobs(account(), DownloadState::ALL, PageRequest::first(50))
                .await
                .expect("the list read answers")
        }

        fn add_calls(&self) -> Vec<EngineCall> {
            self.engine
                .calls()
                .into_iter()
                .filter(|call| matches!(call, EngineCall::Add { .. }))
                .collect()
        }
    }

    /// A capture port that writes a fixture instead of running ffmpeg, and counts what it was asked.
    ///
    /// The real adapter is a child process, which a unit test must not need (AGENTS R14); this one
    /// answers the same contract with a file, so the import path can be asserted end to end.
    #[derive(Default)]
    struct FakeCapture {
        calls: AtomicU64,
        fail: bool,
    }

    impl FakeCapture {
        fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        fn failing() -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicU64::new(0),
                fail: true,
            })
        }

        fn calls(&self) -> u64 {
            self.calls.load(Ordering::Relaxed)
        }
    }

    impl FrameCapture for FakeCapture {
        fn capture<'a>(
            &'a self,
            _source: &'a Path,
            destination: &'a Path,
        ) -> Pin<Box<dyn Future<Output = Result<(), CaptureError>> + Send + 'a>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::Relaxed);
                if self.fail {
                    return Err(CaptureError::Failed);
                }
                tokio::fs::write(destination, b"fixture frame")
                    .await
                    .map_err(|_| CaptureError::Failed)
            })
        }
    }

    /// A show with two episodes, a service that captures stills, and the fake behind it.
    struct StillHarness {
        service: Arc<AcquisitionService>,
        database: Arc<Database>,
        cache: Arc<ImageCache>,
        capture: Arc<FakeCapture>,
        directory: TempDir,
        title_id: i64,
        episode_three: i64,
        episode_four: i64,
    }

    /// The payload entries a job of these names writes.
    fn episode_files(names: &[&str]) -> PayloadDescription {
        PayloadDescription::new(
            names
                .iter()
                .map(|name| PayloadEntry::new(*name, 1_000, EntryKind::File))
                .collect(),
        )
    }

    async fn still_harness(capture: Arc<FakeCapture>) -> StillHarness {
        let directory = TempDir::new().expect("a temporary directory");
        let database = Arc::new(
            Database::open(directory.path().join("peregrine.sqlite"))
                .await
                .expect("the database opens"),
        );
        let at = DEFAULT_EPOCH_MILLISECONDS;
        let title_id = database
            .insert_stub_title(peregrine_storage::StubTitle {
                kind: peregrine_domain::TitleKind::Show,
                subtype: peregrine_domain::TitleSubtype::Standard,
                name: "VisionQuest".to_owned(),
                original_name: None,
                sort_name: "VisionQuest".to_owned(),
                at,
            })
            .await
            .expect("the show is inserted");
        let season_id = database
            .insert_stub_season(peregrine_storage::StubSeason {
                title_id,
                season_number: 1,
                at,
            })
            .await
            .expect("the season is inserted");
        for episode_number in [3, 4] {
            database
                .insert_stub_episode(peregrine_storage::StubEpisode {
                    title_id,
                    season_id,
                    season_number: 1,
                    episode_number,
                    at,
                })
                .await
                .expect("the episode is inserted");
        }
        let episode_three = database
            .episode_id_for(title_id, 1, 3)
            .await
            .expect("the read answers")
            .expect("episode three is there");
        let episode_four = database
            .episode_id_for(title_id, 1, 4)
            .await
            .expect("the read answers")
            .expect("episode four is there");

        let cache = Arc::new(ImageCache::new(directory.path().join("images")));
        let clock = Arc::new(FakeClock::default());
        let engine = Arc::new(
            FakeTorrentEngine::new()
                .with_files(2, Bytes::new(1_000))
                .with_downloaded(Bytes::new(2_000)),
        );
        let sink = Arc::new(RecordingSink::default());
        let guard = PathGuard::new(directory.path().join("downloads"))
            .expect("the fixture root is absolute");
        let service = AcquisitionService::new(
            Arc::clone(&database),
            Arc::clone(&engine) as Arc<dyn TorrentEngine>,
            Arc::clone(&clock) as Arc<dyn Clock + Send + Sync>,
            guard,
            Volume::of(1_000_000) as Arc<dyn VolumeSpace>,
            FLOOR,
            bus(&sink),
            trash(),
        )
        .with_metadata_fetch(Arc::new(FixedMetadata))
        .with_stills(Arc::new(EpisodeStills::new(
            Arc::clone(&cache),
            Arc::clone(&database),
            Arc::clone(&clock) as Arc<dyn Clock + Send + Sync>,
            Arc::clone(&capture) as Arc<dyn FrameCapture>,
        )));
        StillHarness {
            service: Arc::new(service),
            database,
            cache,
            capture,
            directory,
            title_id,
            episode_three,
            episode_four,
        }
    }

    impl StillHarness {
        /// Starts a job, writes the payload's files, and drives the tick that imports them.
        async fn download(&self, request: StartRequest, names: &[&str]) -> Started {
            let started = self
                .service
                .start(request)
                .await
                .expect("the download starts");
            let folder = self.directory.path().join("downloads").join("Movies");
            std::fs::create_dir_all(&folder).expect("the download folder");
            for name in names {
                std::fs::write(folder.join(name), vec![0_u8; 1_000]).expect("the media file");
            }
            let statistics = self
                .service
                .statistics(started.job.id)
                .await
                .expect("the tick answers")
                .expect("the tick is not coalesced");
            assert_eq!(statistics.progress_permille, 1_000);
            started
        }

        /// A one-episode job naming episode three.
        fn episode_request(&self, byte: u8) -> StartRequest {
            let mut request = request(
                self.title_id,
                byte,
                episode_files(&["VisionQuest.S01E03.1080p.mkv"]),
            );
            request.job.episode_id = Some(self.episode_three);
            request
        }

        /// The still the episode records, which must be there for a capture to have landed.
        async fn recorded(&self, episode_id: i64) -> peregrine_storage::StoredStill {
            self.database
                .episode_still(episode_id)
                .await
                .expect("the read answers")
                .expect("the episode records a still")
        }
    }

    /// The happy path: `queued` → `inspecting` → `metadata` → `downloading`, with the report
    /// persisted before the add and the engine's own state persisted after it.
    #[tokio::test]
    async fn an_allowed_release_becomes_a_downloading_job_with_its_report_and_engine_state() {
        let harness = harness().await;
        let started = harness
            .start(1, description())
            .await
            .expect("the gate allows a media file");

        assert!(started.added);
        assert_eq!(started.verdict, Verdict::Allowed);
        assert_eq!(started.state, DownloadState::Downloading);
        assert_eq!(started.job.state, DownloadState::Downloading);
        assert_eq!(started.selection, vec![0, 1]);

        // I4 on the write path: the report the add was licensed by is the row's, and the row is in
        // a state only a report-carrying event could have reached.
        let report = started
            .job
            .safety_report
            .as_ref()
            .expect("the report is persisted with the state it licenses");
        assert_eq!(report.verdict(), Verdict::Allowed);
        assert_eq!(report.file_count(), 2);

        // The resolved destination is what the row holds, not the relative text the caller sent.
        assert!(
            std::path::Path::new(started.job.save_path.as_str())
                .ends_with(std::path::Path::new("downloads").join("Movies")),
            "the row holds the guarded absolute path, found {}",
            started.job.save_path.as_str()
        );

        // The engine was asked once, for every entry, in file order.
        let calls = harness.add_calls();
        assert_eq!(calls.len(), 1, "one add for one start");
        match &calls[0] {
            EngineCall::Add {
                info_hash,
                selection,
                sequential,
                intent,
            } => {
                assert_eq!(*info_hash, hash(1));
                assert_eq!(selection.indices(), &[0, 1]);
                assert!(*sequential);
                assert_eq!(*intent, AddIntent::Download);
            }
            other => panic!("expected an add, found {other:?}"),
        }

        // The engine's state is persisted, and the file rows are written from the description.
        let files = harness
            .database
            .download_files(account(), started.job.id)
            .await
            .expect("the file read answers");
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|file| file.selected));
        assert!(files[0].is_media, "the .mkv is a media file");
        assert!(!files[1].is_media, "the .srt is not");
        assert!(
            harness
                .database
                .download_engine_state(account(), started.job.id)
                .await
                .expect("the state read answers")
                .is_some(),
            "SM1: a downloading job holds a persisted engine state"
        );
    }

    /// A narrowed selection is the season pack's whole write: the rows for entries outside it stay
    /// unselected and the engine is told to write only the named ones.
    #[tokio::test]
    async fn a_narrowed_selection_keeps_the_entries_it_names_and_no_others() {
        let harness = harness().await;
        let mut narrowed = request(harness.title_id, 5, description());
        narrowed.selection = Some(vec![1]);
        let started = harness
            .service
            .start(narrowed)
            .await
            .expect("the gate allows a subset of a media torrent");
        assert_eq!(started.selection, vec![1]);

        let files = harness
            .database
            .download_files(account(), started.job.id)
            .await
            .expect("the file read answers");
        assert_eq!(files.len(), 2, "every entry still has its row");
        assert!(
            !files[0].selected,
            "the entry outside the selection is not written"
        );
        assert!(files[1].selected);

        match &harness.add_calls()[0] {
            EngineCall::Add { selection, .. } => assert_eq!(selection.indices(), &[1]),
            other => panic!("expected an add, found {other:?}"),
        }
    }

    /// F6's pre-write guarantee, read off the engine's call log rather than off the code: neither a
    /// hard block nor an opt-in refusal reaches `add`, and the report explains both after the fact.
    #[tokio::test]
    async fn a_blocked_or_opt_in_torrent_never_reaches_the_engine() {
        let harness = harness().await;

        let blocked = harness
            .start(
                2,
                PayloadDescription::new(vec![PayloadEntry::new(
                    "../authorized_keys",
                    1,
                    EntryKind::File,
                )]),
            )
            .await
            .expect("a block is a decision, not a failure");
        assert!(!blocked.added);
        assert_eq!(blocked.verdict, Verdict::Blocked);
        assert_eq!(blocked.job.state, DownloadState::Blocked);
        assert!(blocked.job.safety_report.is_some());

        let opt_in = harness
            .start(
                3,
                PayloadDescription::new(vec![PayloadEntry::new("extras.zip", 1, EntryKind::File)]),
            )
            .await
            .expect("an opt-in refusal is a decision too");
        assert!(!opt_in.added);
        assert_eq!(opt_in.verdict, Verdict::OptInRequired);
        assert_eq!(opt_in.job.state, DownloadState::Blocked);

        assert!(
            harness.add_calls().is_empty(),
            "a blocked torrent has no spelling that reaches the engine"
        );
    }

    /// The opt-in branch: the same description re-inspected with `OptIn::Granted` lifts the block,
    /// and it does so as an explicit second call — a grant covers one torrent and one prompt. F6
    /// merges this branch back into the add, so the confirmation is what starts the transfer.
    #[tokio::test]
    async fn consent_lifts_an_opt_in_block_and_completes_the_add() {
        let harness = harness().await;
        let archive =
            PayloadDescription::new(vec![PayloadEntry::new("extras.zip", 1, EntryKind::File)]);
        let blocked = harness
            .start(4, archive.clone())
            .await
            .expect("the gate parks the job");
        assert_eq!(blocked.job.state, DownloadState::Blocked);
        assert_eq!(harness.add_calls(), Vec::new());

        let state = harness
            .service
            .consent(blocked.job.id, &archive)
            .await
            .expect("the granted opt-in allows the archive");
        assert_eq!(state, DownloadState::Downloading);
        assert_eq!(
            harness.add_calls().len(),
            1,
            "the confirmation is the last thing before the transfer starts"
        );
        let row = harness
            .database
            .download_job(account(), blocked.job.id)
            .await
            .expect("the read answers")
            .expect("the row exists");
        assert_eq!(row.state, DownloadState::Downloading);
    }

    #[tokio::test]
    async fn a_usenet_post_is_gated_stored_and_handed_to_the_engine() {
        let harness = usenet_harness().await;
        let nzb = "<nzb><file subject=\"movie.mkv\" date=\"0\" poster=\"p\">\
                   <segments><segment bytes=\"5\" number=\"1\">&lt;one@test&gt;</segment>\
                   </segments></file></nzb>";
        let document = NzbDocument::parse(nzb.as_bytes()).expect("the fixture parses");
        let servers = vec![ServerConfig {
            nntp: peregrine_usenet::NntpConfig {
                host: "news.example.test".to_owned(),
                port: 563,
                tls: true,
                username: "reader".to_owned(),
                password: None,
            },
            connections: 1,
            priority: 0,
        }];

        let started = harness
            .service
            .start_usenet(UsenetStartRequest {
                account: account(),
                job: usenet_job(harness.title_id),
                document,
                nzb_bytes: nzb.as_bytes().to_vec(),
                servers,
            })
            .await
            .expect("a clean post starts");

        assert_eq!(started.state, DownloadState::Downloading);
        assert!(started.added, "the post reached the engine");
        assert_eq!(started.job.protocol, peregrine_domain::Protocol::Usenet);
        let Some(DownloadSource::NzbFile(path)) = started.job.source.as_ref() else {
            panic!("the source is the stored NZB");
        };
        assert!(
            std::path::Path::new(path.as_str()).is_file(),
            "the NZB is stored"
        );
        let files = harness
            .database
            .download_files(unrestricted(), started.job.id)
            .await
            .expect("the file rows");
        assert_eq!(files.len(), 1);
        assert!(files[0].selected);
    }

    #[tokio::test]
    async fn a_usenet_archive_waits_for_consent_then_starts() {
        let harness = usenet_harness().await;
        let nzb = "<nzb><file subject=\"extras.zip\" date=\"0\" poster=\"p\">\
                   <segments><segment bytes=\"5\" number=\"1\">&lt;one@test&gt;</segment>\
                   </segments></file></nzb>";
        let document = NzbDocument::parse(nzb.as_bytes()).expect("the fixture parses");
        let servers = vec![ServerConfig {
            nntp: peregrine_usenet::NntpConfig {
                host: "news.example.test".to_owned(),
                port: 563,
                tls: true,
                username: "reader".to_owned(),
                password: None,
            },
            connections: 1,
            priority: 0,
        }];
        let blocked = harness
            .service
            .start_usenet(UsenetStartRequest {
                account: account(),
                job: usenet_job(harness.title_id),
                document,
                nzb_bytes: nzb.as_bytes().to_vec(),
                servers: servers.clone(),
            })
            .await
            .expect("the gate parks the archive");
        assert_eq!(blocked.state, DownloadState::Blocked);
        assert!(!blocked.added);

        let state = harness
            .service
            .consent_usenet(blocked.job.id, servers)
            .await
            .expect("consent lifts the block");
        assert_eq!(state, DownloadState::Downloading);
        assert!(
            harness
                .database
                .usenet_setting()
                .await
                .unwrap()
                .allow_archives
        );
    }

    /// A row interrupted before its file list was written is completed from the stored magnet: the
    /// fetch rebuilds the description, every entry is selected, the gate's own report is reused, and
    /// the row reaches `downloading` with its files recorded.
    #[tokio::test]
    async fn a_metadata_row_is_recovered_from_its_magnet() {
        let harness = harness().await;
        let id = interrupted_handoff(&harness).await;

        let before = harness
            .database
            .download_job(unrestricted(), id)
            .await
            .expect("the row reads")
            .expect("the row exists");
        assert_eq!(before.state, DownloadState::Metadata);
        assert!(
            harness
                .database
                .download_files(unrestricted(), id)
                .await
                .expect("the files read")
                .is_empty(),
            "the interrupted handoff wrote no file list"
        );

        let recovered = harness
            .service
            .recover_metadata_jobs()
            .await
            .expect("recovery runs");
        assert_eq!(recovered, 1);

        let after = harness
            .database
            .download_job(unrestricted(), id)
            .await
            .expect("the row reads")
            .expect("the row exists");
        assert_eq!(after.state, DownloadState::Downloading);
        let files = harness
            .database
            .download_files(unrestricted(), id)
            .await
            .expect("the files read");
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|file| file.selected));
    }

    /// The stall rule on its own: a moving transfer forgets everything, a stalled one waits out the
    /// grace, then asks again on the retry interval until its attempts are spent.
    ///
    /// Driven with explicit instants rather than a timer, which is the whole reason the rule takes
    /// `now` as an argument (AGENTS R14).
    #[test]
    fn a_stall_is_waited_out_asked_about_and_then_given_up() {
        let mut watch = StallWatch::default();
        assert_eq!(watch.observe(0, true), StallAction::Waiting);
        assert_eq!(
            watch.observe(STALL_GRACE_MILLIS - 1, true),
            StallAction::Waiting,
            "the grace period is not over yet"
        );
        assert_eq!(
            watch.observe(STALL_GRACE_MILLIS, true),
            StallAction::Reannounce
        );
        assert_eq!(
            watch.observe(STALL_GRACE_MILLIS + 1, true),
            StallAction::Waiting,
            "the retry interval has not passed"
        );
        let mut at = STALL_GRACE_MILLIS;
        for _ in 1..STALL_ATTEMPTS {
            at += STALL_RETRY_MILLIS;
            assert_eq!(watch.observe(at, true), StallAction::Reannounce);
        }
        assert_eq!(
            watch.observe(at + STALL_RETRY_MILLIS, true),
            StallAction::GiveUp,
            "the attempts are spent"
        );
        assert_eq!(
            watch.observe(at + 2 * STALL_RETRY_MILLIS, false),
            StallAction::Moving,
            "a transfer that moves again resets the watch"
        );
        assert_eq!(
            watch.observe(at + 3 * STALL_RETRY_MILLIS, true),
            StallAction::Waiting,
            "and the next stall starts its grace from the beginning"
        );
    }

    /// How many pause/resume pairs a slice of the engine's call log holds.
    fn reannouncements(calls: &[EngineCall]) -> usize {
        calls
            .iter()
            .filter(|call| matches!(call, EngineCall::Pause(_) | EngineCall::Resume(_)))
            .count()
    }

    /// A peerless transfer is not left alone: the pump asks its swarm again after the grace period,
    /// and a row that has spent every attempt is failed with the reason the queue can draw.
    #[tokio::test]
    async fn a_peerless_transfer_is_reannounced_and_then_failed_with_its_reason() {
        let harness = harness().await;
        let started = harness
            .start(1, description())
            .await
            .expect("the fixture starts a torrent");
        let id = started.job.id;

        // The fake reports an idle, peerless transfer, which is exactly what a dead swarm looks like.
        let before = harness.engine.calls().len();
        harness.service.sample().await;
        assert_eq!(
            reannouncements(&harness.engine.calls()[before..]),
            0,
            "the grace period is not over yet"
        );

        harness.clock.advance(STALL_GRACE_MILLIS);
        harness.service.sample().await;
        assert_eq!(
            reannouncements(&harness.engine.calls()[before..]),
            2,
            "a pause and a resume restart the announcer"
        );

        for _ in 0..STALL_ATTEMPTS {
            harness.clock.advance(STALL_RETRY_MILLIS);
            harness.service.sample().await;
        }

        let row = harness
            .database
            .download_job(account(), id)
            .await
            .expect("the row reads")
            .expect("the row exists");
        assert_eq!(row.state, DownloadState::Failed);
        assert_eq!(row.error_code.as_deref(), Some("NO_PEERS"));
        assert!(
            row.error_message
                .as_deref()
                .is_some_and(|message| message.contains("no peers")),
            "the row says why: {:?}",
            row.error_message
        );
    }

    /// The row a previous process left in `metadata` with no file list, and the report the handoff
    /// never carried into an add.
    async fn interrupted_handoff(harness: &Harness) -> i64 {
        let description = description();
        let mut job = new_job(harness.title_id);
        job.info_hash = Some(hash(42));
        let save_path = harness
            .directory
            .path()
            .join("downloads")
            .join("Movies")
            .to_string_lossy()
            .into_owned();
        job.save_path = text(&save_path);
        let row = harness
            .database
            .create_download_job(job)
            .await
            .expect("the row inserts");
        let report = PayloadInspector::new(InspectionLimits::new(1_000_000, u64::MAX)).inspect(
            &description,
            &PathGuard::new(harness.directory.path().join("downloads"))
                .expect("the fixture root is absolute"),
            OptIn::NotGranted,
        );
        harness
            .service
            .apply(row.id, DownloadEvent::Admitted)
            .await
            .expect("admitted");
        harness
            .service
            .apply(row.id, DownloadEvent::Inspected(report))
            .await
            .expect("inspected");
        row.id
    }

    /// A handoff that lost its process is walked out of `metadata` by the pump, which is the edge the
    /// domain always had and nothing ever took.
    #[tokio::test]
    async fn the_sweep_completes_a_handoff_a_previous_process_abandoned() {
        let harness = harness().await;
        let id = interrupted_handoff(&harness).await;

        harness.service.sample().await;
        assert_eq!(
            harness
                .database
                .download_job(account(), id)
                .await
                .unwrap()
                .unwrap()
                .state,
            DownloadState::Metadata,
            "the grace period has not passed"
        );

        harness.clock.advance(STALL_GRACE_MILLIS);
        harness.service.sample().await;

        let row = harness
            .database
            .download_job(account(), id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.state, DownloadState::Downloading);
        assert_eq!(
            harness
                .database
                .download_files(account(), id)
                .await
                .unwrap()
                .len(),
            2,
            "the file list the re-add produced is written"
        );
    }

    /// A handoff whose metadata never arrives ends in `failed` with its reason rather than parking:
    /// a service without the metadata fetch can never complete the add, which is what a swarm that
    /// keeps refusing the metadata looks like from here.
    #[tokio::test]
    async fn the_sweep_fails_a_handoff_whose_metadata_never_arrives() {
        let harness = harness().await;
        let id = interrupted_handoff(&harness).await;
        let service = AcquisitionService::new(
            Arc::clone(&harness.database),
            Arc::clone(&harness.engine) as Arc<dyn TorrentEngine>,
            Arc::clone(&harness.clock) as Arc<dyn Clock + Send + Sync>,
            PathGuard::new(harness.directory.path().join("downloads"))
                .expect("the fixture root is absolute"),
            Arc::clone(&harness.volume) as Arc<dyn VolumeSpace>,
            FLOOR,
            Arc::clone(&harness.bus),
            trash(),
        );
        let service = Arc::new(service);

        service.sample().await;
        harness.clock.advance(STALL_GRACE_MILLIS);
        service.sample().await;
        assert_eq!(
            harness
                .database
                .download_job(account(), id)
                .await
                .unwrap()
                .unwrap()
                .state,
            DownloadState::Metadata,
            "a retry that cannot produce metadata leaves the row where it is"
        );
        for _ in 0..STALL_ATTEMPTS {
            harness.clock.advance(STALL_RETRY_MILLIS);
            service.sample().await;
        }

        let row = harness
            .database
            .download_job(account(), id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.state, DownloadState::Failed);
        assert_eq!(row.error_code.as_deref(), Some("NO_PEERS"));
        assert!(
            row.error_message
                .as_deref()
                .is_some_and(|message| message.contains("metadata")),
            "the row says what did not arrive: {:?}",
            row.error_message
        );
    }

    /// A person asking for peers by hand restarts the announcer at once, without waiting out the
    /// guard's grace period, and spends the attempts again.
    #[tokio::test]
    async fn find_peers_reannounces_now_and_refuses_a_paused_transfer() {
        let harness = harness().await;
        let started = harness
            .start(1, description())
            .await
            .expect("the fixture starts a torrent");
        let id = started.job.id;

        let before = harness.engine.calls().len();
        harness
            .service
            .find_peers(id)
            .await
            .expect("a live transfer can be asked for peers");
        assert_eq!(
            reannouncements(&harness.engine.calls()[before..]),
            2,
            "the announcer is restarted without waiting for the grace period"
        );
        assert_eq!(
            harness
                .database
                .download_job(account(), id)
                .await
                .unwrap()
                .unwrap()
                .state,
            DownloadState::Downloading,
            "asking for peers does not change what the row is"
        );

        harness
            .service
            .pause(id)
            .await
            .expect("the transfer pauses");
        assert!(
            matches!(
                harness.service.find_peers(id).await,
                Err(AcquisitionError::NotLive)
            ),
            "a paused transfer is resumed, not re-announced"
        );
    }

    /// The pump ticks what the journal says is running, without being asked.
    ///
    /// This is the behaviour the whole queue depends on: an engine transfers in silence, so a row
    /// moves only because something reads it. Before this test the pump had no coverage at all —
    /// every other test called statistics directly — which is how a pump that stopped sampling went
    /// unnoticed while the app showed a frozen queue.
    #[tokio::test]
    async fn the_pump_ticks_a_live_download_without_being_asked() {
        let harness = harness().await;
        harness
            .start(1, description())
            .await
            .expect("the fixture starts a torrent");
        let reads = |engine: &FakeTorrentEngine| {
            engine
                .calls()
                .into_iter()
                .filter(|call| matches!(call, EngineCall::Stats(_)))
                .count()
        };
        let before = reads(&harness.engine);

        harness.service.start_pump();
        tokio::time::sleep(Duration::from_millis(1_300)).await;
        harness.service.stop_pump().await;

        let after = reads(&harness.engine);
        assert!(
            after > before,
            "the pump reads the engine on its own tick: {before} then {after}"
        );
    }

    /// A live row this process holds no handle for is re-added by the pump, not skipped.
    ///
    /// The journal is the state. Sampling the handle map instead meant a row whose handle was lost —
    /// a restart, a failed add, a poisoned lock — was never sampled again, and every update in the
    /// app stopped with no log line to say why.
    #[tokio::test]
    async fn a_live_row_without_a_handle_is_reattached_by_the_pump() {
        let harness = harness().await;
        let started = harness
            .start(1, description())
            .await
            .expect("the fixture starts a torrent");
        let id = started.job.id;
        harness
            .service
            .handles
            .lock()
            .expect("the handle map is not poisoned")
            .remove(&id);
        assert!(
            harness.service.handle_for(id).is_none(),
            "the fixture lost the handle it is about to re-add"
        );
        let adds = |engine: &FakeTorrentEngine| {
            engine
                .calls()
                .into_iter()
                .filter(|call| matches!(call, EngineCall::Add { .. }))
                .count()
        };
        let before = adds(&harness.engine);

        harness.service.sample().await;
        for _ in 0..40 {
            if harness.service.handle_for(id).is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        assert!(
            harness.service.handle_for(id).is_some(),
            "the pump re-added the row and kept its handle"
        );
        assert_eq!(
            adds(&harness.engine),
            before + 1,
            "exactly one re-add, with reattach semantics"
        );
    }

    #[tokio::test]
    async fn a_live_torrent_job_is_reattached_with_reattach_semantics() {
        let harness = harness().await;
        harness
            .start(1, description())
            .await
            .expect("the fixture starts a torrent");
        let before = harness.add_calls().len();

        let count = harness
            .service
            .reattach_torrents()
            .await
            .expect("the live torrent is reattached");
        assert_eq!(count, 1);
        let calls = harness.add_calls();
        assert_eq!(calls.len(), before + 1, "reattach adds once, not twice");
        let Some(EngineCall::Add { intent, .. }) = calls.last() else {
            panic!("the reattach is an add");
        };
        assert!(
            matches!(intent, AddIntent::Reattach { .. }),
            "the engine must verify the bytes already on disk"
        );
    }

    #[tokio::test]
    async fn a_live_usenet_job_is_reattached_from_its_stored_nzb() {
        let harness = usenet_harness().await;
        let nzb = "<nzb><file subject=\"movie.mkv\" date=\"0\" poster=\"p\">\
                   <segments><segment bytes=\"5\" number=\"1\">&lt;one@test&gt;</segment>\
                   </segments></file></nzb>";
        let document = NzbDocument::parse(nzb.as_bytes()).expect("the fixture parses");
        let servers = vec![ServerConfig {
            nntp: peregrine_usenet::NntpConfig {
                host: "news.example.test".to_owned(),
                port: 563,
                tls: true,
                username: "reader".to_owned(),
                password: None,
            },
            connections: 1,
            priority: 0,
        }];
        let started = harness
            .service
            .start_usenet(UsenetStartRequest {
                account: account(),
                job: usenet_job(harness.title_id),
                document,
                nzb_bytes: nzb.as_bytes().to_vec(),
                servers: servers.clone(),
            })
            .await
            .expect("the post starts");

        let guard = PathGuard::new(harness.directory.path().join("downloads"))
            .expect("the fixture root is absolute");
        let restarted = AcquisitionService::new(
            Arc::clone(&harness.database),
            Arc::new(FakeTorrentEngine::new()),
            Arc::clone(&harness.clock) as Arc<dyn Clock + Send + Sync>,
            guard,
            Arc::clone(&harness.volume) as Arc<dyn VolumeSpace>,
            FLOOR,
            Arc::clone(&harness.bus),
            TrashLog::new(),
        )
        .with_usenet(Arc::new(FakeUsenetEngine::new()));
        let count = restarted
            .reattach_usenet(servers)
            .await
            .expect("the live post is reattached");
        assert_eq!(count, 1);
        let stats = restarted
            .statistics(started.job.id)
            .await
            .expect("the reattached job answers")
            .expect("a live row");
        assert_eq!(stats.downloaded_bytes, 0);
        assert_eq!(stats.progress_permille, 0);
    }

    #[tokio::test]
    async fn a_usenet_post_that_finished_before_restart_is_imported_without_a_seed_edge() {
        let harness = usenet_harness().await;
        let nzb = "<nzb><file subject=\"movie.mkv\" date=\"0\" poster=\"p\">\
                   <segments><segment bytes=\"5\" number=\"1\">&lt;one@test&gt;</segment>\
                   </segments></file></nzb>";
        let document = NzbDocument::parse(nzb.as_bytes()).expect("the fixture parses");
        let servers = vec![ServerConfig {
            nntp: peregrine_usenet::NntpConfig {
                host: "news.example.test".to_owned(),
                port: 563,
                tls: true,
                username: "reader".to_owned(),
                password: None,
            },
            connections: 1,
            priority: 0,
        }];
        let started = harness
            .service
            .start_usenet(UsenetStartRequest {
                account: account(),
                job: usenet_job(harness.title_id),
                document,
                nzb_bytes: nzb.as_bytes().to_vec(),
                servers,
            })
            .await
            .expect("the post starts");
        harness
            .database
            .record_download_statistics(
                started.job.id,
                DownloadStatistics {
                    downloaded_bytes: 5,
                    uploaded_bytes: 0,
                    progress_permille: 1_000,
                    download_rate_bps: 0,
                    upload_rate_bps: 0,
                    peers_connected: 0,
                    peers_swarm: 0,
                    eta_seconds: None,
                    ratio: None,
                },
            )
            .await
            .expect("the tick writes");
        assert_eq!(
            harness
                .service
                .reconcile_finished()
                .await
                .expect("reconciled"),
            1
        );
        let row = harness
            .database
            .download_job(account(), started.job.id)
            .await
            .expect("read")
            .expect("row");
        assert_eq!(row.state, DownloadState::Completed);
    }

    #[tokio::test]
    async fn an_orphan_nzb_is_swept_but_a_live_job_keeps_its_own() {
        let harness = harness().await;
        harness
            .start(
                30,
                PayloadDescription::new(vec![PayloadEntry::new("movie.mkv", 1, EntryKind::File)]),
            )
            .await
            .expect("the job starts");
        let root = harness.directory.path().join("downloads");
        tokio::fs::create_dir_all(root.join("Movies"))
            .await
            .expect("the live directory");
        tokio::fs::write(root.join("Movies/source.nzb"), b"live")
            .await
            .expect("the live source");
        tokio::fs::create_dir_all(root.join("orphan"))
            .await
            .expect("the orphan directory");
        tokio::fs::write(root.join("orphan/source.nzb"), b"orphan")
            .await
            .expect("the orphan source");

        assert_eq!(harness.service.sweep_orphan_nzbs().await.expect("swept"), 1);
        assert!(root.join("Movies/source.nzb").is_file());
        assert!(!root.join("orphan/source.nzb").exists());
    }

    /// F6 draws inspection before the free-space check, and this is why: a torrent the gate has
    /// already refused says so even on a volume with no room. Answering "free some space" instead
    /// would send the user to do that for a download that still cannot start, and the report the
    /// gate already produced would be discarded unpersisted.
    #[tokio::test]
    async fn a_refused_torrent_says_so_even_when_the_volume_is_full() {
        let harness = harness().await;
        harness.volume.set(0);
        let archive =
            PayloadDescription::new(vec![PayloadEntry::new("extras.zip", 1, EntryKind::File)]);

        let started = harness
            .start(15, archive)
            .await
            .expect("the refusal is the answer, not the volume");

        assert_eq!(started.verdict, Verdict::OptInRequired);
        assert!(!started.added);
        assert_eq!(started.job.state, DownloadState::Blocked);
        assert!(
            started.job.safety_report.is_some(),
            "the report is persisted, so the blocked row explains itself after a restart"
        );
        assert_eq!(
            harness.add_calls(),
            Vec::new(),
            "nothing may reach the engine on a volume that cannot hold it"
        );
    }

    /// The same ordering on the consent path: admission is checked before the block is lifted, so a
    /// full volume leaves the job `blocked` — where the consent can be given again once there is
    /// room — rather than in `metadata`, which means a handoff is under way.
    #[tokio::test]
    async fn consent_on_a_full_volume_leaves_the_job_blocked() {
        let harness = harness().await;
        let archive =
            PayloadDescription::new(vec![PayloadEntry::new("extras.zip", 1, EntryKind::File)]);
        let blocked = harness
            .start(16, archive.clone())
            .await
            .expect("the gate parks the job");
        harness.volume.set(0);

        let error = harness
            .service
            .consent(blocked.job.id, &archive)
            .await
            .expect_err("a volume with no room does not admit the job");
        assert!(matches!(error, AcquisitionError::DiskFull { .. }));

        let row = harness
            .database
            .download_job(account(), blocked.job.id)
            .await
            .expect("the read answers")
            .expect("the row exists");
        assert_eq!(row.state, DownloadState::Blocked, "the block is not lifted");
        assert_eq!(harness.add_calls(), Vec::new());

        // And it can be consented again once the volume has room.
        harness.volume.set(1_000_000);
        assert_eq!(
            harness
                .service
                .consent(blocked.job.id, &archive)
                .await
                .expect("the second consent is legal"),
            DownloadState::Downloading
        );
    }

    /// A torrent refused after consent stays blocked and never reaches the engine — traversal,
    /// links, and programs re-inspect to `Blocked` however many times a user agrees.
    #[tokio::test]
    async fn a_torrent_refused_after_consent_stays_blocked() {
        let harness = harness().await;
        let traversal = PayloadDescription::new(vec![PayloadEntry::new(
            "../authorized_keys",
            1,
            EntryKind::File,
        )]);
        let blocked = harness
            .start(5, traversal.clone())
            .await
            .expect("the gate parks the job");

        let error = harness
            .service
            .consent(blocked.job.id, &traversal)
            .await
            .expect_err("a hard block has no override");
        assert!(matches!(error, AcquisitionError::NoOverride));

        // And the report the caller could hand in without re-inspecting is refused by the domain.
        let stale = PayloadInspector::new(InspectionLimits::new(1_000, u64::MAX)).inspect(
            &archive_for_stale(),
            &PathGuard::new(harness.directory.path().join("downloads")).expect("absolute"),
            OptIn::NotGranted,
        );
        assert_eq!(stale.verdict(), Verdict::OptInRequired);
        let error = harness
            .service
            .apply(blocked.job.id, DownloadEvent::Consented(stale))
            .await
            .expect_err("a report that still needs the opt-in cannot lift a block");
        assert!(matches!(error, AcquisitionError::TransitionRefused { .. }));

        let row = harness
            .database
            .download_job(account(), blocked.job.id)
            .await
            .expect("the read answers")
            .expect("the row exists");
        assert_eq!(row.state, DownloadState::Blocked);
        assert_eq!(harness.add_calls(), Vec::new());
    }

    /// A description whose opt-in report is not granted, used to drive the domain's own refusal.
    fn archive_for_stale() -> PayloadDescription {
        PayloadDescription::new(vec![PayloadEntry::new("extras.zip", 1, EntryKind::File)])
    }

    /// Two live jobs for one info hash is a typed conflict; the same release after `removed` starts
    /// cleanly, because a terminal job vacates the dedupe key.
    #[tokio::test]
    async fn a_second_live_job_for_one_hash_conflicts_and_a_removed_one_does_not() {
        let harness = harness().await;
        let first = harness
            .start(6, description())
            .await
            .expect("the first start succeeds");

        let error = harness
            .start(6, description())
            .await
            .expect_err("a live job holds the hash");
        assert!(matches!(error, AcquisitionError::Duplicate));

        let state = harness
            .service
            .remove(first.job.id)
            .await
            .expect("a downloading job can be removed");
        assert_eq!(state, DownloadState::Removed);

        let second = harness
            .start(6, description())
            .await
            .expect("the same release starts again after a removal");
        assert_ne!(second.job.id, first.job.id, "a retry is a new job");
        assert!(second.added);
        assert_eq!(harness.add_calls().len(), 2);
    }

    /// Duplicate commands are out of order at the job and idempotent at the engine, and the two
    /// answers stay distinguishable.
    #[tokio::test]
    async fn duplicate_commands_are_refused_by_the_state_machine() {
        let harness = harness().await;
        let started = harness.start(7, description()).await.expect("it starts");

        assert_eq!(
            harness
                .service
                .pause(started.job.id)
                .await
                .expect("the first pause is legal"),
            DownloadState::Paused
        );
        let error = harness
            .service
            .pause(started.job.id)
            .await
            .expect_err("a second pause is out of order");
        assert!(matches!(
            error,
            AcquisitionError::TransitionRefused { ref event, .. } if event == "paused"
        ));

        // Resume is the legal edge out of `paused`, and the engine answers it idempotently. What
        // must *not* be accepted is a resume of a job that never paused, which is the next test:
        // the fake's `resume` is a level, so only the job's own state machine can refuse that.
        assert_eq!(
            harness
                .service
                .resume(started.job.id)
                .await
                .expect("a paused job resumes"),
            DownloadState::Downloading
        );
        assert!(
            harness
                .service
                .statistics(started.job.id)
                .await
                .expect("a tick on a live job answers")
                .is_some(),
            "the job is live again and tickable"
        );
    }

    /// A resume of a job that never paused is refused, and the engine is left alone.
    #[tokio::test]
    async fn a_resume_of_a_running_job_is_refused() {
        let harness = harness().await;
        let started = harness.start(8, description()).await.expect("it starts");

        let error = harness
            .service
            .resume(started.job.id)
            .await
            .expect_err("the job is already downloading");
        assert!(matches!(
            error,
            AcquisitionError::TransitionRefused { ref event, .. } if event == "resumed"
        ));
        assert_eq!(
            harness
                .database
                .download_job(account(), started.job.id)
                .await
                .expect("the read answers")
                .expect("the row exists")
                .state,
            DownloadState::Downloading
        );
    }

    /// `remove` twice: the first moves the job, the second is refused by the absorbing rule.
    #[tokio::test]
    async fn removing_twice_is_refused_after_the_first_removal() {
        let harness = harness().await;
        let started = harness.start(9, description()).await.expect("it starts");

        assert_eq!(
            harness.service.remove(started.job.id).await.expect("legal"),
            DownloadState::Removed
        );
        let error = harness
            .service
            .remove(started.job.id)
            .await
            .expect_err("removed is terminal");
        assert!(matches!(
            error,
            AcquisitionError::TransitionRefused { ref event, .. } if event == "removed"
        ));
    }

    /// A statistics tick after a terminal state is refused rather than written, so a caller whose
    /// coalescing is behind learns it.
    #[tokio::test]
    async fn a_statistics_tick_after_a_terminal_state_is_refused() {
        let harness = harness().await;
        let started = harness.start(10, description()).await.expect("it starts");

        harness
            .service
            .apply(started.job.id, DownloadEvent::EngineFailed)
            .await
            .expect("the engine failed");
        harness.advance();

        let error = harness
            .service
            .statistics(started.job.id)
            .await
            .expect_err("a terminal job takes no statistics");
        assert!(matches!(
            error,
            AcquisitionError::TransitionRefused { ref event, .. } if event == "progress"
        ));
        assert_eq!(
            harness
                .database
                .download_job(account(), started.job.id)
                .await
                .expect("the read answers")
                .expect("the row exists")
                .state,
            DownloadState::Failed
        );
    }

    /// Every way the engine can refuse the add leaves the row consistent and answers with a typed
    /// error that says what the caller can do.
    #[tokio::test]
    async fn an_engine_refusal_leaves_the_row_consistent() {
        for (operation, error) in [
            (EngineOperation::Add, EngineError::Failed),
            (
                EngineOperation::Add,
                EngineError::UnknownTorrent {
                    handle: TorrentHandle::new(7),
                },
            ),
            (EngineOperation::Add, EngineError::SessionClosed),
        ] {
            let harness = harness().await;
            let engine = Arc::new(
                FakeTorrentEngine::new()
                    .with_files(2, Bytes::new(1_000))
                    .failing_with(operation, error),
            );
            let guard =
                PathGuard::new(harness.directory.path().join("downloads")).expect("absolute root");
            let service = AcquisitionService::new(
                Arc::clone(&harness.database),
                Arc::clone(&engine) as Arc<dyn TorrentEngine>,
                Arc::clone(&harness.clock) as Arc<dyn Clock + Send + Sync>,
                guard,
                Arc::clone(&harness.volume) as Arc<dyn VolumeSpace>,
                FLOOR,
                bus(&harness.sink),
                trash(),
            );

            let answer = service
                .start(request(harness.title_id, 11, description()))
                .await;
            let refused = answer.expect_err("the engine refused the add");
            assert!(
                matches!(refused, AcquisitionError::Engine(_)),
                "a scripted {error:?} is a typed engine refusal, found {refused:?}"
            );

            let rows = harness.rows().await;
            assert_eq!(
                rows.len(),
                1,
                "the row that admitted the job is still there"
            );
            assert_eq!(
                rows[0].state,
                DownloadState::Metadata,
                "an add that never happened cannot have advanced the job"
            );
            assert!(
                rows[0].safety_report.is_some(),
                "and its report is persisted"
            );
        }
    }

    /// `ARCHITECTURE` §15: a full disk refuses admission before an add, and an active transfer
    /// pauses rather than fails and can resume.
    #[tokio::test]
    async fn a_full_disk_refuses_admission_and_pauses_an_active_transfer() {
        let harness = harness().await;
        harness.volume.set(FLOOR + 1);
        let error = harness
            .start(12, description())
            .await
            .expect_err("the volume cannot hold the payload");
        match error {
            AcquisitionError::DiskFull {
                required_bytes,
                available_bytes,
            } => {
                assert_eq!(required_bytes, 2_000);
                assert_eq!(available_bytes, 1);
            }
            other => panic!("expected a full disk, found {other:?}"),
        }
        assert!(
            harness.add_calls().is_empty(),
            "nothing may begin writing on a volume that cannot hold the payload"
        );
        let rows = harness.rows().await;
        assert_eq!(rows.len(), 1, "the job row explains the refusal");
        assert_eq!(rows[0].state, DownloadState::Queued);

        // Now the transfer starts, and the volume fills up underneath it.
        harness.volume.set(1_000_000);
        let started = harness
            .start(13, description())
            .await
            .expect("the volume admits the job");
        assert_eq!(started.state, DownloadState::Downloading);

        harness.volume.set(FLOOR - 1);
        harness.advance();
        harness
            .service
            .statistics(started.job.id)
            .await
            .expect("the tick answers");
        let paused = harness
            .database
            .download_job(account(), started.job.id)
            .await
            .expect("the read answers")
            .expect("the row exists");
        assert_eq!(paused.state, DownloadState::Paused);
        assert_ne!(
            paused.state,
            DownloadState::Failed,
            "a full disk is not a failure"
        );

        harness.volume.set(1_000_000);
        assert_eq!(
            harness
                .service
                .resume(started.job.id)
                .await
                .expect("the resume is legal"),
            DownloadState::Downloading
        );
    }

    /// `AGENTS` R34: an out-of-scope title and a nonexistent one are the same refusal, so a
    /// restricted account cannot tell "not for you" from "no such thing".
    #[tokio::test]
    async fn an_out_of_scope_title_and_a_nonexistent_one_are_the_same_refusal() {
        let harness = harness().await;
        let restricted = AccountScope::restricted(AccountId::new(2).expect("account two"));

        let mut unrated = request(harness.title_id, 14, description());
        unrated.account = restricted;
        let hidden = harness
            .service
            .start(unrated)
            .await
            .expect_err("a title with no rating is hidden from a restricted account");

        let mut missing = request(9_999, 14, description());
        missing.account = restricted;
        let absent = harness
            .service
            .start(missing)
            .await
            .expect_err("no such title");

        assert!(matches!(hidden, AcquisitionError::TitleUnavailable));
        assert!(matches!(absent, AcquisitionError::TitleUnavailable));
        assert_eq!(hidden.to_string(), absent.to_string());
        assert!(
            harness.add_calls().is_empty(),
            "neither refusal reached the engine"
        );
        assert!(
            harness.rows().await.is_empty(),
            "and neither inserted a row"
        );
    }

    /// A destination that does not resolve never inserts a row, so a job that could never be added
    /// leaves nothing behind to explain.
    #[tokio::test]
    async fn an_unresolvable_save_path_never_inserts_a_row() {
        let harness = harness().await;
        let mut escaping = request(harness.title_id, 15, description());
        escaping.job.save_path = text("../somewhere-else");

        let error = harness
            .service
            .start(escaping)
            .await
            .expect_err("the destination escapes the root");
        assert!(matches!(error, AcquisitionError::SavePath { .. }));
        assert!(harness.rows().await.is_empty(), "no row was inserted");
        assert!(
            harness.add_calls().is_empty(),
            "and the engine was never asked"
        );
    }

    /// The progress budget: a tick inside the second writes nothing and publishes nothing, the tick
    /// that crosses it writes once, and the full-disk check is part of the same call.
    #[tokio::test]
    async fn statistics_are_coalesced_to_one_write_a_second() {
        let harness = harness().await;
        let started = harness
            .start(16, description())
            .await
            .expect("the job starts");

        assert!(
            harness
                .service
                .statistics(started.job.id)
                .await
                .expect("the first tick answers")
                .is_some()
        );
        assert!(
            harness
                .service
                .statistics(started.job.id)
                .await
                .expect("a tick inside the budget answers")
                .is_none(),
            "a tick inside the budget is coalesced"
        );

        harness.advance();
        assert!(
            harness
                .service
                .statistics(started.job.id)
                .await
                .expect("the tick past the budget answers")
                .is_some()
        );

        let stats = harness
            .engine
            .calls()
            .into_iter()
            .filter(|call| matches!(call, EngineCall::Stats(_)))
            .count();
        assert_eq!(stats, 2, "a coalesced tick does not even read the engine");
        assert_eq!(
            harness.sink.delivered().len(),
            0,
            "delivery is the bus's 250 ms tick, not this call's"
        );
    }

    /// The pump sweeps every live job in one pass, so a queue of two transfers advances together
    /// rather than one of them waiting for the other to stop.
    #[tokio::test]
    async fn sample_reads_every_live_job() {
        let harness = harness().await;
        let first = harness
            .start(17, description())
            .await
            .expect("the first job starts");
        let second = harness
            .start(18, description())
            .await
            .expect("the second job starts");

        harness.service.sample().await;

        let reads = harness
            .engine
            .calls()
            .into_iter()
            .filter(|call| matches!(call, EngineCall::Stats(_)))
            .count();
        assert_eq!(reads, 2, "the sweep asks each live job once");
        let rows = harness.rows().await;
        assert!(
            rows.iter().any(|row| row.id == first.job.id)
                && rows.iter().any(|row| row.id == second.job.id),
            "both jobs are still queued"
        );
    }

    /// A whole payload finishes the job, imports what it wrote, stops the swarm, and can then be
    /// deleted: the three edges the queue screen and the title page need.
    #[tokio::test]
    async fn a_complete_payload_is_imported_and_can_be_deleted() {
        let harness = harness().await;
        let engine = Arc::new(
            FakeTorrentEngine::new()
                .with_files(2, Bytes::new(1_000))
                .with_downloaded(Bytes::new(2_000)),
        );
        let guard = PathGuard::new(harness.directory.path().join("downloads")).expect("absolute");
        let trash = TrashLog::new();
        let notices = NoticeLog::new();
        let service = AcquisitionService::new(
            Arc::clone(&harness.database),
            Arc::clone(&engine) as Arc<dyn TorrentEngine>,
            Arc::clone(&harness.clock) as Arc<dyn Clock + Send + Sync>,
            guard,
            Arc::clone(&harness.volume) as Arc<dyn VolumeSpace>,
            FLOOR,
            bus(&harness.sink),
            Arc::clone(&trash) as Arc<dyn Trash>,
        )
        .with_notices(Arc::clone(&notices) as Arc<dyn DownloadNotices>);
        let started = service
            .start(request(harness.title_id, 21, description()))
            .await
            .expect("the job starts");

        // The fake engine moves counters and writes nothing, so the test writes what it would have.
        let folder = harness.directory.path().join("downloads").join("Movies");
        std::fs::create_dir_all(&folder).expect("the download folder");
        let media = folder.join("Movie.2024.1080p.mkv");
        std::fs::write(&media, vec![0_u8; 1_000]).expect("the media file");
        std::fs::write(folder.join("Movie.2024.1080p.srt"), b"subs!").expect("the subtitle");

        let statistics = service
            .statistics(started.job.id)
            .await
            .expect("the tick answers")
            .expect("the tick is not coalesced");
        assert_eq!(statistics.progress_permille, 1_000);

        let row = harness
            .database
            .download_job(account(), started.job.id)
            .await
            .expect("the read answers")
            .expect("the row is there");
        assert_eq!(
            row.state,
            DownloadState::Completed,
            "a whole payload is a finished job"
        );
        assert_eq!(notices.seen(), vec![(started.job.id, false)]);

        // The media file is in the library; the subtitle is not a media file and is not imported.
        let media_path = media.to_string_lossy().into_owned();
        let imported = harness
            .database
            .media_file_by_path(account(), &media_path)
            .await
            .expect("the read answers")
            .expect("the completed media file is in the library");
        let item = harness
            .database
            .item_id_for(harness.title_id, None)
            .await
            .expect("the read answers")
            .expect("the item was created");
        assert_eq!(imported.media_item_id, item);
        assert!(
            engine
                .calls()
                .iter()
                .any(|call| matches!(call, EngineCall::Remove(_))),
            "the swarm is stopped rather than left seeding"
        );

        // The plan is the files the engine wrote, and nothing has gone yet.
        let plan = service
            .deletion_plan(started.job.id)
            .await
            .expect("the plan answers");
        assert_eq!(plan.len(), 2);
        assert_eq!(plan.total_bytes, 1_005);

        let deleted = service
            .delete_files(started.job.id)
            .await
            .expect("the deletion runs");
        assert_eq!(deleted.len(), 2);
        assert!(!media.exists(), "the media file is gone");
        assert_eq!(trash.taken().len(), 2, "both files went to the trash");
        assert!(
            harness
                .database
                .download_job(account(), started.job.id)
                .await
                .expect("the read answers")
                .is_none(),
            "the queue entry is gone with its files"
        );
        assert!(
            harness
                .database
                .media_file_by_path(account(), &media_path)
                .await
                .expect("the read answers")
                .is_none(),
            "the library record goes with the bytes"
        );
    }

    /// An episode imported with no still gets one from the file that was just imported.
    ///
    /// The row records the capture, the blob is in the cache under the episode's own key, and the
    /// media file is imported as it always was.
    #[tokio::test]
    async fn an_imported_episode_without_a_still_captures_one() {
        let harness = still_harness(FakeCapture::new()).await;
        let capture = Arc::clone(&harness.capture);

        harness
            .download(
                harness.episode_request(31),
                &["VisionQuest.S01E03.1080p.mkv"],
            )
            .await;

        assert_eq!(capture.calls(), 1, "one episode asks the port once");
        let recorded = harness.recorded(harness.episode_three).await;
        assert_eq!(recorded.provider, CAPTURED_STILL_PROVIDER);
        assert_eq!(
            recorded.path,
            harness.episode_three.to_string(),
            "the path is the episode's own, which is the cache key's path"
        );
        let key = ImageKey::for_captured_still(harness.episode_three);
        assert!(
            harness.cache.contains(&key),
            "the captured bytes are in the same cache a provider still uses"
        );
        let entry = harness
            .database
            .image_cache_entry(key.as_str())
            .await
            .expect("the read answers")
            .expect("the cache row is written");
        assert_eq!(entry.provider, CAPTURED_STILL_PROVIDER);
        assert_eq!(entry.variant, "w300");
    }

    /// A still a provider recorded is never captured over, and the port is not called at all.
    #[tokio::test]
    async fn an_episode_that_already_has_a_still_is_never_captured() {
        let harness = still_harness(FakeCapture::new()).await;
        harness
            .database
            .set_episode_still(
                harness.episode_three,
                "tmdb",
                "/s01e03.jpg",
                DEFAULT_EPOCH_MILLISECONDS,
            )
            .await
            .expect("the provider still is recorded");
        let capture = Arc::clone(&harness.capture);

        harness
            .download(
                harness.episode_request(32),
                &["VisionQuest.S01E03.1080p.mkv"],
            )
            .await;

        assert_eq!(
            capture.calls(),
            0,
            "an episode with a still spends no capture"
        );
        let recorded = harness.recorded(harness.episode_three).await;
        assert_eq!(recorded.provider, "tmdb");
        assert_eq!(recorded.path, "/s01e03.jpg");
    }

    /// A capture that fails is a warning: the episode keeps no still and the import still lands.
    #[tokio::test]
    async fn a_failed_capture_leaves_the_still_null_and_keeps_the_import() {
        let harness = still_harness(FakeCapture::failing()).await;
        let capture = Arc::clone(&harness.capture);

        let started = harness
            .download(
                harness.episode_request(33),
                &["VisionQuest.S01E03.1080p.mkv"],
            )
            .await;

        assert_eq!(capture.calls(), 1);
        assert_eq!(
            harness
                .database
                .episode_still(harness.episode_three)
                .await
                .expect("the read answers"),
            None,
            "a failed capture leaves the column exactly as NULL as it was"
        );
        let media = harness
            .directory
            .path()
            .join("downloads")
            .join("Movies")
            .join("VisionQuest.S01E03.1080p.mkv");
        assert!(
            harness
                .database
                .media_file_by_path(account(), media.to_string_lossy().as_ref())
                .await
                .expect("the read answers")
                .is_some(),
            "the file is imported despite the failed capture"
        );
        assert_eq!(
            harness
                .database
                .download_job(account(), started.job.id)
                .await
                .expect("the read answers")
                .expect("the job row is there")
                .state,
            DownloadState::Completed
        );
    }

    /// A pack's files belong to their own episodes, so the capture is per episode rather than per job.
    #[tokio::test]
    async fn a_pack_import_captures_each_episode_rather_than_the_job() {
        let harness = still_harness(FakeCapture::new()).await;
        let capture = Arc::clone(&harness.capture);
        let names = [
            "VisionQuest.S01E03.1080p.mkv",
            "VisionQuest.S01E04.1080p.mkv",
        ];
        let mut pack = request(harness.title_id, 34, episode_files(&names));
        pack.job.episode_id = None;

        harness.download(pack, &names).await;

        assert_eq!(capture.calls(), 2, "each episode asks the port for itself");
        for episode_id in [harness.episode_three, harness.episode_four] {
            assert_eq!(
                harness.recorded(episode_id).await.provider,
                CAPTURED_STILL_PROVIDER,
                "episode {episode_id} records its own capture"
            );
            assert!(
                harness
                    .cache
                    .contains(&ImageKey::for_captured_still(episode_id)),
                "episode {episode_id} has its own blob"
            );
        }
    }

    /// A job this process does not hold has no handle, which is a different answer from a state
    /// refusal: reattaching one is PG-E10-08's, and Resume is the action that does it.
    #[tokio::test]
    async fn operations_on_a_job_this_process_does_not_hold_are_not_live() {
        let harness = harness().await;
        let error = harness.service.pause(404).await.expect_err("no such job");
        assert!(matches!(error, AcquisitionError::NotLive));

        let blocked = harness
            .start(
                17,
                PayloadDescription::new(vec![PayloadEntry::new("extras.zip", 1, EntryKind::File)]),
            )
            .await
            .expect("it parks");
        let error = harness
            .service
            .pause(blocked.job.id)
            .await
            .expect_err("a blocked job has no engine handle");
        assert!(matches!(error, AcquisitionError::NotLive));
    }

    /// The report the engine rejects can never be built: a `.torrent`-file row needs the bytes, and
    /// this slice refuses it rather than reading a path the engine may not receive.
    #[tokio::test]
    async fn a_torrent_file_row_is_refused_rather_than_read() {
        let harness = harness().await;
        let mut file_sourced = request(harness.title_id, 18, description());
        file_sourced.job.source = DownloadSource::TorrentFile(text("/tmp/not-read.torrent"));

        let error = harness
            .service
            .start(file_sourced)
            .await
            .expect_err("the contents belong to the adapter that reads them");
        assert!(matches!(error, AcquisitionError::UnsupportedSource));
        assert_eq!(harness.add_calls(), Vec::new());
        assert_eq!(
            harness.rows().await[0].state,
            DownloadState::Metadata,
            "the gate had already allowed it, so the row is parked where it can be retried"
        );
    }

    /// Every path the engine can be handed came through the guard, so the row's store is absolute.
    #[tokio::test]
    async fn the_stored_destination_is_the_guarded_absolute_path() {
        let harness = harness().await;
        let started = harness.start(19, description()).await.expect("it starts");
        let stored = PathBuf::from(started.job.save_path.as_str());
        assert!(stored.is_absolute(), "the row holds an absolute path");
        assert!(stored.starts_with(harness.directory.path()));
    }

    /// A volume that cannot be asked refuses admission rather than assuming room: adding a torrent
    /// of unknown size to a full disk is the failure the floor exists to prevent.
    #[tokio::test]
    async fn an_unreadable_volume_refuses_admission() {
        let harness = harness().await;
        let guard = PathGuard::new(harness.directory.path().join("downloads")).expect("absolute");
        let service = AcquisitionService::new(
            Arc::clone(&harness.database),
            Arc::clone(&harness.engine) as Arc<dyn TorrentEngine>,
            Arc::clone(&harness.clock) as Arc<dyn Clock + Send + Sync>,
            guard,
            Arc::new(UnknownVolume) as Arc<dyn VolumeSpace>,
            FLOOR,
            bus(&harness.sink),
            trash(),
        );

        let error = service
            .start(request(harness.title_id, 20, description()))
            .await
            .expect_err("the volume could not be asked");
        assert!(matches!(error, AcquisitionError::SpaceUnknown { .. }));
        assert_eq!(harness.add_calls(), Vec::new());
        assert_eq!(harness.rows().await[0].state, DownloadState::Queued);
    }

    /// A volume that cannot answer at all.
    struct UnknownVolume;

    impl VolumeSpace for UnknownVolume {
        fn available_bytes(&self, _path: &Path) -> Result<u64, SpaceError> {
            Err(SpaceError::Unreachable)
        }
    }

    /// One accepted tick reaches the bus on the topic `FLOWS` F6 names, and a coalesced one does
    /// not — the delivery rate itself is the bus's own budget, not this call's.
    #[tokio::test]
    async fn an_accepted_tick_publishes_on_the_download_topic() {
        let harness = harness().await;
        let started = harness.start(21, description()).await.expect("it starts");

        assert!(
            harness
                .service
                .statistics(started.job.id)
                .await
                .expect("the first tick answers")
                .is_some()
        );
        assert!(
            harness
                .service
                .statistics(started.job.id)
                .await
                .expect("a coalesced tick answers")
                .is_none()
        );

        // The bus delivers on its own 250 ms tick, so a test that stopped at the tick would be
        // waiting on a real clock. `shutdown` flushes whatever is pending before the sink goes
        // away, which is the same "final delivery" an orderly stop performs (`PG-E2-11`), so the
        // assertion below is exact and instant (`AGENTS` R14).
        harness.bus.shutdown().await;
        let delivered = harness.sink.delivered();
        let events: Vec<&Event> = delivered
            .iter()
            .filter(|event| event.topic == DOWNLOAD_STATS)
            .collect();
        assert_eq!(
            events.len(),
            1,
            "one accepted tick, and the coalesced one published nothing"
        );
        assert_eq!(events[0].key, started.job.id.to_string());
        assert_eq!(
            events[0].payload["job_id"],
            serde_json::json!(started.job.id),
            "the payload carries the job the tick was for"
        );
    }
}
