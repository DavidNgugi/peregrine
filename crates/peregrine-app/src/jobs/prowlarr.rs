//! Installing and starting the managed Prowlarr pack (docs/adr/0024).
//!
//! Both are jobs rather than commands. The install streams about 59 MB and verifies it, and the
//! start waits up to ninety seconds for Prowlarr's own health check; neither fits a command's
//! budget (AGENTS R10), so the scheduler runs them and the bus reports their progress.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use peregrine_secrets::SecretStore;
use peregrine_storage::{Database, JobChange};
use peregrine_sync::{Job, JobContext, JobError, JobFuture};
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::events::{Event, EventBus, JOB_PROGRESS};
use crate::prowlarr::{self, Installed, Supervisor};

/// The kind the install job registers under.
pub const INSTALL_KIND: &str = "prowlarr.install";
/// The kind the start job registers under.
pub const START_KIND: &str = "prowlarr.start";
/// A person asked for this and is waiting on it, so it outranks background maintenance.
pub const PRIORITY: i32 = 4;

/// The managed instance's state, shared between the runtime and the jobs that move it.
///
/// Two locks rather than one, so a status read never waits on a download.
#[derive(Default)]
pub struct Managed {
    installed: Mutex<Option<Installed>>,
    supervisor: Mutex<Option<Supervisor>>,
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
        phase(&self.database, &self.bus, self.journal_id, job, attempt, 50).await;
        let installed = prowlarr::install(&self.data_dir).await.map_err(|error| {
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
        phase(
            &self.database,
            &self.bus,
            self.journal_id,
            job,
            attempt,
            100,
        )
        .await;
        let installed = self
            .managed
            .installed_pack()
            .ok_or_else(|| JobError::failed("the Prowlarr pack is not installed yet"))?;
        let supervisor = Supervisor::start(&installed, Arc::clone(&self.store), &self.data_dir)
            .await
            .map_err(|error| {
                JobError::failed(format!(
                    "the managed Prowlarr could not be started: {error}"
                ))
            })?;
        self.managed.set_supervisor(supervisor);
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

impl Job for ProwlarrStartJob {
    fn run(&mut self, context: &JobContext<'_>) -> JobFuture<'_> {
        let job = context.id();
        let attempt = context.attempt();
        let cancellation = context.cancellation().clone();
        Box::pin(self.start(job, attempt, cancellation))
    }
}
