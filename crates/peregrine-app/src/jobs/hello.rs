//! The hello job: the Phase 0 walking skeleton's work item.
//!
//! `PG-E0-19` asks for a job that is visible end to end — accepted by the scheduler,
//! journaled in `SQLite`, reported over the event bus at the IPC budget's rate, and
//! cancellable from the UI. It is deliberately not useful work: what it proves is that
//! the scheduler, the storage writer, the event bus, and the log pipeline are connected
//! and observable, which is what the rest of Phase 0 builds on.
//!
//! Two rates are visible here and they are different on purpose. The job steps every
//! [`STEP`] and publishes progress each time; the bus coalesces those to at most 4 Hz; and
//! the journal is written every [`JOURNAL_INTERVAL`] because a row per 10 ms step would
//! be 100 writes a second of rows nobody reads.

use std::sync::Arc;
use std::time::Duration;

use peregrine_domain::Clock;
use peregrine_storage::{Database, JobChange, JobPhase};
use peregrine_sync::{Job, JobContext, JobError, JobFuture, JobOutput};
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use ulid::Ulid;

use crate::events::{Event, EventBus, JOB_PROGRESS, JOB_STATE};
use crate::jobs::{self, CANCELLED_CODE};

/// The kind this job registers under, in the scheduler and in the journal.
pub const KIND: &str = "hello";

/// Scheduling priority: the schema's default, so the journal and the queue agree.
pub const PRIORITY: i32 = 100;

/// The fewest steps a caller may ask for.
pub const MIN_STEPS: u32 = 1;

/// The most steps a caller may ask for.
///
/// At [`STEP`] this is six seconds of work: long enough for a person to see a progress
/// bar and press cancel, short enough that a stuck job is not the app's problem.
pub const MAX_STEPS: u32 = 600;

/// How long one step takes.
const STEP: Duration = Duration::from_millis(10);

/// How often progress reaches the journal, matching the IPC budget in ARCHITECTURE §14.
const JOURNAL_INTERVAL: Duration = Duration::from_millis(250);

/// A job that counts to `steps`, reporting as it goes.
pub struct HelloJob {
    database: Arc<Database>,
    bus: Arc<EventBus>,
    clock: Arc<dyn Clock + Send + Sync>,
    /// The journal row this job reports into.
    journal_id: i64,
    steps: u32,
    /// Whether the running attempt has already been reported to the journal.
    ///
    /// A retry runs [`HelloJob::attempt`] again on the same value, and the second attempt
    /// must record a new row state rather than silently reusing the first one's.
    reported_start: bool,
}

impl HelloJob {
    /// Builds the job for the journal row `journal_id`, which the caller created.
    #[must_use]
    pub fn new(
        database: Arc<Database>,
        bus: Arc<EventBus>,
        clock: Arc<dyn Clock + Send + Sync>,
        journal_id: i64,
        steps: u32,
    ) -> Self {
        Self {
            database,
            bus,
            clock,
            journal_id,
            steps,
            reported_start: false,
        }
    }

    /// The steps this job will take.
    #[must_use]
    pub const fn steps(&self) -> u32 {
        self.steps
    }

    /// Runs one attempt.
    ///
    /// The values a job needs from its context are taken before the attempt future is
    /// created: `JobContext` borrows the scheduler's attempt, so a future that held the
    /// borrow could not be boxed with the lifetime `Job::run` promises.
    async fn attempt(
        &mut self,
        job: Ulid,
        attempt: u32,
        cancellation: CancellationToken,
    ) -> Result<JobOutput, JobError> {
        if !self.reported_start {
            self.reported_start = true;
            self.record(
                job,
                JobChange::Started {
                    at: self.clock.now().as_millisecond(),
                    attempt: i64::from(attempt),
                },
            )
            .await;
            self.publish_state(job, JobPhase::Running, attempt);
            info!(job = %job, kind = KIND, steps = self.steps, "the hello job started");
        }

        let mut ticker = tokio::time::interval(STEP);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        // The first tick of a Tokio interval is immediate; consuming it here is what makes
        // step one cost one `STEP` rather than nothing.
        ticker.tick().await;

        let mut last_journal_write = tokio::time::Instant::now();
        for step in 1..=self.steps {
            // `biased` so a cancellation that is already requested wins over a step that
            // is due in the same poll, which is what makes "cancel during a step" stop
            // before the work rather than after it.
            tokio::select! {
                biased;
                () = cancellation.cancelled() => {
                    self.finish_cancelled(job, attempt).await;
                    return Err(JobError::Cancelled);
                }
                _ = ticker.tick() => {}
            }

            let permille = permille_after(step, self.steps);
            self.bus.publish(Event::new(
                JOB_PROGRESS,
                job.to_string(),
                jobs::progress(job, self.journal_id, permille, attempt),
            ));

            if step == self.steps || last_journal_write.elapsed() >= JOURNAL_INTERVAL {
                self.record(
                    job,
                    JobChange::Progress {
                        permille: i64::from(permille),
                    },
                )
                .await;
                last_journal_write = tokio::time::Instant::now();
            }
        }

        self.record(
            job,
            JobChange::Finished {
                at: self.clock.now().as_millisecond(),
                phase: JobPhase::Succeeded,
                error_code: None,
                error: None,
            },
        )
        .await;
        self.publish_state(job, JobPhase::Succeeded, attempt);
        info!(job = %job, kind = KIND, steps = self.steps, "the hello job finished");
        Ok(Box::new(()))
    }

    /// Records that the job stopped because it was asked to.
    async fn finish_cancelled(&self, job: Ulid, attempt: u32) {
        self.record(
            job,
            JobChange::Finished {
                at: self.clock.now().as_millisecond(),
                phase: JobPhase::Canceled,
                error_code: Some(CANCELLED_CODE.to_owned()),
                error: None,
            },
        )
        .await;
        self.publish_state(job, JobPhase::Canceled, attempt);
        info!(job = %job, kind = KIND, "the hello job stopped on request");
    }

    /// Applies one change to this job's journal row.
    ///
    /// A journal failure is logged and does not fail the job: the row records the work,
    /// and refusing to do the work because the record could not be written would turn a
    /// full disk into a stuck queue. A row that is already terminal is the ordinary race
    /// between a cancellation and a step, so it is logged at debug volume rather than
    /// treated as a fault.
    async fn record(&self, job: Ulid, change: JobChange) {
        match self.database.update_job(self.journal_id, change).await {
            Ok(true) => {}
            Ok(false) => {
                warn!(
                    job = %job,
                    journal_id = self.journal_id,
                    "the journal row is already terminal; the change was dropped"
                );
            }
            Err(error) => {
                warn!(
                    job = %job,
                    journal_id = self.journal_id,
                    %error,
                    "the journal could not be updated"
                );
            }
        }
    }

    /// Publishes one lifecycle phase.
    fn publish_state(&self, job: Ulid, phase: JobPhase, attempt: u32) {
        self.bus.publish(Event::new(
            JOB_STATE,
            job.to_string(),
            jobs::state(job, self.journal_id, KIND, phase, attempt),
        ));
    }
}

impl Job for HelloJob {
    fn run(&mut self, context: &JobContext<'_>) -> JobFuture<'_> {
        let job = context.id();
        let attempt = context.attempt();
        let cancellation = context.cancellation().clone();
        Box::pin(self.attempt(job, attempt, cancellation))
    }
}

/// Completion in permille, clamped so the last step always reports exactly 1000.
///
/// Integer division truncates, so a job whose steps do not divide evenly into 1000 would
/// otherwise end at 999 and a progress bar would stop one pixel short of full.
fn permille_after(step: u32, steps: u32) -> u16 {
    if step >= steps {
        return 1000;
    }
    u16::try_from(u64::from(step) * 1000 / u64::from(steps)).unwrap_or(1000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventSink;
    use crate::testing::RecordingSink;
    use peregrine_storage::{JobRecord, NewJob};
    use peregrine_sync::{JobHandle, JobOutcome, Scheduler, SchedulerConfig};
    use peregrine_testkit::{CountingIds, FakeClock};
    use serde_json::Value;
    use tempfile::TempDir;

    const NOW: i64 = 1_700_000_000_000;

    /// A clock pinned to [`NOW`], so every journal timestamp is predictable.
    fn clock() -> Arc<dyn Clock + Send + Sync> {
        Arc::new(FakeClock::at(NOW))
    }

    async fn database(directory: &TempDir) -> Arc<Database> {
        Arc::new(
            Database::open(directory.path().join("peregrine.sqlite"))
                .await
                .unwrap(),
        )
    }

    async fn journal_row(database: &Database, id: i64) -> JobRecord {
        database.job(id).await.unwrap().expect("the row exists")
    }

    async fn submit(
        database: &Arc<Database>,
        bus: &Arc<EventBus>,
        steps: u32,
    ) -> (Scheduler, JobHandle, i64) {
        let journal_id = database
            .insert_job(NewJob {
                kind: KIND.to_owned(),
                priority: PRIORITY,
                dedupe_key: None,
                payload_json: format!("{{\"steps\":{steps}}}"),
                max_attempts: 1,
                created_at: NOW,
            })
            .await
            .unwrap();
        let scheduler = Scheduler::new(SchedulerConfig::new(
            clock(),
            Arc::new(CountingIds::default()),
        ));
        let job = HelloJob::new(
            Arc::clone(database),
            Arc::clone(bus),
            clock(),
            journal_id,
            steps,
        );
        let handle = scheduler
            .submit(
                peregrine_sync::JobSpec {
                    kind: KIND.to_owned(),
                    priority: PRIORITY,
                    dedupe_key: None,
                    max_attempts: 1,
                },
                job,
            )
            .await
            .unwrap();
        (scheduler, handle, journal_id)
    }

    /// The whole point of the job: it runs, the journal says so, and the bus carries the
    /// final progress value and the terminal phase.
    ///
    /// These tests run on the real clock rather than a paused one: `sqlx`'s pool acquire
    /// timeout is a timer, so a paused clock makes opening a connection never finish.
    #[tokio::test]
    async fn a_completed_job_journals_success_and_publishes_its_progress() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        let sink = Arc::new(RecordingSink::default());
        let bus = Arc::new(EventBus::new(Arc::clone(&sink) as Arc<dyn EventSink>));
        let (scheduler, handle, journal_id) = submit(&database, &bus, 4).await;

        let completion = handle.wait().await;
        assert!(
            matches!(completion.outcome(), JobOutcome::Succeeded(_)),
            "the job must succeed: {:?}",
            completion.outcome()
        );

        // The final flush on shutdown is what delivers the coalesced last value.
        bus.shutdown().await;
        let delivered = sink.delivered();
        let progress = delivered
            .iter()
            .filter(|event| event.topic == JOB_PROGRESS)
            .collect::<Vec<_>>();
        assert_eq!(
            progress.len(),
            1,
            "four steps in 40 ms coalesce to one delivery"
        );
        assert_eq!(progress[0].payload["progressPermille"], 1000);
        let stats = bus.stats();
        assert_eq!(
            stats.published, 6,
            "four progress updates and two lifecycle phases"
        );
        assert_eq!(
            stats.coalesced, 4,
            "each key delivered once, so four updates were superseded"
        );
        assert_eq!(stats.delivered, 2);
        assert_eq!(stats.dropped, 0);

        // The lifecycle is coalesced like any other stream, so a job that finishes inside
        // one tick delivers its terminal phase and not the running one it passed through.
        // That is the design: the journal is the record, and the event is the live view.
        let phases: Vec<&Value> = delivered
            .iter()
            .filter(|event| event.topic == JOB_STATE)
            .map(|event| &event.payload)
            .collect();
        assert_eq!(
            phases.len(),
            1,
            "only the latest phase survives: {phases:#?}"
        );
        assert_eq!(phases[0]["phase"], "succeeded");
        assert_eq!(phases[0]["journalId"], journal_id);
        assert_eq!(phases[0]["kind"], KIND);

        let row = journal_row(&database, journal_id).await;
        assert_eq!(row.state, JobPhase::Succeeded);
        assert_eq!(row.progress_permille, 1000);
        assert_eq!(row.attempts, 1);
        assert_eq!(row.started_at, Some(NOW));
        assert_eq!(row.finished_at, Some(NOW));
        assert_eq!(row.last_error_code, None);

        scheduler.shutdown().await;
    }

    /// Cancellation is cooperative, and the journal records the outcome rather than the
    /// request alone.
    #[tokio::test]
    async fn a_cancelled_job_journals_the_cancellation() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        let sink = Arc::new(RecordingSink::default());
        let bus = Arc::new(EventBus::new(Arc::clone(&sink) as Arc<dyn EventSink>));
        let (scheduler, mut handle, journal_id) = submit(&database, &bus, MAX_STEPS).await;

        // Let a few steps run, then stop it. The job is 600 steps, so this leaves it
        // nowhere near complete and the cancellation is the only way it can end.
        tokio::time::sleep(STEP * 3).await;
        handle.cancel();
        let completion = handle.wait().await;

        assert!(
            matches!(completion.outcome(), JobOutcome::Cancelled),
            "a cancelled job must resolve as cancelled: {:?}",
            completion.outcome()
        );

        bus.shutdown().await;
        let row = journal_row(&database, journal_id).await;
        assert_eq!(row.state, JobPhase::Canceled);
        assert_eq!(row.last_error_code.as_deref(), Some(CANCELLED_CODE));
        assert!(
            row.progress_permille < 1000,
            "a cancelled job must not claim to have finished: {row:?}"
        );
        let last_state = sink
            .delivered()
            .iter()
            .rev()
            .find(|event| event.topic == JOB_STATE)
            .map(|event| event.payload["phase"].clone());
        assert_eq!(last_state, Some(Value::from("canceled")));

        scheduler.shutdown().await;
    }

    /// The boundary at both ends of the step count, and the clamping that keeps the last
    /// step at exactly full.
    #[test]
    fn permille_reaches_exactly_one_thousand_at_the_last_step() {
        assert_eq!(permille_after(1, 1), 1000);
        assert_eq!(permille_after(1, 3), 333);
        assert_eq!(permille_after(2, 3), 666);
        assert_eq!(permille_after(3, 3), 1000);
        assert_eq!(permille_after(1, 600), 1);
        assert_eq!(permille_after(599, 600), 998);
        assert_eq!(permille_after(600, 600), 1000);
    }

    /// The single-step case, which is where an off-by-one in the loop would show.
    #[tokio::test]
    async fn a_one_step_job_completes_immediately() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        let sink = Arc::new(RecordingSink::default());
        let bus = Arc::new(EventBus::new(Arc::clone(&sink) as Arc<dyn EventSink>));
        let (scheduler, handle, journal_id) = submit(&database, &bus, MIN_STEPS).await;

        let completion = handle.wait().await;
        assert!(matches!(completion.outcome(), JobOutcome::Succeeded(_)));
        assert_eq!(completion.attempts(), 1);
        assert_eq!(
            journal_row(&database, journal_id).await.state,
            JobPhase::Succeeded
        );

        scheduler.shutdown().await;
    }
}
