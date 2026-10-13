//! The Phase 0 runtime: the composition root that connects logging, storage, and the
//! scheduler, and the one place a job is started (PG-E0-19).
//!
//! Everything below this module is a library that can be built and tested on its own;
//! nothing above it — the IPC command surface and the desktop shell — is allowed to
//! assemble services itself. The runtime is therefore the only thing that knows the
//! database's path, which scheduler a job runs on, and where the logs are.
//!
//! Startup order is the design, not a detail: the log directory is opened by the
//! composition root first, because every later failure has to be reportable; the database
//! is opened second, so a migration failure stops the app before any job can be accepted;
//! and the event bus is last, so nothing is published into a bus whose sink is not ready.

use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use peregrine_domain::{Clock, IdGenerator, LibraryRoot, LibraryRootKind, ScanMode};
use peregrine_platform::{SystemClock, UlidGenerator};
use peregrine_storage::{
    Cursor, Database, JobChange, JobPhase, JobRecord, LibraryTitleRow, NewJob, PageRequest,
    StorageError,
};
use peregrine_sync::{
    JobHandle, JobSpec, Scheduler, SchedulerConfig, SchedulerError, SchedulerMetrics,
};
use serde_json::json;

use crate::jobs::library_scan::{self, LibraryScanJob};
use crate::library::{LibraryRootRow, LibraryService};
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};
use tokio::time::Instant;
use tracing::{info, warn};
use ulid::Ulid;

use crate::events::{EventBus, EventSink, EventStats};
use crate::jobs::hello::{self, HelloJob};
use crate::jobs::{CANCELLED_CODE, INTERNAL_CODE};
use crate::observability::{LogConfig, Observability};

/// The scope every user-facing read in this build runs under.
///
/// Accounts are `PG-E35`'s epic, and the migrations seed no account row: a fresh installation has
/// nothing to select between, so there is no account to resolve. What a scope *carries* is a
/// ceiling and a curation flag, and this build has neither — so the scope is unrestricted, and the
/// account id it names is inert for both predicates an unrestricted scope applies (`hidden_by_scope`
/// is zero, which turns the predicate into a no-op; `peregrine-storage`'s own tests have relied on
/// exactly that since the first scoped read).
///
/// **When the accounts epic lands, this becomes a resolution rather than a constant:** the runtime
/// reads the default account and builds the scope from its `rating_ceiling` and `curated_only`, and
/// every call site that passes this keeps passing a scope — which is why the constant lives here
/// with the reason rather than at each call site with none.
#[must_use]
pub fn installation_scope() -> peregrine_domain::AccountScope {
    peregrine_domain::AccountScope::unrestricted(
        peregrine_domain::AccountId::new(1).expect("account one is a positive id"),
    )
}

/// The database file inside the data directory.
const DATABASE_FILE: &str = "peregrine.sqlite";

/// The log directory inside the data directory.
const LOG_DIRECTORY: &str = "logs";

/// The directives the runtime logs with when the caller does not choose.
const DEFAULT_DIRECTIVES: &str = "info";

/// The most log lines one tail request may return.
pub const MAX_LOG_LINES: u32 = 500;

/// The most bytes of log file a tail request reads (the IPC payload budget, × 1 KiB).
const LOG_TAIL_BYTES: u64 = 64 * 1024;

/// The prefix and suffix `peregrine-app`'s rotated files are named with.
const LOG_FILE_PREFIX: &str = "peregrine.";
const LOG_FILE_SUFFIX: &str = ".jsonl";

/// How the runtime is wired before it is started.
///
/// The sink is handed in rather than constructed here: the Tauri emitter lives in
/// `peregrine-ipc`, which is above this crate, and a test wants a sink it can read.
pub struct RuntimeConfig {
    data_directory: PathBuf,
    sink: Arc<dyn EventSink>,
    directives: String,
    tray_enabled: bool,
}

impl RuntimeConfig {
    /// Builds a configuration for `data_directory`, delivering events to `sink`.
    #[must_use]
    pub fn new(data_directory: impl Into<PathBuf>, sink: Arc<dyn EventSink>) -> Self {
        Self {
            data_directory: data_directory.into(),
            sink,
            directives: DEFAULT_DIRECTIVES.to_owned(),
            tray_enabled: true,
        }
    }

    /// Replaces the log filter directives.
    #[must_use]
    pub fn with_directives(mut self, directives: impl Into<String>) -> Self {
        self.directives = directives.into();
        self
    }

    /// Turns the shell's tray icon on or off.
    ///
    /// The user's answer will come from the settings registry (PG-E20-01); until that
    /// exists the runtime is built with the default, and the off path is a value a test
    /// can set rather than a branch nobody has run (PG-E1-09's "hidden when disabled").
    #[must_use]
    pub fn with_tray_enabled(mut self, tray_enabled: bool) -> Self {
        self.tray_enabled = tray_enabled;
        self
    }

    /// The directory everything the app owns lives under.
    #[must_use]
    pub fn data_directory(&self) -> &Path {
        &self.data_directory
    }

    /// The `SQLite` file.
    #[must_use]
    pub fn database_path(&self) -> PathBuf {
        self.data_directory.join(DATABASE_FILE)
    }

    /// The directory the rotated JSON lines are written to.
    #[must_use]
    pub fn logs_directory(&self) -> PathBuf {
        self.data_directory.join(LOG_DIRECTORY)
    }

    /// The logging configuration for [`Observability::install`](crate::Observability).
    #[must_use]
    pub fn log_config(&self) -> LogConfig {
        LogConfig {
            directory: self.logs_directory(),
            directives: self.directives.clone(),
        }
    }
}

/// A job the runtime has accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartedJob {
    /// The scheduler's identity for the job; the key progress events carry.
    pub job: Ulid,
    /// The journal row this job reports into.
    pub journal_id: i64,
}

/// What the diagnostics surface reports about the running process.
#[derive(Debug, Clone)]
pub struct Health {
    /// How long startup took, from the first directory check to a ready runtime.
    pub startup: Duration,
    /// The scheduler's counters.
    pub scheduler: SchedulerMetrics,
    /// The event bus's counters.
    pub events: EventStats,
    /// Whether the boot-time integrity check had to rebuild the database, which means the
    /// library it held is gone and has to be rebuilt by scanning (ARCHITECTURE §15).
    pub rescan_required: bool,
    /// Which metadata providers hold a credential (`PG-E5-12`). A provider nobody has configured is
    /// not an error, and the interface shows an empty state rather than a failure for it.
    pub providers: crate::providers::ProviderStatus,
}

/// What the shell's tray shows about background work (PG-E1-09).
///
/// The shell asks the runtime for this rather than assembling it from webview events:
/// the job journal and the scheduler are the only owners of "how much work is in
/// flight" (ARCHITECTURE §6, "State ownership"), and a count computed in the UI would
/// be wrong the moment a job started while the window was closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellStatus {
    /// Jobs the scheduler still tracks: ready, deferred, or running.
    pub active_downloads: u32,
    /// Torrents seeding. Zero until a torrent engine is composed into the runtime
    /// (PG-E10): `peregrine-torrent` is documentation-only today, so there is no source
    /// for a different number and inventing one would be the lie the tray menu must not
    /// tell.
    pub seeding: u32,
    /// Whether the shell should show a tray icon at all.
    pub tray_enabled: bool,
}

/// The running application.
pub struct Runtime {
    config: RuntimeConfig,
    database: Arc<Database>,
    scheduler: Arc<Scheduler>,
    bus: Arc<EventBus>,
    clock: Arc<dyn Clock + Send + Sync>,
    live: Mutex<HashMap<Ulid, LiveJob>>,
    // Never read: held for its `Drop`, which flushes the queued log lines and stops the
    // writer thread. Losing it before shutdown would silently drop the lines that explain
    // the shutdown.
    _logging: Observability,
    /// Which providers hold a credential, refreshed off the command path (see `providers`).
    providers: Arc<crate::providers::ProviderRegistry>,
    startup: Duration,
}

/// A job the runtime can still cancel.
struct LiveJob {
    handle: JobHandle,
    journal_id: i64,
}

impl Runtime {
    /// Opens the data directory, the database, and the scheduler.
    ///
    /// The logging handle is passed in already installed, so that a caller which cannot
    /// install a process-wide subscriber — a test — can still start a runtime without
    /// either failing or writing into the developer's log directory.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::DataDirectory`] when the directory cannot be created,
    /// [`RuntimeError::Storage`] when the database cannot be opened or migrated. Both
    /// happen before any job can be accepted, so a failure here leaves nothing running.
    pub async fn start(
        config: RuntimeConfig,
        logging: Observability,
    ) -> Result<Self, RuntimeError> {
        let started = Instant::now();
        tokio::fs::create_dir_all(config.data_directory())
            .await
            .map_err(|source| RuntimeError::DataDirectory {
                path: config.data_directory().to_path_buf(),
                source,
            })?;

        // A recovery is logged where it happens, in `Database::open`; the runtime carries
        // the signal on to health.
        let database = Arc::new(Database::open(config.database_path()).await?);
        // The composition root is the one place the production adapters are chosen
        // (AGENTS R8); the scheduler is handed both ports rather than reaching for them.
        let clock: Arc<dyn Clock + Send + Sync> = Arc::new(SystemClock);
        let ids: Arc<dyn IdGenerator + Send + Sync> = Arc::new(UlidGenerator);
        let scheduler = Arc::new(Scheduler::new(SchedulerConfig::new(
            Arc::clone(&clock),
            ids,
        )));
        let bus = Arc::new(EventBus::new(Arc::clone(&config.sink)));

        let startup = started.elapsed();
        info!(
            startup_ms = startup.as_millis(),
            database = %config.database_path().display(),
            "the runtime is ready"
        );

        let providers = Arc::new(crate::providers::ProviderRegistry::new());
        // Off the start-up path and off the command path: a keyring read is blocking and can raise an OS
        // prompt, so it happens once, on a blocking thread, and everything else reads the cache.
        {
            let providers = Arc::clone(&providers);
            tokio::task::spawn_blocking(move || {
                if let Err(error) = providers.refresh(&peregrine_secrets::KeyringStore::new()) {
                    tracing::debug!(%error, "no credential store answered, so no provider is configured");
                }
            });
        }

        Ok(Self {
            config,
            database,
            scheduler,
            bus,
            clock,
            live: Mutex::new(HashMap::new()),
            _logging: logging,
            providers,
            startup,
        })
    }

    /// The configuration this runtime was built with.
    #[must_use]
    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }

    /// Starts the hello job (`PG-E0-19`) and returns its identities.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::InvalidRequest`] when `steps` is outside
    /// [`MIN_STEPS`](hello::MIN_STEPS)..=[`MAX_STEPS`](hello::MAX_STEPS), because a caller
    /// that asks for an unbounded amount of work is a caller bug and the message says
    /// which bound it broke.
    pub async fn start_hello(&self, steps: u32) -> Result<StartedJob, RuntimeError> {
        if !(hello::MIN_STEPS..=hello::MAX_STEPS).contains(&steps) {
            return Err(RuntimeError::InvalidRequest {
                detail: format!(
                    "steps must be between {} and {}",
                    hello::MIN_STEPS,
                    hello::MAX_STEPS
                ),
            });
        }

        let now = self.clock.now().as_millisecond();
        let journal_id = self
            .database
            .insert_job(NewJob {
                kind: hello::KIND.to_owned(),
                priority: hello::PRIORITY,
                dedupe_key: None,
                payload_json: json!({ "steps": steps }).to_string(),
                max_attempts: 1,
                created_at: now,
            })
            .await?;

        let spec = JobSpec {
            kind: hello::KIND.to_owned(),
            priority: hello::PRIORITY,
            dedupe_key: None,
            max_attempts: 1,
        };
        let job = HelloJob::new(
            Arc::clone(&self.database),
            Arc::clone(&self.bus),
            Arc::clone(&self.clock),
            journal_id,
            steps,
        );

        let handle = match self.scheduler.submit(spec, job).await {
            Ok(handle) => handle,
            Err(error) => {
                // The row exists and will never run, so leaving it `queued` would show a
                // job waiting forever in the UI. Closing it here is the same rule the
                // scheduler applies to a failed attempt: a job that cannot run reaches a
                // terminal state.
                let _ = self
                    .database
                    .update_job(
                        journal_id,
                        JobChange::Finished {
                            at: now,
                            phase: JobPhase::Failed,
                            error_code: Some(INTERNAL_CODE.to_owned()),
                            error: Some(error.to_string()),
                        },
                    )
                    .await;
                return Err(RuntimeError::Scheduler(error));
            }
        };

        let started = StartedJob {
            job: handle.id(),
            journal_id,
        };
        info!(
            job = %started.job,
            journal_id,
            steps,
            "accepted the hello job"
        );
        self.remember(handle, journal_id);
        Ok(started)
    }

    /// Asks the job to stop.
    ///
    /// The request is journaled before the token trips, so a job that is never observed
    /// again still shows that it was asked to stop.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::UnknownJob`] when no job with that id is live — a job that already
    /// finished, or an id from another process.
    pub async fn cancel(&self, job: Ulid) -> Result<(), RuntimeError> {
        let journal_id = {
            let live = self.live();
            live.get(&job)
                .map(|entry| entry.journal_id)
                .ok_or(RuntimeError::UnknownJob { job })?
        };

        // The row is the authority on whether a job is still a job. A job writes its own terminal
        // row and the scheduler resolves its handle afterwards, so there is a window in which the
        // journal says `canceled` and the handle still looks live — and asking to cancel in that
        // window used to answer "ok" about a job that had already stopped, which is the one answer
        // a caller cannot tell apart from a real cancellation. Reading the phase closes the window
        // at the point where the two sources of truth meet. A read that fails leaves the handle as
        // the only answer rather than failing the request: refusing to cancel because a journal
        // read failed would be strictly worse.
        if let Ok(Some(row)) = self.database.job(journal_id).await
            && row.state.is_terminal()
        {
            self.live().remove(&job);
            return Err(RuntimeError::UnknownJob { job });
        }

        if let Err(error) = self
            .database
            .update_job(journal_id, JobChange::CancelRequested)
            .await
        {
            // The cancellation itself must not depend on the audit write succeeding.
            warn!(job = %job, %error, "the cancellation request could not be journaled");
        }

        let mut live = self.live();
        if let Some(entry) = live.get_mut(&job) {
            entry.handle.cancel();
        }
        info!(job = %job, "cancellation requested");
        Ok(())
    }

    /// A page of the rules a user's corrections became, newest first.
    ///
    /// Read under [`installation_scope`], like every user-facing read: this is a list a person
    /// reviews and edits.
    pub async fn match_rules(
        &self,
        limit: u32,
    ) -> Result<Vec<peregrine_storage::MatchRuleRow>, RuntimeError> {
        Ok(self
            .database
            .list_match_rules(installation_scope(), PageRequest::first(limit))
            .await?)
    }

    /// Removes one rule, reporting whether it was there.
    ///
    /// What makes a wrong answer recoverable: a rule places every later file that reads like the one
    /// it was made from, so withdrawing it is the only way back for a user who answered wrongly. The
    /// files it already placed stay, because removing a file is a different action with its own
    /// confirmation (`PG-E15`).
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Library`] when the database refuses, which is the only failure this has.
    pub async fn forget_match_rule(&self, id: i64) -> Result<bool, RuntimeError> {
        Ok(self.database.remove_match_rule(id).await?)
    }

    /// A page of the files a scan could not place, newest first.
    ///
    /// Read under [`installation_scope`], like every user-facing read: this is a list a person
    /// answers, so a file belonging to a title an account may not see is not offered to it.
    pub async fn unmatched_files(
        &self,
        limit: u32,
    ) -> Result<Vec<peregrine_storage::UnmatchedFileRow>, RuntimeError> {
        Ok(self
            .database
            .list_unmatched_files(installation_scope(), PageRequest::first(limit))
            .await?)
    }

    /// Answers one parked file by placing it as the title its parse read.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Library`] when the database refuses, which is the only failure this has.
    /// Answers one parked file: under the title its parse read, or under the title the scan offered
    /// when the caller asks for that one.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Library`] when the database refuses, which is the only failure this has.
    pub async fn match_unmatched_file(
        &self,
        id: i64,
        as_title: crate::library::MatchAs,
    ) -> Result<bool, RuntimeError> {
        let matched = crate::library::match_unmatched(
            &self.database,
            installation_scope(),
            id,
            as_title,
            self.clock.now(),
        )
        .await?;
        Ok(matches!(matched, crate::library::Matched::Placed))
    }

    /// A page of titles `scope` may see that match the user's own text.
    ///
    /// The picker's read. The query reaches `FTS5` through the storage layer, which is the only place
    /// it is turned into a `MATCH` expression and the only place hostile input is answered rather than
    /// refused: a query the grammar would refuse comes back as an empty page.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Library`] when the database refuses, which is the only failure this has.
    pub async fn search_titles(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<peregrine_storage::TitleIdentity>, RuntimeError> {
        Ok(self
            .database
            .search(installation_scope(), query, PageRequest::first(limit))
            .await?)
    }

    /// One title, with the counts a screen shows, when the scope may see it.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Library`] when the database refuses, which is the only failure this has.
    pub async fn library_title(
        &self,
        id: i64,
    ) -> Result<Option<peregrine_storage::LibraryTitleRow>, RuntimeError> {
        Ok(self
            .database
            .library_title(installation_scope(), id)
            .await?)
    }

    /// A title's episodes, in the order a person watches them.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Library`] when the database refuses, which is the only failure this has.
    pub async fn title_episodes(
        &self,
        title_id: i64,
        limit: u32,
    ) -> Result<Vec<peregrine_storage::TitleEpisodeRow>, RuntimeError> {
        Ok(self
            .database
            .list_title_episodes(installation_scope(), title_id, limit)
            .await?)
    }

    /// A page of the titles the library holds, newest first.
    ///
    /// Read under [`installation_scope`], which is the whole of this build's account story: the
    /// screen asks what *it* has, and on an installation with one account that is everything.
    pub async fn library_titles(
        &self,
        after: Option<i64>,
        limit: u32,
    ) -> Result<Vec<LibraryTitleRow>, RuntimeError> {
        let request = match after {
            Some(cursor) => PageRequest::after(Cursor::new(cursor), limit),
            None => PageRequest::first(limit),
        };
        Ok(self
            .database
            .list_library_titles(installation_scope(), request)
            .await?)
    }

    /// The library roots, newest first.
    ///
    /// A root is a directory the user configured, so there is no `AccountScope` here and none is
    /// accepted: what is scoped is the content inside the roots, not the list of them.
    pub async fn library_roots(
        &self,
        after: Option<i64>,
        limit: u32,
    ) -> Result<Vec<LibraryRootRow>, RuntimeError> {
        let request = match after {
            Some(cursor) => PageRequest::after(Cursor::new(cursor), limit),
            None => PageRequest::first(limit),
        };
        Ok(self.library().roots(request).await?)
    }

    /// Adds a directory as a library root.
    ///
    /// The path arrives from the shell's own folder picker, which hands it over as text it has not
    /// validated (`UntrustedPath`, PG-E1-12); `PathGuard` in the service is what decides whether it
    /// can be a root, and this is where that refusal becomes a structured error the UI can show.
    pub async fn add_library_root(
        &self,
        path: &str,
        kind: LibraryRootKind,
        scan_mode: ScanMode,
    ) -> Result<LibraryRoot, RuntimeError> {
        let now = self.clock.now();
        Ok(self
            .library()
            .add_root(Path::new(path), kind, scan_mode, now)
            .await?)
    }

    /// Removes a library root, reporting whether it was there.
    ///
    /// Nothing else is removed with it: the files, the items, and the watch progress all stay
    /// (PG-E14-01), which is why this returns a `bool` rather than a plan the caller has to
    /// confirm. There is no destructive step here to confirm.
    pub async fn remove_library_root(&self, id: i64) -> Result<bool, RuntimeError> {
        Ok(self.library().remove_root(id).await?)
    }

    /// The library service over this runtime's database.
    ///
    /// Built on demand rather than held: it is two fields of an `Arc`, and a service that outlives
    /// its runtime is the kind of thing that keeps a database handle alive after shutdown.
    fn library(&self) -> LibraryService {
        LibraryService::new(Arc::clone(&self.database))
    }

    /// Starts a scan of every library root, or says that one is already running.
    ///
    /// # Why this is not deduped by the scheduler
    ///
    /// `PG-E6-01`'s scheduler can coalesce a submission onto a live job that holds the same
    /// `dedupe_key`, and that is the obvious way to say "only one scan at a time". It is not what
    /// this does, because a coalesced handle's state never reaches a terminal one — the coalesce
    /// path resolves the caller's completion but sends no state change — so a runtime that had
    /// inserted a journal row for it would hold a row that stays `queued` forever and a live entry
    /// that is pruned only when the shutdown sweep runs. The pre-check below is the honest version
    /// of the same intention: a second request while a scan is live is refused by name.
    ///
    /// Two scans that did start together would not corrupt anything: each reconciles only on a
    /// complete walk of a root, so their conclusions agree and the cost is duplicated IO. The check
    /// is here to avoid paying it, not to keep the library consistent.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::ScanAlreadyRunning`] when a scan is live, and [`RuntimeError::Storage`] or
    /// [`RuntimeError::Scheduler`] when the job cannot be journaled or accepted.
    pub async fn start_library_scan(&self) -> Result<StartedJob, RuntimeError> {
        if let Some(running) = self.live_scan() {
            return Err(RuntimeError::ScanAlreadyRunning { job: running });
        }

        let now = self.clock.now().as_millisecond();
        let journal_id = self
            .database
            .insert_job(NewJob {
                kind: library_scan::KIND.to_owned(),
                priority: library_scan::PRIORITY,
                // No dedupe key, for the reason in this method's own doc.
                dedupe_key: None,
                payload_json: json!({ "roots": true }).to_string(),
                max_attempts: 1,
                created_at: now,
            })
            .await?;

        let spec = JobSpec {
            kind: library_scan::KIND.to_owned(),
            priority: library_scan::PRIORITY,
            dedupe_key: None,
            max_attempts: 1,
        };
        let job = LibraryScanJob::new(
            Arc::clone(&self.database),
            Arc::clone(&self.bus),
            Arc::clone(&self.clock),
            journal_id,
        );

        let handle = match self.scheduler.submit(spec, job).await {
            Ok(handle) => handle,
            Err(error) => {
                // The row exists and will never run, so leaving it `queued` would show a scan
                // waiting forever in the UI. Closed here for the same reason the hello job closes
                // its own row on a refused submission.
                let _ = self
                    .database
                    .update_job(
                        journal_id,
                        JobChange::Finished {
                            at: now,
                            phase: JobPhase::Failed,
                            error_code: Some(INTERNAL_CODE.to_owned()),
                            error: Some(error.to_string()),
                        },
                    )
                    .await;
                return Err(RuntimeError::Scheduler(error));
            }
        };

        let started = StartedJob {
            job: handle.id(),
            journal_id,
        };
        info!(job = %started.job, journal_id, "accepted the library scan");
        self.remember(handle, journal_id);
        Ok(started)
    }

    /// The live scan job's id, when one is running.
    fn live_scan(&self) -> Option<Ulid> {
        self.live()
            .values()
            .find(|entry| entry.handle.state().kind() == library_scan::KIND)
            .map(|entry| entry.handle.id())
    }

    /// One page of journal rows, newest first.
    ///
    /// `after` is the id of the previous page's last row; `None` reads the newest page. The
    /// cursor is a bare `i64` here rather than in the command surface because the journal
    /// screen shows one page, and `specta` cannot export a large integer as a command
    /// argument without a DTO to carry it — the repository's capability is what matters.
    pub async fn jobs(
        &self,
        after: Option<i64>,
        limit: u32,
    ) -> Result<Vec<JobRecord>, RuntimeError> {
        let request = match after {
            Some(cursor) => PageRequest::after(Cursor::new(cursor), limit),
            None => PageRequest::first(limit),
        };
        Ok(self.database.list_jobs(request).await?)
    }

    /// The counters a diagnostics screen shows.
    ///
    /// `rescan_required` is read from the database handle the runtime actually opened, so it
    /// cannot disagree with the file in use (ARCHITECTURE §15).
    pub async fn health(&self) -> Health {
        Health {
            startup: self.startup,
            scheduler: self.scheduler.metrics().await,
            events: self.bus.stats(),
            rescan_required: self.database.recovery().requires_rescan(),
            // From the cache the refresh task fills, never from the keyring: reading one is blocking
            // and can raise an OS prompt, which is what the command budget test exists to catch.
            providers: self.providers.status(),
        }
    }

    /// The state the tray's menu presents (PG-E1-09).
    pub async fn shell_status(&self) -> ShellStatus {
        let scheduler = self.scheduler.metrics().await;
        ShellStatus {
            active_downloads: u32::try_from(scheduler.live).unwrap_or(u32::MAX),
            seeding: 0,
            tray_enabled: self.config.tray_enabled,
        }
    }

    /// The last `lines` lines of the newest log file.
    ///
    /// Reading is bounded twice — a 64 KiB window from the end of the file and at most
    /// [`MAX_LOG_LINES`] lines — because this answer travels over IPC, where a payload
    /// over 64 KB is a design error (ARCHITECTURE §7 rule 1). When the window starts
    /// mid-line the partial first line is dropped rather than returned as a truncated
    /// JSON object.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Logs`] when the directory or the newest file cannot be read. No
    /// log file at all is not an error: a process that has not logged yet returns an
    /// empty tail.
    pub async fn log_tail(&self, lines: u32) -> Result<Vec<String>, RuntimeError> {
        let directory = self.config.logs_directory();
        let Some(path) = newest_log_file(&directory).await else {
            return Ok(Vec::new());
        };

        let mut file = tokio::fs::File::open(&path)
            .await
            .map_err(|source| RuntimeError::Logs {
                path: path.clone(),
                source,
            })?;
        let length = file
            .metadata()
            .await
            .map_err(|source| RuntimeError::Logs {
                path: path.clone(),
                source,
            })?
            .len();
        let window_start = length.saturating_sub(LOG_TAIL_BYTES);
        file.seek(SeekFrom::Start(window_start))
            .await
            .map_err(|source| RuntimeError::Logs {
                path: path.clone(),
                source,
            })?;

        let mut buffer = Vec::new();
        file.take(LOG_TAIL_BYTES)
            .read_to_end(&mut buffer)
            .await
            .map_err(|source| RuntimeError::Logs {
                path: path.clone(),
                source,
            })?;

        let text = String::from_utf8_lossy(&buffer);
        let mut tail: Vec<String> = text.lines().map(str::to_owned).collect();
        if window_start > 0 && !tail.is_empty() {
            tail.remove(0);
        }
        let wanted = lines.clamp(1, MAX_LOG_LINES) as usize;
        if tail.len() > wanted {
            tail.drain(..tail.len() - wanted);
        }
        Ok(tail)
    }

    /// Stops the scheduler and the bus, and closes the journal rows of live jobs.
    ///
    /// The scheduler deliberately does not wait for a job that ignores its token, so the
    /// runtime closes their rows itself: a process that exits with a row still `running`
    /// would show a job that can never finish. The job's own report is then refused by
    /// the same terminal-state guard that refuses a late progress report.
    ///
    /// Idempotent.
    pub async fn shutdown(&self) {
        info!("the runtime is stopping");
        self.scheduler.shutdown().await;
        self.bus.shutdown().await;

        let live: Vec<(Ulid, i64)> = {
            let mut guard = self.live();
            guard
                .drain()
                .map(|(job, entry)| (job, entry.journal_id))
                .collect()
        };
        let now = self.clock.now().as_millisecond();
        for (job, journal_id) in &live {
            match self
                .database
                .update_job(
                    *journal_id,
                    JobChange::Finished {
                        at: now,
                        phase: JobPhase::Canceled,
                        error_code: Some(CANCELLED_CODE.to_owned()),
                        error: None,
                    },
                )
                .await
            {
                Ok(true) => info!(job = %job, journal_id, "closed a live job's journal row"),
                Ok(false) => {}
                Err(error) => {
                    warn!(job = %job, journal_id, %error, "a live job's row could not be closed");
                }
            }
        }
        info!(closed = live.len(), "the runtime stopped");
    }

    /// The live jobs, with terminal entries pruned.
    ///
    /// Returns the guard rather than a copy because a caller needs `&mut` to cancel; the
    /// lock is held only for synchronous work, never across an `.await`.
    fn live(&self) -> MutexGuard<'_, HashMap<Ulid, LiveJob>> {
        let mut live = match self.live.lock() {
            Ok(live) => live,
            Err(poisoned) => poisoned.into_inner(),
        };
        // A job handle that reached a terminal state is dead weight: dropping it does not
        // cancel anything (ARCHITECTURE §7 rule 6), and keeping it would grow the map for
        // the life of the process.
        live.retain(|_, entry| !entry.handle.state().is_terminal());
        live
    }

    /// Tracks an accepted job so it can be cancelled later.
    fn remember(&self, handle: JobHandle, journal_id: i64) {
        let mut live = self.live();
        live.insert(handle.id(), LiveJob { handle, journal_id });
    }
}

/// The newest rotated log file, or `None` when none exists.
///
/// The file name carries a `YYYY-MM-DD` date, so the greatest name is the newest file and
/// no metadata read is needed to choose it.
async fn newest_log_file(directory: &Path) -> Option<PathBuf> {
    // A missing directory means nothing has been logged yet, which is not a failure to
    // report to a user who is only asking what the log says.
    let Ok(mut entries) = tokio::fs::read_dir(directory).await else {
        return None;
    };

    let mut newest: Option<PathBuf> = None;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(LOG_FILE_PREFIX) || !name.ends_with(LOG_FILE_SUFFIX) {
            continue;
        }
        let candidate = entry.path();
        if newest.as_ref().is_none_or(|current| {
            current
                .file_name()
                .and_then(|name| name.to_str())
                .is_none_or(|current| name > current)
        }) {
            newest = Some(candidate);
        }
    }
    newest
}

/// Why the runtime could not start, or could not do what it was asked.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// The data directory could not be created.
    #[error("cannot create the data directory {}: {source}", path.display())]
    DataDirectory {
        /// The directory that failed.
        path: PathBuf,
        /// The filesystem's error.
        source: std::io::Error,
    },

    /// The database could not be opened, migrated, or written.
    #[error("the database could not be used: {0}")]
    Storage(#[from] StorageError),

    /// The scheduler is not accepting work.
    #[error("the scheduler could not accept the job: {0}")]
    Scheduler(#[from] SchedulerError),

    /// The caller asked for something outside the documented range.
    #[error("{detail}")]
    InvalidRequest {
        /// What the caller got wrong, phrased for a user.
        detail: String,
    },

    /// A scan was asked for while one was already running.
    #[error("a library scan is already running")]
    ScanAlreadyRunning {
        /// The job that is already scanning, so a caller can watch that one instead.
        job: Ulid,
    },

    /// Indexing refused the operation, with the reason a caller can map.
    #[error(transparent)]
    Index(#[from] crate::library::IndexError),

    /// The library refused the operation, with the reason a user can read.
    #[error(transparent)]
    Library(#[from] crate::library::LibraryError),

    /// No live job has that id.
    #[error("no job {job} is running; it may have finished already")]
    UnknownJob {
        /// The id the caller asked about.
        job: Ulid,
    },

    /// The log files could not be read.
    #[error("cannot read the logs at {}: {source}", path.display())]
    Logs {
        /// The file or directory that failed.
        path: PathBuf,
        /// The filesystem's error.
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::RecordingSink;
    use std::fmt::Write as _;
    use tempfile::TempDir;

    /// Starts a runtime against a temporary directory, with a recording sink.
    ///
    /// The subscriber is built but never installed process-wide: `set_global_default` can
    /// only be called once per process, so a test that installed it would make every other
    /// test in this binary order-dependent. Dropping it here is deliberate — the runtime
    /// holds the handle it needs, and nothing in these tests changes the log level.
    async fn runtime(directory: &TempDir) -> (Runtime, Arc<RecordingSink>) {
        let sink = Arc::new(RecordingSink::default());
        let config = RuntimeConfig::new(directory.path(), Arc::clone(&sink) as Arc<dyn EventSink>);
        let (subscriber, logging) = Observability::build(&config.log_config()).unwrap();
        drop(subscriber);
        let runtime = Runtime::start(config, logging).await.unwrap();
        (runtime, sink)
    }

    /// Waits for the journal row to reach a terminal phase.
    async fn await_terminal(runtime: &Runtime, journal_id: i64) -> JobRecord {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let row = runtime.jobs(None, 20).await.unwrap();
                let row = row
                    .into_iter()
                    .find(|row| row.id == journal_id)
                    .expect("the row exists");
                if row.state.is_terminal() {
                    return row;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the job must reach a terminal state")
    }

    /// The acceptance for PG-E0-19 in one test: the runtime opens the database, accepts a
    /// job, the job's state is readable from the journal, progress arrived over the bus,
    /// and shutdown leaves nothing running.
    #[tokio::test]
    async fn a_hello_job_runs_end_to_end_and_is_journaled() {
        let directory = TempDir::new().unwrap();
        let (runtime, sink) = runtime(&directory).await;

        let started = runtime.start_hello(4).await.unwrap();
        assert!(runtime.config().database_path().exists());

        let row = await_terminal(&runtime, started.journal_id).await;
        assert_eq!(row.state, JobPhase::Succeeded);
        assert_eq!(row.kind, hello::KIND);
        assert_eq!(row.progress_permille, 1000);
        assert_eq!(row.attempts, 1);
        assert!(
            row.started_at.is_some() && row.finished_at.is_some(),
            "both timestamps are recorded: {row:?}"
        );

        let health = runtime.health().await;
        assert!(health.startup < Duration::from_secs(30));
        assert_eq!(
            health.scheduler.live, 0,
            "a finished job must not stay in the scheduler's bookkeeping"
        );
        assert_eq!(health.events.dropped, 0, "one job cannot exhaust the bus");
        assert!(!health.events.is_degraded());

        runtime.shutdown().await;
        let delivered = sink.delivered();
        assert!(
            delivered
                .iter()
                .any(|event| event.topic.name() == "job://progress"),
            "progress must reach the sink: {delivered:#?}"
        );
        assert!(
            delivered
                .iter()
                .any(|event| event.payload["phase"] == "succeeded"),
            "the terminal phase must reach the sink: {delivered:#?}"
        );

        // Idempotent: a second shutdown must not hang or double-close anything.
        runtime.shutdown().await;
    }

    /// A row that has reached a terminal phase means the job is gone, whatever the handle says.
    ///
    /// A job writes its own terminal row and the scheduler resolves the handle afterwards, so
    /// there is a window in which the journal is honest and the handle is not yet: a cancel
    /// arriving there used to answer `ok` about a job that had already stopped, which is the one
    /// answer a caller cannot tell apart from a real cancellation. This test stages that window by
    /// writing the row by hand, where racing a real job staged it about one run in ten.
    #[tokio::test]
    async fn cancelling_a_job_whose_row_is_already_terminal_reports_it_unknown() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;
        let started = runtime.start_hello(hello::MAX_STEPS).await.unwrap();

        // What the job itself writes as its last act, before the scheduler sees its outcome.
        let closed = runtime
            .database
            .update_job(
                started.journal_id,
                JobChange::Finished {
                    at: runtime.clock.now().as_millisecond(),
                    phase: JobPhase::Canceled,
                    error_code: Some(CANCELLED_CODE.to_owned()),
                    error: None,
                },
            )
            .await
            .unwrap();
        assert!(closed, "the row was open");

        let error = runtime
            .cancel(started.job)
            .await
            .expect_err("a job whose row is terminal is not live");
        assert!(
            matches!(error, RuntimeError::UnknownJob { .. }),
            "{error:?}"
        );

        runtime.shutdown().await;
    }

    /// A scan of an empty library runs, journals, and reaches a terminal state.
    ///
    /// The walk's own behaviour is the job's to test; what this asserts is the runtime's half —
    /// a row exists before the job runs, the job is remembered, and the row closes.
    #[tokio::test]
    async fn a_scan_can_be_started_and_its_row_closes() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;

        let started = runtime.start_library_scan().await.unwrap();
        let row = await_terminal(&runtime, started.journal_id).await;

        assert_eq!(row.kind, library_scan::KIND);
        assert_eq!(row.state, JobPhase::Succeeded, "{row:?}");
        assert!(
            !runtime.live().contains_key(&started.job),
            "and it is no longer live"
        );

        runtime.shutdown().await;
    }

    /// A second scan while one is live is refused by name rather than queued or dropped.
    ///
    /// The slot is held the way the runtime holds it — a scheduler job of the scan's kind,
    /// remembered as live — because a scan of a temporary library finishes in microseconds and a
    /// test that raced two real ones would be a test of the machine's speed. What is under test is
    /// the guard, and the guard reads the live set.
    #[tokio::test]
    async fn a_second_scan_while_one_is_live_is_refused_by_name() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;

        let journal_id = runtime
            .database
            .insert_job(NewJob {
                kind: library_scan::KIND.to_owned(),
                priority: library_scan::PRIORITY,
                dedupe_key: None,
                payload_json: "{}".to_owned(),
                max_attempts: 1,
                created_at: runtime.clock.now().as_millisecond(),
            })
            .await
            .unwrap();
        let held = runtime
            .scheduler
            .submit(
                JobSpec {
                    kind: library_scan::KIND.to_owned(),
                    priority: library_scan::PRIORITY,
                    dedupe_key: None,
                    max_attempts: 1,
                },
                HelloJob::new(
                    Arc::clone(&runtime.database),
                    Arc::clone(&runtime.bus),
                    Arc::clone(&runtime.clock),
                    journal_id,
                    hello::MAX_STEPS,
                ),
            )
            .await
            .unwrap();
        let held_id = held.id();
        runtime.remember(held, journal_id);

        let refused = runtime
            .start_library_scan()
            .await
            .expect_err("one scan at a time");

        match refused {
            RuntimeError::ScanAlreadyRunning { job } => assert_eq!(job, held_id),
            other => panic!("expected a refusal naming the running scan, got {other:?}"),
        }

        runtime.shutdown().await;
    }

    /// A cancellation is honored and journaled, and the row ends terminal rather than
    /// running forever.
    #[tokio::test]
    async fn cancelling_a_hello_job_stops_it_and_closes_its_row() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;

        let started = runtime.start_hello(hello::MAX_STEPS).await.unwrap();
        runtime.cancel(started.job).await.unwrap();

        let row = await_terminal(&runtime, started.journal_id).await;
        assert_eq!(row.state, JobPhase::Canceled);
        assert_eq!(row.last_error_code.as_deref(), Some(CANCELLED_CODE));
        assert!(row.cancel_requested, "the request is part of the audit");

        // The handle is gone once the job is terminal, so asking again is honest rather
        // than a silent no-op.
        let error = runtime
            .cancel(started.job)
            .await
            .expect_err("a finished job cannot be cancelled again");
        assert!(
            matches!(error, RuntimeError::UnknownJob { .. }),
            "{error:?}"
        );

        runtime.shutdown().await;
    }

    /// Shutdown closes the row of a job that is still running, because the scheduler does
    /// not wait for a job that is mid-step.
    #[tokio::test]
    async fn shutdown_closes_a_live_jobs_row() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;

        let started = runtime.start_hello(hello::MAX_STEPS).await.unwrap();
        // One step is enough for the job to have written its `running` row.
        tokio::time::sleep(Duration::from_millis(25)).await;
        runtime.shutdown().await;

        let rows = runtime.jobs(None, 10).await.unwrap();
        let row = rows
            .into_iter()
            .find(|row| row.id == started.journal_id)
            .expect("the row exists");
        assert_eq!(
            row.state,
            JobPhase::Canceled,
            "a stopping runtime must not leave a job running: {row:?}"
        );
    }

    /// The boundary: a step count outside the documented range is refused with the range
    /// in the message, and nothing is journaled.
    #[tokio::test]
    async fn start_hello_refuses_a_step_count_outside_the_bounds() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;

        for steps in [0, hello::MAX_STEPS + 1, u32::MAX] {
            let error = runtime
                .start_hello(steps)
                .await
                .expect_err("the range must be enforced");
            let RuntimeError::InvalidRequest { detail } = &error else {
                panic!("expected an invalid-request failure, got {error:?}");
            };
            assert!(
                detail.contains(&hello::MIN_STEPS.to_string())
                    && detail.contains(&hello::MAX_STEPS.to_string()),
                "the message must name the range: {detail}"
            );
        }
        assert!(
            runtime.jobs(None, 10).await.unwrap().is_empty(),
            "a refused request must not leave a journal row behind"
        );

        // The bounds themselves are accepted.
        let lowest = runtime.start_hello(hello::MIN_STEPS).await.unwrap();
        let highest = runtime.start_hello(hello::MAX_STEPS).await.unwrap();
        assert_ne!(lowest.journal_id, highest.journal_id);
        runtime.cancel(highest.job).await.unwrap();
        runtime.shutdown().await;
    }

    /// The log tail reads the newest file, stays inside the line limit, and does not
    /// return the partial line a byte window starts in the middle of.
    #[tokio::test]
    async fn log_tail_reads_the_newest_file_within_its_window() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;
        let logs = runtime.config().logs_directory();
        tokio::fs::create_dir_all(&logs).await.unwrap();

        // The fixture's dates are far outside any real one, because the choice is made by comparing
        // names as text: a date written in the past is outranked by whatever the running process has
        // logged today, and this test would then read the live file instead of the fixture. It did
        // exactly that on 2026-10-13, which is why these are the extremes of the format's range.
        tokio::fs::write(
            logs.join("peregrine.0001-01-01.jsonl"),
            "{\"message\":\"from-yesterday\"}\n",
        )
        .await
        .unwrap();

        // A newest file larger than the window, so the read starts mid-line.
        let mut newest = String::from("{\"message\":\"partial-line-that-must-be-dropped\"}\n");
        for index in 0..4_000 {
            let _ = writeln!(newest, "{{\"message\":\"line-{index}\"}}");
        }
        assert!(
            newest.len() as u64 > LOG_TAIL_BYTES,
            "the fixture must be larger than the window"
        );
        tokio::fs::write(logs.join("peregrine.9999-12-31.jsonl"), &newest)
            .await
            .unwrap();

        let tail = runtime.log_tail(10).await.unwrap();
        assert_eq!(tail.len(), 10);
        assert_eq!(tail[9], "{\"message\":\"line-3999\"}");
        assert_eq!(tail[0], "{\"message\":\"line-3990\"}");
        assert!(
            !tail.iter().any(|line| line.contains("partial")),
            "a window that starts mid-line must drop the partial record: {tail:?}"
        );
        assert!(
            !tail.iter().any(|line| line.contains("yesterday")),
            "the newest file is the one to read"
        );

        // Asking for more than the cap returns at most the cap.
        assert_eq!(
            runtime.log_tail(u32::MAX).await.unwrap().len(),
            usize::try_from(MAX_LOG_LINES).unwrap()
        );

        runtime.shutdown().await;
    }

    /// A process that has logged nothing yet has an empty tail, not an error.
    #[tokio::test]
    async fn log_tail_of_a_process_that_has_not_logged_is_empty() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;

        assert!(
            runtime.log_tail(10).await.unwrap().is_empty(),
            "no line has been written by this runtime"
        );
        runtime.shutdown().await;
    }

    /// A data directory that cannot exist fails startup with the path in the message.
    #[tokio::test]
    async fn start_reports_a_data_directory_that_cannot_be_created() {
        let directory = TempDir::new().unwrap();
        let blocked = directory.path().join("a-file");
        std::fs::write(&blocked, b"not a directory").unwrap();

        let sink = Arc::new(RecordingSink::default());
        let config = RuntimeConfig::new(&blocked, sink as Arc<dyn EventSink>);
        // The log directory has to be somewhere writable, so it is built from a second
        // configuration; startup must fail on the data directory regardless.
        let logs = RuntimeConfig::new(
            directory.path().join("logs-home"),
            Arc::new(RecordingSink::default()) as Arc<dyn EventSink>,
        );
        let (_subscriber, logging) = Observability::build(&logs.log_config()).unwrap();

        let error = Runtime::start(config, logging)
            .await
            .err()
            .expect("a file cannot be a data directory");
        assert!(
            matches!(error, RuntimeError::DataDirectory { .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains("a-file"), "{error}");
    }

    /// An empty journal lists nothing, and health reports a scheduler with no work.
    #[tokio::test]
    async fn a_fresh_runtime_has_no_jobs() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;

        assert!(
            runtime.jobs(None, 10).await.unwrap().is_empty(),
            "a fresh runtime has journaled nothing"
        );
        let health = runtime.health().await;
        assert_eq!(health.scheduler, SchedulerMetrics::default());
        assert_eq!(health.events, EventStats::default());
        assert!(
            !health.rescan_required,
            "a first install has no library to rebuild"
        );
        runtime.shutdown().await;
    }

    /// PG-E3-13 and PG-E3-14 on the real boot path: a damaged database is detected, kept
    /// aside, rebuilt, and the rescan it requires reaches the health surface the commands
    /// read — after which the runtime is a working one.
    #[tokio::test]
    async fn a_damaged_database_is_rebuilt_at_boot_and_reported_through_health() {
        let directory = TempDir::new().unwrap();
        tokio::fs::write(directory.path().join(DATABASE_FILE), b"not a database")
            .await
            .unwrap();

        let (runtime, _sink) = runtime(&directory).await;

        assert!(
            runtime.health().await.rescan_required,
            "the boot must signal that the library has to be rescanned"
        );
        let quarantine = directory.path().join(format!("{DATABASE_FILE}.corrupt-1"));
        assert!(
            quarantine.is_file(),
            "the damaged file must be kept, not deleted"
        );

        let started = runtime.start_hello(1).await.unwrap();
        let row = await_terminal(&runtime, started.journal_id).await;
        assert_eq!(
            row.state,
            JobPhase::Succeeded,
            "the rebuilt database must be a working one: {row:?}"
        );
        runtime.shutdown().await;
    }

    /// The tray is on by default and the off state is a value, not a branch nobody has
    /// run: PG-E1-09's "hidden when disabled" needs something to be disabled by.
    #[test]
    fn the_tray_is_enabled_unless_the_configuration_says_otherwise() {
        let directory = TempDir::new().unwrap();
        let sink = Arc::new(RecordingSink::default());
        let enabled = RuntimeConfig::new(directory.path(), Arc::clone(&sink) as Arc<dyn EventSink>);
        assert!(enabled.tray_enabled, "a tray nobody asked to hide is shown");
        assert!(!enabled.with_tray_enabled(false).tray_enabled);
    }

    /// The counts the tray presents are the runtime's, read from the scheduler that owns
    /// them; the shell never assembles them from webview events.
    #[tokio::test]
    async fn shell_status_reports_the_schedulers_counters_and_no_seeding() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;

        let status = runtime.shell_status().await;
        assert_eq!(status.active_downloads, 0, "nothing has been queued yet");
        assert_eq!(
            status.seeding, 0,
            "no torrent engine is composed into the runtime yet (PG-E10)"
        );
        assert!(status.tray_enabled);
        runtime.shutdown().await;
    }
}
