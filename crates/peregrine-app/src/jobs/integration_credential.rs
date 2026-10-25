//! Proving, storing, and clearing one integration's credentials.
//!
//! **The candidate never enters the journal.** The caller parks the values in the registry under a
//! one-use token, and this job takes them from memory; the journal row carries the integration id
//! and the action, never a value (SECURITY.md T13).
//!
//! **A probe runs before a write.** A key the provider refuses is not stored, so a typo is reported
//! while the person is looking at the field rather than discovered later as a feature that does
//! nothing. The exception is an optional field: `TheIntroDB` reads work without a key, and a saved
//! key only becomes visible when a submission needs it, so its probe is the same public read and a
//! server-side key refusal is the only signal available.
//!
//! Every keyring call runs on a blocking thread, because every OS keyring API is synchronous and
//! the first read from a new binary can raise a prompt (PG-E5-12).

use std::sync::Arc;
use std::time::Duration;

use peregrine_domain::Clock;
use peregrine_providers::tmdb::Tmdb;
use peregrine_providers::{
    AniSkip, IntroDb, MarkerError, MarkerProvider, MarkerRequest, MetadataProvider,
};
use peregrine_secrets::{Secret, SecretKey, SecretStore, SecretStoreError};
use peregrine_storage::{Database, JobChange, JobPhase};
use peregrine_sync::{Job, JobContext, JobError, JobFuture, JobOutput};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use ulid::Ulid;

use crate::events::{Event, EventBus, JOB_PROGRESS, JOB_STATE};
use crate::integrations::{IntegrationManifest, IntegrationRegistry, Probe};
use crate::jobs::{self, CANCELLED_CODE};

/// The kind this job registers under.
pub const KIND: &str = "integration.credential";

/// Scheduling priority: a settings field somebody is waiting at.
pub const PRIORITY: i32 = 100;

/// The stable code a refused credential records.
pub const INTEGRATION_UNAUTHORIZED: &str = "INTEGRATION_UNAUTHORIZED";

/// The stable code a rate-limited service records.
pub const INTEGRATION_RATE_LIMITED: &str = "INTEGRATION_RATE_LIMITED";

/// The stable code any other reachability failure records.
pub const INTEGRATION_UNREACHABLE: &str = "INTEGRATION_UNREACHABLE";

/// The stable code for an integration that needs a credential and has none.
pub const INTEGRATION_NOT_CONFIGURED: &str = "INTEGRATION_NOT_CONFIGURED";

/// The stable code for a keyring failure.
pub const INTEGRATION_STORE: &str = "INTEGRATION_STORE";

/// What to do with one integration's credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Prove the stored values still work.
    Verify,
    /// Prove a candidate set and store it only when the service accepts it.
    Replace,
    /// Remove every field.
    Forget,
}

/// A job that proves, stores, or clears one integration's credentials.
pub struct IntegrationCredentialJob {
    bus: Arc<EventBus>,
    clock: Arc<dyn Clock + Send + Sync>,
    database: Arc<Database>,
    secrets: Arc<dyn SecretStore>,
    registry: Arc<IntegrationRegistry>,
    journal_id: i64,
    manifest: &'static IntegrationManifest,
    action: Action,
    token: Option<u64>,
}

impl IntegrationCredentialJob {
    /// Builds the job for one journal row.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        bus: Arc<EventBus>,
        clock: Arc<dyn Clock + Send + Sync>,
        database: Arc<Database>,
        secrets: Arc<dyn SecretStore>,
        registry: Arc<IntegrationRegistry>,
        journal_id: i64,
        manifest: &'static IntegrationManifest,
        action: Action,
        token: Option<u64>,
    ) -> Self {
        Self {
            bus,
            clock,
            database,
            secrets,
            registry,
            journal_id,
            manifest,
            action,
            token,
        }
    }

    /// The work, which every failure short-circuits into one typed value.
    async fn work(&mut self, cancellation: &CancellationToken) -> Result<(), Failure> {
        if cancellation.is_cancelled() {
            return Err(Failure::Cancelled);
        }
        match self.action {
            Action::Verify => self.verify(cancellation).await,
            Action::Replace => self.replace(cancellation).await,
            Action::Forget => self.forget(cancellation).await,
        }
    }

    /// Proves the stored values.
    async fn verify(&self, cancellation: &CancellationToken) -> Result<(), Failure> {
        let values = self.stored().await?;
        if self
            .manifest
            .credential_fields
            .iter()
            .any(|field| field.required && !values.iter().any(|(name, _)| name == field.name))
        {
            return Err(Failure::NotConfigured);
        }
        if cancellation.is_cancelled() {
            return Err(Failure::Cancelled);
        }
        self.probe(&values).await
    }

    /// Proves a parked candidate and stores it only when the service accepted it.
    async fn replace(&mut self, cancellation: &CancellationToken) -> Result<(), Failure> {
        let token = self.token.take().ok_or(Failure::NotConfigured)?;
        let pending = self.registry.take(token).ok_or(Failure::NotConfigured)?;
        if pending.integration != self.manifest.id() {
            return Err(Failure::NotConfigured);
        }
        let values: Vec<(String, Secret)> = pending.values;
        if cancellation.is_cancelled() {
            return Err(Failure::Cancelled);
        }
        self.probe(&values).await?;
        if cancellation.is_cancelled() {
            return Err(Failure::Cancelled);
        }
        for (name, secret) in values {
            let Some(field) = self
                .manifest
                .credential_fields
                .iter()
                .find(|field| field.name == name)
            else {
                continue;
            };
            let key = self.manifest.secret_key(field);
            let store = Arc::clone(&self.secrets);
            let secret = Secret::new(secret.expose_secret().to_owned());
            blocking(move || store.set(&key, &secret)).await??;
            self.registry.set_present(self.manifest, field, true);
        }
        self.record(peregrine_domain::IntegrationOutcome::Working, None)
            .await;
        Ok(())
    }

    /// Removes every field.
    async fn forget(&self, cancellation: &CancellationToken) -> Result<(), Failure> {
        if cancellation.is_cancelled() {
            return Err(Failure::Cancelled);
        }
        for field in self.manifest.credential_fields {
            let key = self.manifest.secret_key(field);
            let store = Arc::clone(&self.secrets);
            blocking(move || store.delete(&key)).await??;
            self.registry.set_present(self.manifest, field, false);
        }
        self.record(peregrine_domain::IntegrationOutcome::NotConfigured, None)
            .await;
        Ok(())
    }

    /// Reads every stored field, off the runtime.
    async fn stored(&self) -> Result<Vec<(String, Secret)>, Failure> {
        let mut values = Vec::new();
        for field in self.manifest.credential_fields {
            let key = self.manifest.secret_key(field);
            let store = Arc::clone(&self.secrets);
            if let Some(secret) = blocking(move || store.get(&key)).await?? {
                values.push((field.name.to_owned(), secret));
            }
        }
        Ok(values)
    }

    /// Asks the service whether these values work.
    async fn probe(&self, values: &[(String, Secret)]) -> Result<(), Failure> {
        let value = |name: &str| {
            values
                .iter()
                .find(|(field, _)| field == name)
                .map(|(_, secret)| secret.expose_secret().to_owned())
        };
        match self.manifest.probe {
            Probe::None => Ok(()),
            Probe::Tmdb => {
                let Some(key) = value("apiKey") else {
                    return Err(Failure::NotConfigured);
                };
                let adapter = Tmdb::new(Tmdb::BASE_URL, peregrine_providers::tmdb::IMAGE_BASE, key)
                    .map_err(|_| Failure::Build)?;
                adapter.check().await.map_err(Failure::Provider)
            }
            Probe::IntroDb => {
                let adapter =
                    IntroDb::new(IntroDb::BASE_URL, value("apiKey")).map_err(Failure::Marker)?;
                probe_marker(&adapter, INTRO_DB_PROBE).await
            }
            Probe::AniSkip => {
                let adapter = AniSkip::new(AniSkip::BASE_URL).map_err(Failure::Marker)?;
                probe_marker(&adapter, ANI_SKIP_PROBE).await
            }
        }
    }

    /// Records the outcome in the database and the cache; best-effort, because the answer is
    /// already known and a health row is not what the person is waiting for.
    async fn record(
        &self,
        outcome: peregrine_domain::IntegrationOutcome,
        code: Option<&'static str>,
    ) {
        let at = self.clock.now().as_millisecond();
        if let Err(error) = self
            .database
            .record_integration_outcome(&self.manifest.id(), outcome, code, at)
            .await
        {
            debug!(%error, "the integration outcome could not be journalled");
        }
        self.registry
            .remember_outcome(&self.manifest.id(), outcome, code, at);
    }

    /// Records progress and publishes it.
    async fn phase(&self, job: Ulid, attempt: u32, permille: u16) {
        if let Err(error) = self
            .database
            .update_job(
                self.journal_id,
                JobChange::Progress {
                    permille: i64::from(permille),
                },
            )
            .await
        {
            debug!(%error, "job progress could not be journalled");
        }
        self.bus.publish(Event::new(
            JOB_PROGRESS,
            job.to_string(),
            jobs::progress(job, self.journal_id, permille, attempt),
        ));
    }

    /// Records a finished job.
    async fn finish(&self, job: Ulid, attempt: u32, phase: JobPhase, code: Option<&str>) {
        let at = self.clock.now().as_millisecond();
        if let Err(error) = self
            .database
            .update_job(
                self.journal_id,
                JobChange::Finished {
                    at,
                    phase,
                    error_code: code.map(str::to_owned),
                    error: code.map(|code| message_for(code).to_owned()),
                },
            )
            .await
        {
            debug!(%error, "job completion could not be journalled");
        }
        self.bus.publish(Event::new(
            JOB_STATE,
            job.to_string(),
            jobs::state(job, self.journal_id, KIND, phase, attempt),
        ));
    }
}

impl Job for IntegrationCredentialJob {
    fn run(&mut self, context: &JobContext<'_>) -> JobFuture<'_> {
        let job = context.id();
        let attempt = context.attempt();
        let cancellation = context.cancellation().clone();
        Box::pin(async move {
            self.phase(job, attempt, 100).await;
            match self.work(&cancellation).await {
                Ok(()) => {
                    self.finish(job, attempt, JobPhase::Succeeded, None).await;
                    info!(
                        integration = self.manifest.id,
                        action = ?self.action,
                        "an integration credential was handled"
                    );
                    Ok(Box::new(()) as JobOutput)
                }
                Err(Failure::Cancelled) => {
                    self.finish(job, attempt, JobPhase::Canceled, Some(CANCELLED_CODE))
                        .await;
                    Err(JobError::Cancelled)
                }
                Err(failure) => {
                    let code = failure.code();
                    self.record(failure.outcome(), Some(code)).await;
                    self.finish(job, attempt, JobPhase::Failed, Some(code))
                        .await;
                    warn!(
                        integration = self.manifest.id,
                        action = ?self.action,
                        code,
                        "an integration credential could not be handled"
                    );
                    Err(JobError::failed(message_for(code)))
                }
            }
        })
    }
}

/// The probe request `TheIntroDB`'s documentation uses for a public example episode.
const INTRO_DB_PROBE: MarkerRequest = MarkerRequest {
    kind: peregrine_domain::TitleKind::Show,
    season: Some(1),
    episode: Some(1),
    tvdb_id: Some(71_663),
    tmdb_id: None,
    imdb_id: None,
    mal_id: None,
    duration_ms: Some(1_440_000),
    is_anime: false,
};

/// The probe request `AniSkip` answers for a documented anime episode.
const ANI_SKIP_PROBE: MarkerRequest = MarkerRequest {
    kind: peregrine_domain::TitleKind::Show,
    season: Some(1),
    episode: Some(1),
    tvdb_id: None,
    tmdb_id: None,
    imdb_id: None,
    mal_id: Some(21),
    duration_ms: Some(1_440_000),
    is_anime: true,
};

/// Runs one marker probe under a bounded wait; an empty answer is still a working service.
async fn probe_marker(adapter: &dyn MarkerProvider, request: MarkerRequest) -> Result<(), Failure> {
    tokio::time::timeout(Duration::from_secs(8), adapter.fetch(request))
        .await
        .map_err(|_| Failure::Unreachable)?
        .map(|_| ())
        .map_err(Failure::Marker)
}

/// Why a credential could not be handled.
enum Failure {
    /// The service could not be reached, or answered something unusable.
    Unreachable,
    /// The keyring failed.
    Store(SecretStoreError),
    /// An adapter could not be built.
    Build,
    /// A marker adapter failed.
    Marker(MarkerError),
    /// Nothing is stored and something is required.
    NotConfigured,
    /// A metadata provider failed outside a marker probe.
    Provider(peregrine_providers::ProviderError),
    /// The caller stopped waiting.
    Cancelled,
}

impl From<SecretStoreError> for Failure {
    fn from(error: SecretStoreError) -> Self {
        Self::Store(error)
    }
}

impl Failure {
    const fn code(&self) -> &'static str {
        match self {
            Self::Provider(peregrine_providers::ProviderError::Unauthorized { .. }) => {
                INTEGRATION_UNAUTHORIZED
            }
            Self::Marker(MarkerError::RateLimited { .. })
            | Self::Provider(peregrine_providers::ProviderError::RateLimited { .. }) => {
                INTEGRATION_RATE_LIMITED
            }
            Self::Store(_) => INTEGRATION_STORE,
            Self::NotConfigured => INTEGRATION_NOT_CONFIGURED,
            Self::Cancelled => CANCELLED_CODE,
            Self::Unreachable | Self::Build | Self::Marker(_) | Self::Provider(_) => {
                INTEGRATION_UNREACHABLE
            }
        }
    }

    const fn outcome(&self) -> peregrine_domain::IntegrationOutcome {
        match self {
            Self::Cancelled | Self::NotConfigured => {
                peregrine_domain::IntegrationOutcome::NotConfigured
            }
            Self::Store(SecretStoreError::Denied) => {
                peregrine_domain::IntegrationOutcome::KeychainDenied
            }
            Self::Store(_) => peregrine_domain::IntegrationOutcome::Unreachable,
            Self::Provider(peregrine_providers::ProviderError::Unauthorized { .. }) => {
                peregrine_domain::IntegrationOutcome::Unauthorized
            }
            Self::Marker(MarkerError::RateLimited { .. })
            | Self::Provider(peregrine_providers::ProviderError::RateLimited { .. }) => {
                peregrine_domain::IntegrationOutcome::RateLimited
            }
            Self::Unreachable | Self::Build | Self::Marker(_) | Self::Provider(_) => {
                peregrine_domain::IntegrationOutcome::Unreachable
            }
        }
    }
}

/// The sentence a settings screen can show for a stable code.
fn message_for(code: &str) -> &'static str {
    match code {
        INTEGRATION_UNAUTHORIZED => {
            "That service refused the credential. Check the key and try again."
        }
        INTEGRATION_RATE_LIMITED => "That service is rate limiting this machine. Try again later.",
        INTEGRATION_NOT_CONFIGURED => {
            "Enter the credential this integration needs before testing it."
        }
        INTEGRATION_STORE => "The operating system's keychain refused the request.",
        INTEGRATION_UNREACHABLE => {
            "That service could not be reached, or answered something unusable."
        }
        _ => "The connection test did not finish.",
    }
}

/// Runs one keyring call on a blocking thread.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Failure> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| Failure::Store(SecretStoreError::Failed))
}

/// The dedupe key for one integration, so a double-click waits on one probe.
#[must_use]
pub fn dedupe_key(integration: &str) -> String {
    format!("{KIND}:{integration}")
}

/// The journal payload: the integration id, the action, and the parked token. Never a value.
#[must_use]
pub fn payload(integration: &str, action: Action, token: Option<u64>) -> String {
    serde_json::json!({
        "integration": integration,
        "action": match action {
            Action::Verify => "verify",
            Action::Replace => "replace",
            Action::Forget => "forget",
        },
        "token": token,
    })
    .to_string()
}

/// A helper for the IPC layer: the keyring account one field uses.
#[must_use]
pub fn key_for(integration: &str, field: &str) -> SecretKey {
    SecretKey::integration(integration, field)
}
