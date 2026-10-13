//! Filling one title from its provider, and fetching the artwork that comes with it.
//!
//! A job rather than a command because it leaves the machine twice — once for the title's details, once
//! for its poster — and `ARCHITECTURE §7` rule 2 says anything that can take longer than a command's
//! budget is a job with progress. It is also the first job whose *purpose* is one screen's content
//! rather than the library as a whole, which is why it carries a title id in its payload.
//!
//! **Two phases, and the second is not a failure when it does nothing.** A title with no artwork path is
//! a title whose provider had none, which is a fact about that provider rather than an error — so the
//! job succeeds with what it has. A poster that cannot be written *is* reported, because a title whose
//! metadata arrived and whose artwork silently did not is the state that looks like a rendering bug.

use std::sync::Arc;

use peregrine_domain::Clock;
use peregrine_domain::catalogue::{ImageKind, TitleKind};
use peregrine_storage::{Database, JobChange, JobPhase};
use peregrine_sync::{Job, JobContext, JobError, JobFuture, JobOutput};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

use crate::events::{Event, EventBus, JOB_PROGRESS, JOB_STATE};
use crate::providers::ProviderRegistry;

/// The kind this job registers under, in the scheduler and in the journal.
pub const KIND: &str = "provider.refresh_title";

/// The default priority: a screen is waiting for it, which is ahead of background maintenance and
/// behind the work a user just asked for.
pub const PRIORITY: i32 = 10;

/// Which title a refresh is for.
///
/// One value rather than two arguments because the pair is what a title *is* to this job, and because
/// eight arguments is where a constructor stops being readable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    /// The catalogue row.
    pub title_id: i64,
    /// A film or a series, which decides the endpoint the provider is asked at.
    pub kind: TitleKind,
}

/// Fills one title from one provider.
pub struct ProviderRefreshJob {
    database: Arc<Database>,
    bus: Arc<EventBus>,
    clock: Arc<dyn Clock + Send + Sync>,
    providers: Arc<ProviderRegistry>,
    artwork: Arc<peregrine_sync::ImageCache>,
    journal_id: i64,
    target: Target,
}

impl ProviderRefreshJob {
    /// A job that refreshes `title_id`.
    #[must_use]
    pub fn new(
        database: Arc<Database>,
        bus: Arc<EventBus>,
        clock: Arc<dyn Clock + Send + Sync>,
        providers: Arc<ProviderRegistry>,
        artwork: Arc<peregrine_sync::ImageCache>,
        journal_id: i64,
        target: Target,
    ) -> Self {
        Self {
            database,
            bus,
            clock,
            providers,
            artwork,
            journal_id,
            target,
        }
    }

    /// Reports which half is running, so a screen can say so.
    async fn phase(&self, job: ulid::Ulid, attempt: u32, permille: u16) {
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
            crate::jobs::progress(job, self.journal_id, permille, attempt),
        ));
    }
}

impl Job for ProviderRefreshJob {
    fn run(&mut self, context: &JobContext<'_>) -> JobFuture<'_> {
        // Copied out of the context before the future is built, because a future that held the context
        // borrow would have to live as long as this job does — the same shape the library scan uses.
        let job = context.id();
        let attempt = context.attempt();
        let cancellation = context.cancellation().clone();
        Box::pin(self.refresh(job, attempt, cancellation))
    }
}

impl ProviderRefreshJob {
    /// The work, owning everything it borrows so the returned future borrows only `self`.
    async fn refresh(
        &mut self,
        job: ulid::Ulid,
        attempt: u32,
        cancellation: CancellationToken,
    ) -> peregrine_sync::JobResult {
        // One provider is enough: a title carries its own external ids, and `refresh_title` asks the one
        // that owns one of them.
        let Some(provider) = self.providers.adapters().into_iter().next() else {
            return Err(JobError::failed("no provider is configured"));
        };

        self.phase(job, attempt, 100).await;
        let at = self.clock.now().as_millisecond();
        let details = peregrine_sync::refresh_title(
            provider.as_ref(),
            &self.database,
            self.target.title_id,
            self.target.kind,
            at,
        )
        .await
        .map_err(|error| JobError::failed(error.to_string()))?;
        let Some(details) = details else {
            return Ok(Box::new(None::<std::path::PathBuf>) as JobOutput);
        };

        if cancellation.is_cancelled() {
            return Err(JobError::Cancelled);
        }

        self.phase(job, attempt, 600).await;
        let poster = match details.poster_path.as_deref() {
            None => None,
            Some(path) => Some(
                peregrine_sync::cache_image(
                    provider.as_ref(),
                    &self.artwork,
                    path,
                    ImageKind::Poster,
                )
                .await
                .map_err(|error| JobError::failed(error.to_string()))?,
            ),
        };

        if let Err(error) = self
            .database
            .update_job(self.journal_id, JobChange::Progress { permille: 1_000 })
            .await
        {
            debug!(%error, "job progress could not be journalled");
        }
        self.bus.publish(Event::new(
            JOB_STATE,
            job.to_string(),
            crate::jobs::state(job, self.journal_id, KIND, JobPhase::Succeeded, attempt),
        ));
        info!(
            title = self.target.title_id,
            "a title was refreshed from its provider"
        );
        Ok(Box::new(poster) as JobOutput)
    }
}

/// The dedupe key for one title, so two screens opening the same one wait on one refresh.
#[must_use]
pub fn dedupe_key(title_id: i64) -> String {
    format!("{KIND}:{title_id}")
}
