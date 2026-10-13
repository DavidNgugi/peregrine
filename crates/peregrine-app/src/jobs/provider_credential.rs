//! Proving a provider credential, storing it, and clearing it.
//!
//! **Validate before storing.** A `Replace` probes the candidate with the same constructor the
//! registry uses and only writes it once the provider accepted it, so a key that does not work never
//! reaches the keychain and a person who mistyped one is told so rather than discovering it later as
//! a library that fills in nothing. The candidate lives in this job's memory and in the keyring:
//! the journal row names the provider and nothing else, because a credential in `payload_json` is a
//! credential in a file, in a backup, and in a diagnostic bundle (SECURITY.md T13).
//!
//! **Every keyring call runs on a blocking thread.** Reading one is synchronous and, on macOS, the
//! first read from a new binary raises an OS prompt that measured 11.7 s (PG-E5-12). A job that made
//! that call on a worker thread would hold a slot and stall the queue behind it (AGENTS §5), so the
//! port's blocking shape meets the runtime in exactly one helper below.

use std::sync::Arc;

use peregrine_domain::Clock;
use peregrine_providers::{MetadataProvider, ProviderError};
use peregrine_secrets::{Secret, SecretKey, SecretStore, SecretStoreError};
use peregrine_storage::{Database, JobChange, JobPhase};
use peregrine_sync::{Job, JobContext, JobError, JobFuture, JobOutput, JobResult};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use ulid::Ulid;

use crate::events::{Event, EventBus, JOB_PROGRESS, JOB_STATE};
use crate::jobs::{self, CANCELLED_CODE};
use crate::providers::{BuildError, ProviderManifest, ProviderRegistry};

/// The kind this job registers under, in the scheduler and in the journal.
pub const KIND: &str = "provider.credential";

/// Scheduling priority: the same as the work a person asked for, because that is what this is.
///
/// **Higher runs first** — `peregrine_sync`'s ready heap pops the highest priority and breaks ties by
/// submission order (`heap.rs` and its own test), which is what `library_scan` and `hello` at 100 and
/// `provider_refresh` at 10 already mean. `NewJob::priority`'s doc said the opposite and is corrected
/// in the same change; a settings field somebody is waiting at must not queue behind a poster.
pub const PRIORITY: i32 = 100;

/// The stable code a refused credential records.
pub const PROVIDER_UNAUTHORIZED: &str = "PROVIDER_UNAUTHORIZED";

/// The stable code a rate-limited provider records.
pub const PROVIDER_RATE_LIMITED: &str = "PROVIDER_RATE_LIMITED";

/// The stable code any other provider failure records: the provider could not be reached at all.
pub const PROVIDER_UNREACHABLE: &str = "PROVIDER_UNREACHABLE";

/// The stable code a request to use a provider with nothing stored records.
pub const PROVIDER_NOT_CONFIGURED: &str = "PROVIDER_NOT_CONFIGURED";

/// The stable code a store or adapter-construction failure records.
pub const PROVIDER_STORE: &str = "PROVIDER_STORE";

/// What to do with one provider's credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Prove the stored credential still works.
    Verify,
    /// Prove a candidate and store it only when the provider accepts it.
    Replace,
    /// Remove whatever is stored.
    Forget,
}

/// How an adapter is built from a provider and a credential.
///
/// A function rather than a trait object table, and a parameter rather than a call to
/// [`crate::providers::build_with`] inside the job: the job is tested with a fake adapter, and the
/// runtime supplies the real constructor.
pub type Build = fn(&ProviderManifest, &Secret) -> Result<Arc<dyn MetadataProvider>, BuildError>;

/// A job that proves, stores, or clears one provider's credential.
pub struct ProviderCredentialJob {
    bus: Arc<EventBus>,
    clock: Arc<dyn Clock + Send + Sync>,
    database: Arc<Database>,
    providers: Arc<ProviderRegistry>,
    secrets: Arc<dyn SecretStore>,
    journal_id: i64,
    entry: &'static ProviderManifest,
    action: Action,
    candidate: Option<Secret>,
    build: Build,
}

impl ProviderCredentialJob {
    /// A job that performs `action` for `entry`'s provider, reporting into `journal_id`.
    ///
    /// Ten collaborators is one per thing this job touches, and each is the runtime's own handle
    /// rather than a value assembled here: wrapping them would be a second type for one call site.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        bus: Arc<EventBus>,
        clock: Arc<dyn Clock + Send + Sync>,
        database: Arc<Database>,
        providers: Arc<ProviderRegistry>,
        secrets: Arc<dyn SecretStore>,
        journal_id: i64,
        entry: &'static ProviderManifest,
        action: Action,
        candidate: Option<Secret>,
        build: Build,
    ) -> Self {
        Self {
            bus,
            clock,
            database,
            providers,
            secrets,
            journal_id,
            entry,
            action,
            candidate,
            build,
        }
    }

    /// Records which half of the work is running, so a settings field can say so.
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

    /// The work, which every failure short-circuits into one typed value.
    async fn work(
        &mut self,
        job: Ulid,
        attempt: u32,
        cancellation: &CancellationToken,
    ) -> Result<(), Failure> {
        if cancellation.is_cancelled() {
            return Err(Failure::Cancelled);
        }
        match self.action {
            Action::Verify => self.verify(job, attempt, cancellation).await,
            Action::Replace => self.replace(job, attempt, cancellation).await,
            Action::Forget => self.forget(job, attempt, cancellation).await,
        }
    }

    /// Asks the provider whether the stored credential is accepted.
    async fn verify(
        &mut self,
        job: Ulid,
        attempt: u32,
        cancellation: &CancellationToken,
    ) -> Result<(), Failure> {
        let stored = self.stored().await?;
        // Never spend a request to find out there is nothing to test.
        let Some(secret) = stored else {
            return Err(Failure::NotConfigured);
        };
        if cancellation.is_cancelled() {
            return Err(Failure::Cancelled);
        }
        self.probe(job, attempt, &secret).await
    }

    /// Proves the candidate and stores it only if the provider accepted it.
    async fn replace(
        &mut self,
        job: Ulid,
        attempt: u32,
        cancellation: &CancellationToken,
    ) -> Result<(), Failure> {
        // The runtime supplies a candidate with every `Replace`; a job built without one has
        // nothing to prove and must not be able to store anything.
        let Some(candidate) = self.candidate.take() else {
            return Err(Failure::NotConfigured);
        };
        self.probe(job, attempt, &candidate).await?;
        // Checked *after* the probe: a person who cancels while the check is in flight must not
        // have the key stored by an answer that arrived once they had stopped waiting for it.
        if cancellation.is_cancelled() {
            return Err(Failure::Cancelled);
        }

        self.phase(job, attempt, 700).await;
        let key = SecretKey::provider(self.entry.provider);
        let store = Arc::clone(&self.secrets);
        // The candidate is moved into the blocking write and dropped with it: `Secret` is
        // deliberately not `Clone`, so there is exactly one copy to lose.
        blocking(move || store.set(&key, &candidate))
            .await?
            .map_err(Failure::Store)?;
        self.refresh().await
    }

    /// Removes whatever is stored for this provider.
    async fn forget(
        &mut self,
        job: Ulid,
        attempt: u32,
        cancellation: &CancellationToken,
    ) -> Result<(), Failure> {
        if cancellation.is_cancelled() {
            return Err(Failure::Cancelled);
        }
        self.phase(job, attempt, 300).await;
        let key = SecretKey::provider(self.entry.provider);
        let store = Arc::clone(&self.secrets);
        // `SecretStore::delete` documents removing nothing as `Ok(())`: a disconnect that is
        // already disconnected has done what it was asked.
        blocking(move || store.delete(&key))
            .await?
            .map_err(Failure::Store)?;
        self.refresh().await
    }

    /// Reads this provider's stored credential, off the runtime.
    async fn stored(&self) -> Result<Option<Secret>, Failure> {
        let key = SecretKey::provider(self.entry.provider);
        let store = Arc::clone(&self.secrets);
        blocking(move || store.get(&key))
            .await?
            .map_err(Failure::Store)
    }

    /// Builds an adapter from `secret` and asks the provider whether it is accepted.
    ///
    /// Cancellation is not checked inside: the port's probe has no cancellation point the caller
    /// can reach, and an answer that arrives is an answer the caller decides what to do with.
    async fn probe(&self, job: Ulid, attempt: u32, secret: &Secret) -> Result<(), Failure> {
        self.phase(job, attempt, 300).await;
        let adapter = (self.build)(self.entry, secret).map_err(Failure::Build)?;
        adapter.check().await.map_err(Failure::Provider)
    }

    /// Rebuilds the registry's cache from the store, off the runtime.
    ///
    /// The registry is filled once at startup and every polled read answers from it, so a key that
    /// was just stored or removed is invisible until this runs — the same blocking read that filled
    /// it, for the same reason.
    async fn refresh(&self) -> Result<(), Failure> {
        let providers = Arc::clone(&self.providers);
        let store = Arc::clone(&self.secrets);
        blocking(move || providers.refresh(store.as_ref()))
            .await?
            .map_err(Failure::Build)?;
        Ok(())
    }

    /// Records that the work finished and publishes the phase.
    ///
    /// The progress write comes first because `JobChange::Finished` does not touch
    /// `progress_permille`: a row left at the last reported fraction would show finished work as
    /// seven-tenths done.
    async fn succeeded(&self, job: Ulid, attempt: u32) -> JobResult {
        let at = self.clock.now().as_millisecond();
        for change in [
            JobChange::Progress { permille: 1_000 },
            JobChange::Finished {
                at,
                phase: JobPhase::Succeeded,
                error_code: None,
                error: None,
            },
        ] {
            if let Err(error) = self.database.update_job(self.journal_id, change).await {
                debug!(%error, "job completion could not be journalled");
            }
        }
        self.bus.publish(Event::new(
            JOB_STATE,
            job.to_string(),
            jobs::state(job, self.journal_id, KIND, JobPhase::Succeeded, attempt),
        ));
        info!(
            provider = self.entry.provider,
            action = ?self.action,
            "a provider credential was handled"
        );
        Ok(Box::new(()) as JobOutput)
    }

    /// Records the typed failure and answers with it, which is the shape every job here uses.
    async fn failed(&self, job: Ulid, attempt: u32, failure: Failure) -> JobResult {
        let code = failure.code();
        let message = failure.message();
        if let Err(error) = self
            .database
            .update_job(
                self.journal_id,
                JobChange::Finished {
                    at: self.clock.now().as_millisecond(),
                    phase: JobPhase::Failed,
                    error_code: Some(code.to_owned()),
                    error: Some(message.clone()),
                },
            )
            .await
        {
            debug!(%error, "job failure could not be journalled");
        }
        self.bus.publish(Event::new(
            JOB_STATE,
            job.to_string(),
            jobs::state(job, self.journal_id, KIND, JobPhase::Failed, attempt),
        ));
        // The message is the failing type's own `Display`: a provider's error and a store's error
        // both name only what happened, never a key, a URL, or a response body (SECURITY.md T13).
        warn!(
            provider = self.entry.provider,
            action = ?self.action,
            code,
            %message,
            "a provider credential could not be handled"
        );
        Err(JobError::failed(message))
    }

    /// Records that the job stopped because it was asked to.
    async fn cancelled(&self, job: Ulid, attempt: u32) -> JobResult {
        if let Err(error) = self
            .database
            .update_job(
                self.journal_id,
                JobChange::Finished {
                    at: self.clock.now().as_millisecond(),
                    phase: JobPhase::Canceled,
                    error_code: Some(CANCELLED_CODE.to_owned()),
                    error: None,
                },
            )
            .await
        {
            debug!(%error, "job cancellation could not be journalled");
        }
        self.bus.publish(Event::new(
            JOB_STATE,
            job.to_string(),
            jobs::state(job, self.journal_id, KIND, JobPhase::Canceled, attempt),
        ));
        info!(
            provider = self.entry.provider,
            action = ?self.action,
            "a provider credential check stopped on request"
        );
        Err(JobError::Cancelled)
    }
}

impl Job for ProviderCredentialJob {
    fn run(&mut self, context: &JobContext<'_>) -> JobFuture<'_> {
        // Copied out of the context before the future is built, because a future that held the
        // context borrow would have to live as long as this job does.
        let job = context.id();
        let attempt = context.attempt();
        let cancellation = context.cancellation().clone();
        Box::pin(self.attempt(job, attempt, cancellation))
    }
}

impl ProviderCredentialJob {
    /// The driver: one outcome per attempt, and every terminal state recorded.
    async fn attempt(
        &mut self,
        job: Ulid,
        attempt: u32,
        cancellation: CancellationToken,
    ) -> JobResult {
        match self.work(job, attempt, &cancellation).await {
            Ok(()) => self.succeeded(job, attempt).await,
            Err(Failure::Cancelled) => self.cancelled(job, attempt).await,
            Err(failure) => self.failed(job, attempt, failure).await,
        }
    }
}

/// Why a credential could not be handled.
///
/// The typed failure exists so that the journal carries a stable code a screen branches on, and a
/// message that is safe to show: none of these types can carry a key, and none of them quotes a URL.
enum Failure {
    /// The provider refused the credential or could not be reached.
    Provider(ProviderError),
    /// The keyring could not be read or written.
    Store(SecretStoreError),
    /// The adapter could not be built from the provider and the credential.
    Build(BuildError),
    /// There is nothing stored to use.
    NotConfigured,
    /// The caller stopped waiting.
    Cancelled,
}

impl Failure {
    /// The stable code the journal records and the UI maps.
    fn code(&self) -> &'static str {
        match self {
            Self::Provider(ProviderError::Unauthorized { .. }) => PROVIDER_UNAUTHORIZED,
            Self::Provider(ProviderError::RateLimited { .. }) => PROVIDER_RATE_LIMITED,
            Self::Provider(_) => PROVIDER_UNREACHABLE,
            Self::Store(_) | Self::Build(_) => PROVIDER_STORE,
            Self::NotConfigured => PROVIDER_NOT_CONFIGURED,
            Self::Cancelled => CANCELLED_CODE,
        }
    }

    /// What to tell the person who asked.
    fn message(&self) -> String {
        match self {
            Self::Provider(error) => error.to_string(),
            Self::Store(error) => error.to_string(),
            Self::Build(error) => error.to_string(),
            Self::NotConfigured => "no credential is stored for this provider".to_owned(),
            Self::Cancelled => "the credential check was cancelled".to_owned(),
        }
    }
}

/// Runs one keyring call on a blocking thread and waits for it.
///
/// A panic inside the adapter becomes a store failure: a caller cannot tell the two apart, and
/// neither leaves a credential stored.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Failure> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| Failure::Store(SecretStoreError::Failed))
}

/// The dedupe key for one provider, so a double-click waits on one check rather than starting two.
#[must_use]
pub fn dedupe_key(provider: &str) -> String {
    format!("{KIND}:{provider}")
}

/// The journal payload this job is recorded with: the provider's name and nothing else.
///
/// The candidate credential lives only in memory and in the keyring, and this function is the only
/// thing that decides what the row holds — which is why it cannot take a credential at all.
#[must_use]
pub fn payload(provider: &str) -> String {
    serde_json::json!({ "provider": provider }).to_string()
}

#[cfg(test)]
mod tests {
    // The fixtures below take the `Build` type's fallible shape and have no failure to report,
    // which is something `unnecessary_wraps` cannot see through a function pointer.
    #![allow(clippy::unnecessary_wraps)]

    use std::sync::Arc;
    use std::time::Duration;

    use peregrine_domain::catalogue::{ImageKind, TitleDetails, TitleKind};
    use peregrine_domain::id::{ExternalId, Provider};
    use peregrine_providers::{
        Capabilities, Capability, ProviderDescriptor, ProviderFuture, SearchCandidate, SearchQuery,
    };
    use peregrine_secrets::SecretStore;
    use peregrine_storage::{Database, JobRecord, NewJob};
    use peregrine_sync::{JobHandle, JobOutcome, JobSpec, Scheduler, SchedulerConfig};
    use peregrine_testkit::{CountingIds, FakeClock, FakeMetadataProvider, MemorySecretStore};
    use tempfile::TempDir;

    use crate::events::{EventSink, JOB_STATE};
    use crate::providers;
    use crate::testing::{CountingSecretStore, RecordingSink};

    use super::*;

    const NOW: i64 = 1_700_000_000_000;

    /// A clock pinned to [`NOW`], so every journal timestamp is predictable.
    fn clock() -> Arc<dyn Clock + Send + Sync> {
        Arc::new(FakeClock::at(NOW))
    }

    /// An adapter whose credential the provider accepts.
    fn accepting(
        _entry: &ProviderManifest,
        _credential: &Secret,
    ) -> Result<Arc<dyn MetadataProvider>, BuildError> {
        Ok(Arc::new(FakeMetadataProvider::searching(
            Provider::Tmdb,
            Vec::new(),
        )))
    }

    /// An adapter whose credential the provider refuses.
    fn refusing(
        _entry: &ProviderManifest,
        _credential: &Secret,
    ) -> Result<Arc<dyn MetadataProvider>, BuildError> {
        Ok(Arc::new(FakeMetadataProvider::refusing_credentials(
            Provider::Tmdb,
        )))
    }

    /// A build that must never run: nothing stored means no request worth spending.
    fn never_built(
        _entry: &ProviderManifest,
        _credential: &Secret,
    ) -> Result<Arc<dyn MetadataProvider>, BuildError> {
        panic!("a provider with nothing stored must not be built")
    }

    /// An adapter whose probe takes long enough for a cancellation to land inside it.
    fn slow(
        entry: &ProviderManifest,
        _credential: &Secret,
    ) -> Result<Arc<dyn MetadataProvider>, BuildError> {
        Ok(Arc::new(SlowProvider::new(entry.provider)))
    }

    /// A provider that answers its probe after a delay, so cancellation has somewhere to land.
    struct SlowProvider {
        descriptor: ProviderDescriptor,
    }

    impl SlowProvider {
        fn new(provider: &str) -> Self {
            let provider = match provider {
                "tmdb" => Provider::Tmdb,
                other => panic!("the fixture only knows tmdb, not `{other}`"),
            };
            Self {
                descriptor: peregrine_testkit::fake_descriptor(
                    provider,
                    Capabilities::EMPTY.with(Capability::Search),
                ),
            }
        }
    }

    impl MetadataProvider for SlowProvider {
        fn descriptor(&self) -> &ProviderDescriptor {
            &self.descriptor
        }

        fn check(&self) -> ProviderFuture<'_, ()> {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(200)).await;
                Ok(())
            })
        }

        fn image(&self, _path: &str, _kind: ImageKind) -> ProviderFuture<'_, Vec<u8>> {
            Box::pin(async {
                Err(ProviderError::Unsupported {
                    provider: Provider::Tmdb,
                    capability: "images",
                })
            })
        }

        fn title(&self, _id: &ExternalId, _kind: TitleKind) -> ProviderFuture<'_, TitleDetails> {
            Box::pin(async {
                Err(ProviderError::Unsupported {
                    provider: Provider::Tmdb,
                    capability: "title",
                })
            })
        }

        fn search(&self, _query: &SearchQuery) -> ProviderFuture<'_, Vec<SearchCandidate>> {
            Box::pin(async {
                Err(ProviderError::Unsupported {
                    provider: Provider::Tmdb,
                    capability: "search",
                })
            })
        }
    }

    /// One database, bus, registry, and scheduler, with the store a test chose.
    struct Fixture {
        directory: TempDir,
        database: Arc<Database>,
        bus: Arc<EventBus>,
        sink: Arc<RecordingSink>,
        registry: Arc<ProviderRegistry>,
        scheduler: Scheduler,
        secrets: Arc<dyn SecretStore>,
    }

    impl Fixture {
        async fn new(secrets: Arc<dyn SecretStore>) -> Self {
            let directory = TempDir::new().unwrap();
            let database = Arc::new(
                Database::open(directory.path().join("peregrine.sqlite"))
                    .await
                    .unwrap(),
            );
            let sink = Arc::new(RecordingSink::default());
            let bus = Arc::new(EventBus::new(Arc::clone(&sink) as Arc<dyn EventSink>));
            let registry = Arc::new(ProviderRegistry::new());
            let scheduler = Scheduler::new(SchedulerConfig::new(
                clock(),
                Arc::new(CountingIds::default()),
            ));
            Self {
                directory,
                database,
                bus,
                sink,
                registry,
                scheduler,
                secrets,
            }
        }

        /// Submits one credential job and answers its handle and journal row.
        async fn start(
            &self,
            action: Action,
            candidate: Option<Secret>,
            build: Build,
        ) -> (JobHandle, i64) {
            let entry = providers::entry("tmdb").expect("tmdb is a known provider");
            let journal_id = self
                .database
                .insert_job(NewJob {
                    kind: KIND.to_owned(),
                    priority: PRIORITY,
                    dedupe_key: Some(dedupe_key(entry.provider)),
                    payload_json: payload(entry.provider),
                    max_attempts: 1,
                    created_at: NOW,
                })
                .await
                .unwrap();
            let job = ProviderCredentialJob::new(
                Arc::clone(&self.bus),
                clock(),
                Arc::clone(&self.database),
                Arc::clone(&self.registry),
                Arc::clone(&self.secrets),
                journal_id,
                entry,
                action,
                candidate,
                build,
            );
            let handle = self
                .scheduler
                .submit(
                    JobSpec {
                        kind: KIND.to_owned(),
                        priority: PRIORITY,
                        dedupe_key: Some(dedupe_key(entry.provider)),
                        max_attempts: 1,
                    },
                    job,
                )
                .await
                .unwrap();
            (handle, journal_id)
        }

        async fn row(&self, journal_id: i64) -> JobRecord {
            self.database
                .job(journal_id)
                .await
                .unwrap()
                .expect("the row exists")
        }
    }

    /// Whether `haystack` holds `needle` as bytes, for the assertion that nothing wrote a key down.
    fn contains(haystack: &[u8], needle: &str) -> bool {
        !needle.is_empty()
            && haystack
                .windows(needle.len())
                .any(|window| window == needle.as_bytes())
    }

    /// A refused credential is never stored: the rule this whole job exists for.
    #[tokio::test]
    async fn a_refused_credential_is_never_stored() {
        let store = Arc::new(CountingSecretStore::new());
        let fixture = Fixture::new(Arc::clone(&store) as Arc<dyn SecretStore>).await;

        let (handle, journal_id) = fixture
            .start(
                Action::Replace,
                Some(Secret::new("a-refused-key")),
                refusing,
            )
            .await;
        let completion = handle.wait().await;

        assert!(
            matches!(completion.outcome(), JobOutcome::Failed(_)),
            "{:?}",
            completion.outcome()
        );
        let row = fixture.row(journal_id).await;
        assert_eq!(row.state, JobPhase::Failed, "{row:?}");
        assert_eq!(
            row.last_error_code.as_deref(),
            Some(PROVIDER_UNAUTHORIZED),
            "{row:?}"
        );
        assert_eq!(store.writes(), 0, "a refused key must not reach the store");
        assert!(!store.holds("tmdb"));
        assert!(
            fixture.registry.status().configured.is_empty(),
            "a refused credential must not configure a provider"
        );

        fixture.scheduler.shutdown().await;
    }

    /// A credential the provider accepts is stored, and the registry that every other read uses
    /// notices.
    #[tokio::test]
    async fn a_working_credential_is_stored_and_the_registry_notices() {
        let store = Arc::new(MemorySecretStore::new());
        let fixture = Fixture::new(Arc::clone(&store) as Arc<dyn SecretStore>).await;

        let (handle, journal_id) = fixture
            .start(
                Action::Replace,
                Some(Secret::new("a-working-key")),
                accepting,
            )
            .await;
        let completion = handle.wait().await;

        assert!(
            matches!(completion.outcome(), JobOutcome::Succeeded(_)),
            "{:?}",
            completion.outcome()
        );
        let row = fixture.row(journal_id).await;
        assert_eq!(row.state, JobPhase::Succeeded, "{row:?}");
        assert_eq!(row.progress_permille, 1000);
        assert_eq!(row.last_error_code, None);
        assert!(
            store
                .get(&SecretKey::provider("tmdb"))
                .expect("the store answers")
                .is_some(),
            "the accepted key is stored"
        );
        assert_eq!(fixture.registry.status().configured, vec!["tmdb"]);

        // The terminal phase is what a settings screen follows, and it must arrive.
        fixture.bus.shutdown().await;
        assert!(
            fixture
                .sink
                .delivered()
                .iter()
                .any(|event| { event.topic == JOB_STATE && event.payload["phase"] == "succeeded" }),
            "the terminal phase must reach the sink"
        );

        fixture.scheduler.shutdown().await;
    }

    /// Nothing stored cannot be verified, and finding that out spends no request.
    #[tokio::test]
    async fn a_provider_with_nothing_stored_cannot_be_verified() {
        let store = Arc::new(CountingSecretStore::new());
        let fixture = Fixture::new(Arc::clone(&store) as Arc<dyn SecretStore>).await;

        // `never_built` panics, so a job that builds an adapter anyway fails this test twice over.
        let (handle, journal_id) = fixture.start(Action::Verify, None, never_built).await;
        let completion = handle.wait().await;

        assert!(
            matches!(completion.outcome(), JobOutcome::Failed(_)),
            "{:?}",
            completion.outcome()
        );
        let row = fixture.row(journal_id).await;
        assert_eq!(
            row.last_error_code.as_deref(),
            Some(PROVIDER_NOT_CONFIGURED),
            "{row:?}"
        );
        assert_eq!(
            store.reads(),
            1,
            "one read established that nothing is stored"
        );

        fixture.scheduler.shutdown().await;
    }

    /// Forget removes what is stored, and succeeding when there was nothing is what the store's own
    /// contract says it should be.
    #[tokio::test]
    async fn forgetting_removes_what_is_stored_and_succeeds_when_nothing_is() {
        let store = Arc::new(MemorySecretStore::with_provider("tmdb", "a-key"));
        let fixture = Fixture::new(Arc::clone(&store) as Arc<dyn SecretStore>).await;

        let (handle, journal_id) = fixture.start(Action::Forget, None, never_built).await;
        let completion = handle.wait().await;
        assert!(
            matches!(completion.outcome(), JobOutcome::Succeeded(_)),
            "{:?}",
            completion.outcome()
        );
        assert_eq!(fixture.row(journal_id).await.state, JobPhase::Succeeded);
        assert!(
            store
                .get(&SecretKey::provider("tmdb"))
                .expect("the store answers")
                .is_none()
        );
        assert!(
            fixture.registry.status().configured.is_empty(),
            "the removal must reach the registry's cache"
        );

        // Already disconnected: the same request again is still a success.
        let (handle, journal_id) = fixture.start(Action::Forget, None, never_built).await;
        let completion = handle.wait().await;
        assert!(
            matches!(completion.outcome(), JobOutcome::Succeeded(_)),
            "{:?}",
            completion.outcome()
        );
        let row = fixture.row(journal_id).await;
        assert_eq!(row.state, JobPhase::Succeeded);
        assert_eq!(row.last_error_code, None);

        fixture.scheduler.shutdown().await;
    }

    /// The credential is in the row's fields nowhere and in nothing the database wrote.
    #[tokio::test]
    async fn the_credential_never_reaches_the_journal() {
        const KEY: &str = "super-secret-key";
        let store = Arc::new(MemorySecretStore::new());
        let fixture = Fixture::new(Arc::clone(&store) as Arc<dyn SecretStore>).await;

        let (handle, journal_id) = fixture
            .start(Action::Replace, Some(Secret::new(KEY)), accepting)
            .await;
        assert!(matches!(
            handle.wait().await.outcome(),
            JobOutcome::Succeeded(_)
        ));

        let row = fixture.row(journal_id).await;
        assert_eq!(row.last_error_code, None, "{row:?}");
        for field in [
            row.kind.as_str(),
            row.last_error.as_deref().unwrap_or_default(),
            row.last_error_code.as_deref().unwrap_or_default(),
            row.dedupe_key.as_deref().unwrap_or_default(),
        ] {
            assert!(!field.contains(KEY), "`{field}` carries the credential");
        }

        // `JobRecord` does not surface `payload_json`, so the strongest thing a test can do here is
        // read back every byte the database wrote: a credential in a file is a credential in a
        // backup and in a diagnostic bundle.
        for entry in std::fs::read_dir(fixture.directory.path()).unwrap() {
            let path = entry.unwrap().path();
            let bytes = std::fs::read(&path).unwrap();
            assert!(
                !contains(&bytes, KEY),
                "{} carries the credential",
                path.display()
            );
        }

        fixture.scheduler.shutdown().await;
    }

    /// The journal payload is the one function that decides what the row holds, and it cannot take
    /// a credential.
    #[test]
    fn the_journal_payload_names_only_the_provider() {
        assert_eq!(payload("tmdb"), r#"{"provider":"tmdb"}"#);
    }

    /// Every keyring call happens on a blocking thread, never on the one the job runs on.
    ///
    /// A read blocks, and on macOS the first one from a new binary raises an OS prompt that
    /// measured 11.7 s (PG-E5-12): made inline, it would hold a worker slot and stall the queue
    /// behind it. The job's own thread is the test's, because these tests run on a current-thread
    /// runtime, so a call the job made itself would name it.
    #[tokio::test]
    async fn every_keyring_call_is_off_the_async_runtime() {
        let store = Arc::new(CountingSecretStore::new());
        let fixture = Fixture::new(Arc::clone(&store) as Arc<dyn SecretStore>).await;

        let (handle, _journal_id) = fixture
            .start(Action::Replace, Some(Secret::new("a-key")), accepting)
            .await;
        assert!(matches!(
            handle.wait().await.outcome(),
            JobOutcome::Succeeded(_)
        ));

        let caller = store.caller().expect("the job used the store");
        assert_ne!(
            caller,
            std::thread::current().id(),
            "a keyring call on the job's own thread would block the runtime"
        );

        fixture.scheduler.shutdown().await;
    }

    /// A keyring that is unavailable is a store failure, not a credential refusal.
    #[tokio::test]
    async fn an_unavailable_keyring_is_a_store_failure() {
        let store = Arc::new(CountingSecretStore::unavailable());
        let fixture = Fixture::new(Arc::clone(&store) as Arc<dyn SecretStore>).await;

        let (handle, journal_id) = fixture.start(Action::Verify, None, never_built).await;
        assert!(matches!(
            handle.wait().await.outcome(),
            JobOutcome::Failed(_)
        ));
        let row = fixture.row(journal_id).await;
        assert_eq!(
            row.last_error_code.as_deref(),
            Some(PROVIDER_STORE),
            "{row:?}"
        );
        assert_eq!(
            row.last_error.as_deref(),
            Some(SecretStoreError::Unavailable.to_string().as_str()),
            "the message is the store's own, which carries no detail to leak"
        );
        assert!(!store.holds("tmdb"));

        fixture.scheduler.shutdown().await;
    }

    /// A keyring that refuses the write reports that rather than claiming success.
    #[tokio::test]
    async fn a_keyring_that_cannot_be_written_is_reported() {
        let store = Arc::new(CountingSecretStore::unavailable());
        let fixture = Fixture::new(Arc::clone(&store) as Arc<dyn SecretStore>).await;

        let (handle, journal_id) = fixture
            .start(Action::Replace, Some(Secret::new("a-key")), accepting)
            .await;
        let completion = handle.wait().await;

        assert!(
            matches!(completion.outcome(), JobOutcome::Failed(_)),
            "{:?}",
            completion.outcome()
        );
        let row = fixture.row(journal_id).await;
        assert_eq!(
            row.last_error_code.as_deref(),
            Some(PROVIDER_STORE),
            "{row:?}"
        );
        assert_eq!(store.writes(), 1, "the write was attempted and reported");
        assert!(!store.holds("tmdb"));

        fixture.scheduler.shutdown().await;
    }

    /// Cancelling while the probe is in flight must not store the candidate.
    ///
    /// The timing is not something a test can pin, so the assertion is conditional on the terminal
    /// state — which is the rule a cancel is held to: a `Cancelled` outcome and a written key
    /// cannot both be true.
    #[tokio::test]
    async fn a_cancelled_replace_never_stores_the_candidate() {
        let store = Arc::new(CountingSecretStore::new());
        let fixture = Fixture::new(Arc::clone(&store) as Arc<dyn SecretStore>).await;

        let (mut handle, journal_id) = fixture
            .start(Action::Replace, Some(Secret::new("a-key")), slow)
            .await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.cancel();
        let completion = handle.wait().await;

        match completion.outcome() {
            JobOutcome::Cancelled => {
                let row = fixture.row(journal_id).await;
                assert_eq!(row.state, JobPhase::Canceled, "{row:?}");
                assert_eq!(row.last_error_code.as_deref(), Some(CANCELLED_CODE));
                assert_eq!(
                    store.writes(),
                    0,
                    "a cancellation that lands during the probe must not store the key"
                );
                assert!(!store.holds("tmdb"));
            }
            JobOutcome::Succeeded(_) => assert_eq!(
                store.writes(),
                1,
                "the probe finished before the cancel landed, so the key was stored"
            ),
            other @ JobOutcome::Failed(_) => panic!("unexpected outcome: {other:?}"),
        }

        fixture.scheduler.shutdown().await;
    }
}
