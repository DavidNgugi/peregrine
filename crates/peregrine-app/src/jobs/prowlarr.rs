//! Installing and starting the managed Prowlarr pack (docs/adr/0024).
//!
//! Both are jobs rather than commands. The install streams about 59 MB and verifies it, and the
//! start waits up to ninety seconds for Prowlarr's own health check; neither fits a command's
//! budget (AGENTS R10), so the scheduler runs them and the bus reports their progress.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use peregrine_secrets::SecretStore;
use peregrine_storage::{Database, JobChange};
use peregrine_sync::{Job, JobContext, JobError, JobFuture};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::events::{Event, EventBus, JOB_PROGRESS};
use crate::prowlarr::{self, Installed, Supervisor};

/// The kind the install job registers under.
pub const INSTALL_KIND: &str = "prowlarr.install";
/// The kind the start job registers under.
pub const START_KIND: &str = "prowlarr.start";
/// A person asked for this and is waiting on it, so it outranks background maintenance.
pub const PRIORITY: i32 = 4;

/// What the managed instance is doing, for a status a screen polls while a job runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// Nothing in flight.
    #[default]
    Idle,
    /// The pinned pack is downloading.
    Installing,
    /// The instance is booting and its key is being read.
    Starting,
}

impl Phase {
    /// The wire spelling a settings screen reads.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Installing => "installing",
            Self::Starting => "starting",
        }
    }
}

/// The managed instance's state, shared between the runtime and the jobs that move it.
///
/// The locks are separate and the byte counter is atomic, so a status read never waits on a
/// download.
#[derive(Default)]
pub struct Managed {
    installed: Mutex<Option<Installed>>,
    supervisor: Mutex<Option<Supervisor>>,
    phase: Mutex<Phase>,
    progress: AtomicU64,
}

impl Managed {
    /// Builds the state by discovering a pack already on disk under `data_dir`.
    ///
    /// Without this a restart would report an installed pack as not installed until the user
    /// pressed Install again, because the state is otherwise in memory only.
    #[must_use]
    pub fn discover(data_dir: &Path) -> Self {
        Self {
            installed: Mutex::new(prowlarr::installed(data_dir)),
            supervisor: Mutex::new(None),
            phase: Mutex::new(Phase::Idle),
            progress: AtomicU64::new(0),
        }
    }

    /// Whether the verified pack is on disk.
    #[must_use]
    pub fn is_installed(&self) -> bool {
        self.installed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// Whether a managed Prowlarr is running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.supervisor
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// The loopback endpoint, when one is running. Never the key.
    #[must_use]
    pub fn base_url(&self) -> Option<String> {
        self.supervisor
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|supervisor| supervisor.base_url().to_owned())
    }

    /// What the managed instance is doing right now.
    #[must_use]
    pub fn phase(&self) -> Phase {
        *self.phase.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sets the phase a status read reports.
    pub fn set_phase(&self, phase: Phase) {
        *self.phase.lock().unwrap_or_else(PoisonError::into_inner) = phase;
    }

    /// Bytes of the pinned pack streamed so far, for install progress.
    #[must_use]
    pub fn downloaded_bytes(&self) -> u64 {
        self.progress.load(Ordering::Relaxed)
    }

    /// The installed pack, cloned out so a caller never holds the lock across an await.
    fn installed_pack(&self) -> Option<Installed> {
        self.installed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_installed(&self, installed: Installed) {
        *self
            .installed
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(installed);
    }

    fn set_supervisor(&self, supervisor: Supervisor) {
        *self
            .supervisor
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(supervisor);
    }

    /// Stops the managed instance, so no listener outlives the runtime.
    pub fn stop(&self) {
        let _ = self
            .supervisor
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
    }
}

/// Reports progress the way every other job here does.
async fn phase(
    database: &Database,
    bus: &EventBus,
    journal_id: i64,
    job: ulid::Ulid,
    attempt: u32,
    permille: u16,
) {
    if let Err(error) = database
        .update_job(
            journal_id,
            JobChange::Progress {
                permille: i64::from(permille),
            },
        )
        .await
    {
        debug!(%error, "job progress could not be journalled");
    }
    bus.publish(Event::new(
        JOB_PROGRESS,
        job.to_string(),
        crate::jobs::progress(job, journal_id, permille, attempt),
    ));
}

/// Downloads, verifies, and unpacks the pinned Prowlarr.
pub struct ProwlarrInstallJob {
    database: Arc<Database>,
    bus: Arc<EventBus>,
    managed: Arc<Managed>,
    data_dir: PathBuf,
    journal_id: i64,
}

impl ProwlarrInstallJob {
    /// A job that installs the pinned pack under `data_dir`.
    #[must_use]
    pub fn new(
        database: Arc<Database>,
        bus: Arc<EventBus>,
        managed: Arc<Managed>,
        data_dir: PathBuf,
        journal_id: i64,
    ) -> Self {
        Self {
            database,
            bus,
            managed,
            data_dir,
            journal_id,
        }
    }

    async fn install(
        &mut self,
        job: ulid::Ulid,
        attempt: u32,
        _cancellation: CancellationToken,
    ) -> peregrine_sync::JobResult {
        self.managed.set_phase(Phase::Installing);
        phase(&self.database, &self.bus, self.journal_id, job, attempt, 50).await;
        let installed = prowlarr::install(&self.data_dir, &self.managed.progress).await;
        self.managed.set_phase(Phase::Idle);
        let installed = installed.map_err(|error| {
            JobError::failed(format!("the Prowlarr pack could not be installed: {error}"))
        })?;
        self.managed.set_installed(installed);
        phase(
            &self.database,
            &self.bus,
            self.journal_id,
            job,
            attempt,
            1000,
        )
        .await;
        Ok(Box::new(()))
    }
}

impl Job for ProwlarrInstallJob {
    fn run(&mut self, context: &JobContext<'_>) -> JobFuture<'_> {
        let job = context.id();
        let attempt = context.attempt();
        let cancellation = context.cancellation().clone();
        Box::pin(self.install(job, attempt, cancellation))
    }
}

/// Starts the installed Prowlarr and stores the endpoint and key it generated.
pub struct ProwlarrStartJob {
    database: Arc<Database>,
    bus: Arc<EventBus>,
    managed: Arc<Managed>,
    store: Arc<dyn SecretStore>,
    data_dir: PathBuf,
    journal_id: i64,
}

impl ProwlarrStartJob {
    /// A job that starts the installed pack under `data_dir`.
    #[must_use]
    pub fn new(
        database: Arc<Database>,
        bus: Arc<EventBus>,
        managed: Arc<Managed>,
        store: Arc<dyn SecretStore>,
        data_dir: PathBuf,
        journal_id: i64,
    ) -> Self {
        Self {
            database,
            bus,
            managed,
            store,
            data_dir,
            journal_id,
        }
    }

    async fn start(
        &mut self,
        job: ulid::Ulid,
        attempt: u32,
        _cancellation: CancellationToken,
    ) -> peregrine_sync::JobResult {
        if self.managed.is_running() {
            // One managed instance is all there can be, so a second Start is a no-op rather than
            // a second spawn fighting for the same port and config directory.
            return Ok(Box::new(()));
        }
        let installed = self
            .managed
            .installed_pack()
            .ok_or_else(|| JobError::failed("the Prowlarr pack is not installed yet"))?;
        self.managed.set_phase(Phase::Starting);
        phase(
            &self.database,
            &self.bus,
            self.journal_id,
            job,
            attempt,
            100,
        )
        .await;
        let started = Supervisor::start(&installed, Arc::clone(&self.store), &self.data_dir).await;
        self.managed.set_phase(Phase::Idle);
        let supervisor = started.map_err(|error| {
            JobError::failed(format!(
                "the managed Prowlarr could not be started: {error}"
            ))
        })?;
        self.managed.set_supervisor(supervisor);
        self.seed_default_indexers().await;
        phase(
            &self.database,
            &self.bus,
            self.journal_id,
            job,
            attempt,
            1000,
        )
        .await;
        Ok(Box::new(()))
    }

    /// The public indexers an empty Prowlarr is seeded with.
    ///
    /// PRODUCT DECISION, and a deliberate departure from ADR 0009 decision 9 (Peregrine ships
    /// every built-in indexer disabled and indexes nothing by default): the app now enables these
    /// unless the user removes them. They are public trackers, no account is needed, and this one
    /// constant is the whole list, so it is trivial to change or drop.
    const PUBLIC_DEFAULTS: [&'static str; 5] =
        ["1337x", "The Pirate Bay", "YTS", "EZTV", "Nyaa.si"];

    /// Adds the curated public indexers when Prowlarr has none configured. Best effort: a tracker
    /// that is down or refuses the add is logged and skipped, and never fails the start.
    async fn seed_default_indexers(&self) {
        let store = Arc::clone(&self.store);
        let configured = match crate::indexers::list_indexers_async(Arc::clone(&store)).await {
            Ok(rows) => rows,
            Err(error) => {
                warn!(%error, "could not read the configured indexers to seed defaults");
                return;
            }
        };
        if !configured.is_empty() {
            return;
        }
        let catalog = match crate::indexers::catalog_async(Arc::clone(&store)).await {
            Ok(catalog) => catalog,
            Err(error) => {
                warn!(%error, "could not read the indexer catalog to seed defaults");
                return;
            }
        };
        for target in Self::PUBLIC_DEFAULTS {
            let Some(definition) = catalog
                .iter()
                .find(|definition| matches_seed_name(definition, target))
            else {
                debug!(target, "no catalog definition matched a seed name");
                continue;
            };
            let name = definition.name.clone();
            let key = definition.key.clone();
            match crate::indexers::add_indexer_async(
                Arc::clone(&store),
                key,
                name.clone(),
                Vec::new(),
            )
            .await
            {
                Ok(_) => info!(indexer = %name, "seeded a public indexer"),
                Err(error) => warn!(indexer = %name, %error, "could not seed a public indexer"),
            }
        }
    }
}

/// Whether a catalog definition is the one a seed name means, comparing the definition's name and
/// its implementation with case and punctuation removed, because Prowlarr spells them differently
/// ("Nyaa.si" against an implementation of "Nyaa").
fn matches_seed_name(definition: &peregrine_indexers::IndexerDefinition, target: &str) -> bool {
    fn normalized(text: &str) -> String {
        text.chars()
            .filter(char::is_ascii_alphanumeric)
            .flat_map(char::to_lowercase)
            .collect()
    }
    let wanted = normalized(target);
    let implementation = definition.key.split("::").next().unwrap_or_default();
    normalized(&definition.name) == wanted || normalized(implementation) == wanted
}

impl Job for ProwlarrStartJob {
    fn run(&mut self, context: &JobContext<'_>) -> JobFuture<'_> {
        let job = context.id();
        let attempt = context.attempt();
        let cancellation = context.cancellation().clone();
        Box::pin(self.start(job, attempt, cancellation))
    }
}
