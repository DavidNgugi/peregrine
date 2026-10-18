//! Filling one title from its provider, and fetching the artwork that comes with it.
//!
//! A job rather than a command because it leaves the machine more than once — the title's details, a
//! series' season list, one request per season, and one image request per picture — and
//! `ARCHITECTURE §7` rule 2 says anything that can take longer than a command's budget is a job with
//! progress. It is also the first job whose *purpose* is one screen's content rather than the library
//! as a whole, which is why it carries a title id in its payload.
//!
//! **Two phases, and the second is not a failure when it does nothing.** A title with no artwork path is
//! a title whose provider had none, which is a fact about that provider rather than an error — so the
//! job succeeds with what it has. A poster that cannot be written *is* reported, because a title whose
//! metadata arrived and whose artwork silently did not is the state that looks like a rendering bug.
//! The season posters and episode stills are the exception, and the reason is in `refresh`: they are
//! decoration on a card whose metadata has already been written, and there are a dozen of them.

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
        let adapters = self.providers.adapters();
        if adapters.is_empty() {
            return Err(JobError::failed("no provider is configured"));
        }

        self.phase(job, attempt, 100).await;
        let at = self.clock.now().as_millisecond();
        // **The provider that owns one of this title's identifiers, not simply the first one
        // configured.** A title carries ids for several providers, and asking one for another's
        // identifier is what `refresh_title` refuses. A provider with no id for the title answers
        // `None` without a request, so walking the list costs one read each and no network.
        let mut answered = None;
        for adapter in &adapters {
            match peregrine_sync::refresh_title(
                adapter.as_ref(),
                &self.database,
                self.target.title_id,
                self.target.kind,
                at,
            )
            .await
            {
                Ok(Some(details)) => {
                    answered = Some((Arc::clone(adapter), details));
                    break;
                }
                Ok(None) => {}
                Err(error) => return Err(JobError::failed(error.to_string())),
            }
        }

        // **A scanned title has no id yet, so it is searched for before it is given up on.** This is
        // the difference between a library that fills itself in and one whose titles stay stubs until
        // somebody happens to search for them by hand. The matcher only attaches an id when the
        // provider names the same title and kind, so a near miss stays a stub rather than becoming
        // another film.
        if answered.is_none() {
            for adapter in &adapters {
                match peregrine_sync::match_title(
                    adapter.as_ref(),
                    &self.database,
                    self.target.title_id,
                    self.target.kind,
                    at,
                )
                .await
                {
                    Ok(Some(details)) => {
                        answered = Some((Arc::clone(adapter), details));
                        break;
                    }
                    Ok(None) => {}
                    Err(error) => return Err(JobError::failed(error.to_string())),
                }
            }
        }

        // **A title neither the catalogue nor a match can place is a success, not a failure.** No
        // provider named it, so there is nothing to fill in and nothing to fetch. A failure here
        // would put a red row in the journal for a library that is simply not matched yet.
        let Some((provider, refreshed)) = answered else {
            return self.succeeded(job, attempt, None).await;
        };

        if cancellation.is_cancelled() {
            return Err(JobError::Cancelled);
        }

        // The cast is best effort: a title whose credits request fails still gets its poster and
        // metadata, and a missing cast is not a failed refresh.
        self.store_cast(provider.as_ref(), at).await;

        self.phase(job, attempt, 600).await;
        let poster = match refreshed.details.poster_path.as_deref() {
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

        // **The season posters and episode stills are best effort, and the title's own poster is not.**
        // The metadata write has already happened, so a picture that cannot be fetched leaves a card
        // drawing its placeholder until the next refresh — where failing the job would put a red row in
        // the journal for a title whose name, synopsis, air dates, and numbers all arrived. A season
        // list is a dozen pictures at the image host's own rate, so one transient failure among them is
        // the normal case rather than the exceptional one.
        self.phase(job, attempt, 800).await;
        for (kind, path) in &refreshed.artwork {
            if cancellation.is_cancelled() {
                return Err(JobError::Cancelled);
            }
            if let Err(error) =
                peregrine_sync::cache_image(provider.as_ref(), &self.artwork, path, *kind).await
            {
                debug!(%error, path, "a season or episode picture could not be fetched");
            }
        }

        self.succeeded(job, attempt, poster).await
    }

    /// Records that the job succeeded and publishes the phase, which is the last thing every
    /// successful path does.
    ///
    /// **The journal write is what stops the row saying `running`.** `JobChange::Finished` does not
    /// touch `progress_permille`, so the progress write comes first: a row left at the last reported
    /// fraction would show a finished job as three-fifths done. A journal failure is logged and does
    /// not fail the job — refusing to do the work because the record of it could not be written would
    /// turn a full disk into a stuck queue — which is the rule `hello` and `library_scan` follow.
    ///
    /// The state event is what a screen following this job waits for, and *every* success publishes
    /// it: a title with nothing to fill in is the common case for a fresh library, and a job that
    /// completed without saying so is a screen waiting for a stop that cannot arrive.
    /// Stores the cast of the title this job is for, when the provider that answered owns its id.
    ///
    /// The portrait cache is the same one the poster uses, so a person's photograph is fetched once
    /// and evicted under the same LRU. A credits request that fails leaves the title without a cast
    /// rather than failing a refresh whose metadata and poster already landed.
    async fn store_cast(
        &self,
        provider: &dyn peregrine_providers::provider::MetadataProvider,
        at: i64,
    ) {
        let held = self
            .database
            .external_ids_of(self.target.title_id)
            .await
            .unwrap_or_default();
        let external = held
            .into_iter()
            .map(|held| held.id)
            .find(|id| id.provider() == provider.descriptor().provider());
        let Some(external) = external else {
            return;
        };
        if let Err(error) = peregrine_sync::store_cast(
            provider,
            &self.database,
            &self.artwork,
            self.target.title_id,
            &external,
            self.target.kind,
            at,
        )
        .await
        {
            tracing::warn!(%error, "the cast could not be stored");
        }
    }

    async fn succeeded(
        &self,
        job: ulid::Ulid,
        attempt: u32,
        poster: Option<std::path::PathBuf>,
    ) -> peregrine_sync::JobResult {
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
