//! `AcquisitionService`: the write half of `FLOWS` F6.
//!
//! The one place the inspection gate, the torrent engine port, and the download repositories are
//! combined. It is deliberately not the whole of acquisition: the metadata-only fetch that produces
//! a [`TorrentDescription`] is `PG-E10-03`'s, the release decision that chooses one is `PG-E12-10`'s,
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
//! 4. `Admitted` → `inspecting`, then the real [`TorrentInspector`] runs over the description the
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

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jiff::Timestamp;
use peregrine_domain::{
    AccountScope, Clock, DeletionEntry, DeletionPlan, DownloadEvent, DownloadState, InfoHash,
    InspectionLimits, MediaFile, MediaItem, MediaItemState, MediaOrigin, NonEmptyText, OptIn,
    PathGuard, PathGuardError, SafePath, SafetyReport, TorrentDescription, TorrentInspector,
    TransitionRejected, Trash, TrashError, Verdict,
};
use peregrine_storage::{
    Database, DownloadFileRow, DownloadJobRow, DownloadSource, DownloadStatistics,
    DownloadTransitionError, NewDownloadJob, PageRequest, StorageError,
};
use peregrine_torrent::{
    AddIntent, AddRefused, AddRequest, EngineError, FileSelection, TorrentEngine, TorrentHandle,
    TorrentSource,
};
use thiserror::Error;
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

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
    pub description: TorrentDescription,
    /// The torrent's identity, which is also the dedupe key.
    pub info_hash: InfoHash,
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
    /// The handles this process holds, so an engine operation after an add can name its torrent
    /// without the row carrying a number. A job started by another process has no handle here and
    /// is `PG-E10-08`'s to reattach.
    handles: Mutex<HashMap<i64, TorrentHandle>>,
    /// The last instant each job's statistics reached the database, which is the whole of the
    /// coalescing state.
    written_at: Mutex<HashMap<i64, Timestamp>>,
    /// The statistics pump, started by [`AcquisitionService::start_pump`] and `None` until then.
    ///
    /// The engine is pull-only: it transfers without telling anyone, so without this a live
    /// transfer writes nothing — the row stays at zero and `download://stats` never fires. The
    /// composition root starts it once a real engine exists and stops it before the bus; a test
    /// that drives [`AcquisitionService::statistics`] directly never starts one.
    pump: Mutex<Option<Pump>>,
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
            handles: Mutex::new(HashMap::new()),
            written_at: Mutex::new(HashMap::new()),
            pump: Mutex::new(None),
        }
    }

    /// The live engine handle for a job this process started, when it still holds one.
    ///
    /// A job started by another process has no handle here — `PG-E10-08`'s reattach is what finds it
    /// again — so `None` is "not this session's to read", not "no such job".
    #[must_use]
    pub fn handle_for(&self, job_id: i64) -> Option<TorrentHandle> {
        self.handles.lock().ok()?.get(&job_id).copied()
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
                    _ = ticker.tick() => service.sample().await,
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

    /// Asks every job this session holds for one statistics tick.
    ///
    /// The ids are taken under the lock and the lock is released before any await, so a slow
    /// engine read never blocks a pause or an add. A tick the domain refuses — a job that went
    /// terminal between the read and the sample — is logged and skipped rather than ending the
    /// sweep: one finished job must not stop every other transfer's progress.
    async fn sample(&self) {
        let ids: Vec<i64> = {
            let handles = match self.handles.lock() {
                Ok(handles) => handles,
                Err(poisoned) => poisoned.into_inner(),
            };
            handles.keys().copied().collect()
        };
        for id in ids {
            if let Err(error) = self.statistics(id).await {
                debug!(%error, job = id, "a download statistics tick was refused");
            }
        }
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
        // 1. The scope gate. `None` covers both "no such title" and "not for you", and the refusal
        //    below is one sentence for both (AGENTS R34).
        if self
            .database
            .title_identity(request.account, request.job.title_id)
            .await?
            .is_none()
        {
            return Err(AcquisitionError::TitleUnavailable);
        }

        // 2. The destination, before a row exists to point at it.
        let save_path = self
            .destination
            .resolve(std::path::Path::new(request.job.save_path.as_str()))
            .map_err(|source| AcquisitionError::SavePath { source })?;

        // 3. The gate, over the description the caller already has. Its caps come from the same
        //    floor admission uses, so a torrent too large for the volume is refused by inspection
        //    rather than after it.
        let limits = self.limits_for(&save_path);
        let report = TorrentInspector::new(limits).inspect(
            &request.description,
            &self.destination,
            OptIn::NotGranted,
        );
        let verdict = report.verdict();
        let selection: Vec<usize> = (0..request.description.entries().len()).collect();

        // 4. The row, inserted `queued`. The state literal is the repository's, so no caller can
        //    admit a job anywhere else.
        let mut job = request.job;
        job.info_hash = Some(request.info_hash);
        job.save_path = text_of(&save_path)?;
        job.added_at = self.clock.now();
        let row = match self.database.create_download_job(job.clone()).await {
            Ok(row) => row,
            Err(StorageError::Conflict { .. }) => return Err(AcquisitionError::Duplicate),
            Err(error) => return Err(AcquisitionError::Storage(error)),
        };

        // 5. Admission's free-space floor, for a job that could actually run. A job the volume
        //    cannot hold stays `queued`, which is what SM1 makes of it: it was admitted to the
        //    queue and never to a transfer, so nothing has to be undone. A torrent the gate has
        //    already refused is not admission's to mask — the verdict was computed in step 3, and
        //    answering "free some space" would send the user to do that for a download that still
        //    cannot start.
        if verdict == Verdict::Allowed {
            self.admit_space(&save_path, request.description.total_bytes())?;
        } else if report.total_bytes() > limits.max_total_bytes() {
            // The cap is the volume's own room, so a size refusal is the same fact admission
            // reports with numbers; \`DiskFull\` is the reason a person can act on, and it leaves
            // the row \`queued\` rather than parked in \`blocked\`.
            self.admit_space(&save_path, request.description.total_bytes())?;
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
        self.hand_off(
            row.id,
            &save_path,
            &selection,
            &request.description,
            &report,
        )
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
            Ok(Some(state)) => Ok(state),
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
        description: &TorrentDescription,
    ) -> Result<Verdict, AcquisitionError> {
        let resolved = self
            .destination
            .resolve(std::path::Path::new(save_path))
            .map_err(|source| AcquisitionError::SavePath { source })?;
        Ok(TorrentInspector::new(self.limits_for(&resolved))
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
        description: &TorrentDescription,
        report: &SafetyReport,
    ) -> Result<TorrentHandle, AcquisitionError> {
        let row = self.reload(id).await?;
        let source = match row.source.as_ref() {
            Some(DownloadSource::Magnet(uri)) => TorrentSource::Magnet(
                NonEmptyText::new(uri.as_str()).map_err(|_| AcquisitionError::UnsupportedSource)?,
            ),
            // The engine is handed contents and never a path outside the download root
            // (ADR 0005), so a row that stores a path needs the adapter that reads it.
            Some(DownloadSource::TorrentFile(_)) | None => {
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
        let handle = self.engine.add(&request).await?;
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
        description: &TorrentDescription,
    ) -> Result<DownloadState, AcquisitionError> {
        let row = self.reload(id).await?;
        let save_path = self
            .destination
            .resolve(std::path::Path::new(row.save_path.as_str()))
            .map_err(|source| AcquisitionError::SavePath { source })?;
        let limits = self.limits_for(&save_path);
        let report =
            TorrentInspector::new(limits).inspect(description, &self.destination, OptIn::Granted);
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
    /// [`AcquisitionError::NotFound`] when this process holds no handle for the job — a job started
    /// before a restart is `PG-E10-08`'s to reattach — and the engine and domain refusals.
    pub async fn pause(&self, id: i64) -> Result<DownloadState, AcquisitionError> {
        let handle = self.handle(id)?;
        let state = self.apply(id, DownloadEvent::Paused).await?;
        self.engine.pause(handle).await?;
        Ok(state)
    }

    /// Restarts one stopped transfer from the bytes already on disk.
    ///
    /// The engine call comes first here, so a resume the engine cannot carry out cannot leave a row
    /// that says `downloading` while nothing transfers — which is the failure a user cannot
    /// diagnose. A resume of a job that never paused is [`AcquisitionError::TransitionRefused`].
    pub async fn resume(&self, id: i64) -> Result<DownloadState, AcquisitionError> {
        let handle = self.handle(id)?;
        self.engine.resume(handle).await?;
        self.apply(id, DownloadEvent::Resumed).await
    }

    /// Removes one job, from any non-terminal state.
    ///
    /// `FLOWS` SM1 gives `removed` an edge from every state a user can still be waiting in, so a job
    /// parked behind the active set is cancellable too. The engine's removal is idempotent and runs
    /// only for a job this process actually added; the domain's transition is what refuses a second
    /// removal after the fact.
    pub async fn remove(&self, id: i64) -> Result<DownloadState, AcquisitionError> {
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
        }
        Ok(Some(statistics))
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

    /// Gives the library the media files a completed download wrote.
    ///
    /// The full import (PG-E14-19) owns probing, naming, and episode placement. This is the part a
    /// completed job can do without any of them: the title the job was started for is already
    /// known, so the item is that title, and each selected media entry that is on disk becomes a
    /// `media_file` row. A file the engine has not flushed is skipped rather than failing the
    /// import, and a path that is already recorded is left alone (`media_file.path` is UNIQUE, I3).
    async fn import_completed(&self, row: &DownloadJobRow) -> Result<(), AcquisitionError> {
        let files = self.database.download_files(unrestricted(), row.id).await?;
        let at = self.clock.now();
        let folder = std::path::Path::new(row.save_path.as_str());
        let mut item_id = self
            .database
            .item_id_for(row.title_id, row.episode_id)
            .await?;

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
            let id = if let Some(id) = item_id {
                id
            } else {
                let created = self
                    .database
                    .insert_media_item(MediaItem {
                        id: 0,
                        title_id: row.title_id,
                        episode_id: row.episode_id,
                        state: MediaItemState::Available,
                        origin: MediaOrigin::Torrent,
                        preferred_file_id: None,
                        quality_label: None,
                        created_at: at,
                        updated_at: at,
                    })
                    .await?;
                item_id = Some(created);
                created
            };
            let Ok(path) = NonEmptyText::new(safe.as_path().to_string_lossy().as_ref()) else {
                continue;
            };
            if self
                .database
                .media_file_by_path(unrestricted(), path.as_str())
                .await?
                .is_some()
            {
                continue;
            }
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
                    mtime_ms: Timestamp::from_millisecond(modified_millis(&metadata)).unwrap_or(at),
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
        Ok(())
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

    use peregrine_domain::{AccountId, EntryKind, InfoHash, TorrentEntry};
    use peregrine_storage::{AddedBy, MagnetUri, PageRequest};
    use peregrine_testkit::{
        DEFAULT_EPOCH_MILLISECONDS, EngineCall, EngineOperation, FakeClock, FakeTorrentEngine,
    };
    use peregrine_torrent::{Bytes, EngineError};
    use tempfile::TempDir;

    use super::*;
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

    /// The floor every fixture reserves.
    const FLOOR: u64 = 1_000;

    /// One torrent of two entries, which is what the fake engine is told it holds.
    fn description() -> TorrentDescription {
        TorrentDescription::new(vec![
            TorrentEntry::new("Movie.2024.1080p.mkv", 1_000, EntryKind::File),
            TorrentEntry::new("Movie.2024.1080p.srt", 1_000, EntryKind::File),
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
            release_title: "Movie.2024.1080p.BluRay.x264-GROUP".to_owned(),
            release_source: Some("BluRay".to_owned()),
            release_group: Some("GROUP".to_owned()),
            resolution: Some("1080p".to_owned()),
            video_codec: Some("h264".to_owned()),
            hdr_format: None,
            audio_summary: Some("DTS 5.1".to_owned()),
            reported_size_bytes: Some(2_000),
            info_hash: None,
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

    fn request(title_id: i64, byte: u8, description: TorrentDescription) -> StartRequest {
        StartRequest {
            account: account(),
            job: new_job(title_id),
            description,
            info_hash: hash(byte),
        }
    }

    /// The service under test, and the things a test inspects it through.
    struct Harness {
        service: AcquisitionService,
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
        Harness {
            service,
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
            description: TorrentDescription,
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
            started.job.save_path.as_str().ends_with("downloads/Movies"),
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

    /// F6's pre-write guarantee, read off the engine's call log rather than off the code: neither a
    /// hard block nor an opt-in refusal reaches `add`, and the report explains both after the fact.
    #[tokio::test]
    async fn a_blocked_or_opt_in_torrent_never_reaches_the_engine() {
        let harness = harness().await;

        let blocked = harness
            .start(
                2,
                TorrentDescription::new(vec![TorrentEntry::new(
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
                TorrentDescription::new(vec![TorrentEntry::new("extras.zip", 1, EntryKind::File)]),
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
            TorrentDescription::new(vec![TorrentEntry::new("extras.zip", 1, EntryKind::File)]);
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

    /// F6 draws inspection before the free-space check, and this is why: a torrent the gate has
    /// already refused says so even on a volume with no room. Answering "free some space" instead
    /// would send the user to do that for a download that still cannot start, and the report the
    /// gate already produced would be discarded unpersisted.
    #[tokio::test]
    async fn a_refused_torrent_says_so_even_when_the_volume_is_full() {
        let harness = harness().await;
        harness.volume.set(0);
        let archive =
            TorrentDescription::new(vec![TorrentEntry::new("extras.zip", 1, EntryKind::File)]);

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
            TorrentDescription::new(vec![TorrentEntry::new("extras.zip", 1, EntryKind::File)]);
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
        let traversal = TorrentDescription::new(vec![TorrentEntry::new(
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
        let stale = TorrentInspector::new(InspectionLimits::new(1_000, u64::MAX)).inspect(
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
    fn archive_for_stale() -> TorrentDescription {
        TorrentDescription::new(vec![TorrentEntry::new("extras.zip", 1, EntryKind::File)])
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
        let service = AcquisitionService::new(
            Arc::clone(&harness.database),
            Arc::clone(&engine) as Arc<dyn TorrentEngine>,
            Arc::clone(&harness.clock) as Arc<dyn Clock + Send + Sync>,
            guard,
            Arc::clone(&harness.volume) as Arc<dyn VolumeSpace>,
            FLOOR,
            bus(&harness.sink),
            Arc::clone(&trash) as Arc<dyn Trash>,
        );
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

    /// A job this process does not hold has no handle, which is a different answer from a state
    /// refusal: reattaching one is PG-E10-08's.
    #[tokio::test]
    async fn operations_on_a_job_this_process_does_not_hold_are_not_found() {
        let harness = harness().await;
        let error = harness.service.pause(404).await.expect_err("no such job");
        assert!(matches!(error, AcquisitionError::NotFound));

        let blocked = harness
            .start(
                17,
                TorrentDescription::new(vec![TorrentEntry::new("extras.zip", 1, EntryKind::File)]),
            )
            .await
            .expect("it parks");
        let error = harness
            .service
            .pause(blocked.job.id)
            .await
            .expect_err("a blocked job has no engine handle");
        assert!(matches!(error, AcquisitionError::NotFound));
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
