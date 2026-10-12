//! The scheduler's acceptance tests.
//!
//! Each test names the acceptance it proves from `PG-E6-01` or the `PLAN.md` Phase 0
//! Scheduler row: priority order, dedupe, queued cancellation, running cancellation, the
//! injected panic that must release its slot, the retry schedule, and the empty queue.

#![allow(clippy::unwrap_used)]

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

use peregrine_domain::Clock;
use peregrine_testkit::{CountingIds, FakeClock};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use crate::backoff::Backoff;
use crate::core::{Scheduler, SchedulerConfig, Submission};
use crate::error::JobError;
use crate::heap::JobSpec;
use crate::job::{
    Job, JobCompletion, JobContext, JobFuture, JobHandle, JobOutcome, JobOutput, JobState,
    JobStateKind,
};

/// A pinned clock, so a deferral's recorded time is predictable.
fn clock() -> Arc<dyn Clock + Send + Sync> {
    Arc::new(FakeClock::default())
}

/// A scheduler with deterministic identifiers, a frozen clock, and one worker.
///
/// One worker is what makes "which job runs next" observable: with several, two ready
/// jobs both start and their order stops being a property of the queue.
fn scheduler_with_one_worker() -> Scheduler {
    Scheduler::new(
        SchedulerConfig::new(clock(), Arc::new(CountingIds::default()))
            .with_max_concurrency(NonZeroUsize::MIN),
    )
}

fn job<J: Job>(kind: &str, priority: i32, dedupe_key: Option<&str>, work: J) -> Submission {
    Submission::new(spec(kind, priority, dedupe_key), work)
}

fn spec(kind: &str, priority: i32, dedupe_key: Option<&str>) -> JobSpec {
    JobSpec {
        kind: kind.to_owned(),
        priority,
        dedupe_key: dedupe_key.map(str::to_owned),
        max_attempts: crate::DEFAULT_MAX_ATTEMPTS,
    }
}

/// A job that reports when it starts and then waits to be released.
///
/// The release channel is what makes ordering deterministic: nothing finishes until the
/// test says so, so the order in which starts arrive *is* the queue's order.
struct Blocking {
    started: mpsc::UnboundedSender<usize>,
    released: Option<oneshot::Receiver<()>>,
    /// Held open by a job that should outlive the test's assertions. It is never sent on:
    /// its only purpose is to keep the job's own task alive so its slot stays occupied.
    parked: Option<oneshot::Sender<()>>,
    label: usize,
}

impl Blocking {
    fn new(
        started: &mpsc::UnboundedSender<usize>,
        label: usize,
        released: oneshot::Receiver<()>,
    ) -> Self {
        Self {
            started: started.clone(),
            released: Some(released),
            parked: None,
            label,
        }
    }

    /// A job that reports its start and then runs until the test drops its guard.
    ///
    /// `oneshot::Receiver::close` is the release, so the guard the caller keeps decides
    /// when the job ends. That makes "this job is still running" a fact the test controls
    /// instead of a race against how fast the job finishes.
    fn gated(
        started: &mpsc::UnboundedSender<usize>,
        label: usize,
    ) -> (Self, oneshot::Receiver<()>) {
        let (parked, guard) = oneshot::channel();
        (
            Self {
                started: started.clone(),
                released: None,
                parked: Some(parked),
                label,
            },
            guard,
        )
    }
}

impl Job for Blocking {
    fn run(&mut self, _context: &JobContext<'_>) -> JobFuture<'_> {
        let started = self.started.clone();
        let label = self.label;
        let released = self.released.take();
        let parked = self.parked.take();
        Box::pin(async move {
            started.send(label).expect("the test still listens");
            if let Some(released) = released {
                let _ = released.await;
            }
            if let Some(mut parked) = parked {
                // The guard the test holds is the receiver; dropping it ends this job, and
                // until then this is what a long-running job looks like.
                parked.closed().await;
            }
            Ok(Box::new(()) as JobOutput)
        })
    }
}

/// A job that does nothing at all.
struct Noop;

impl Job for Noop {
    fn run(&mut self, _context: &JobContext<'_>) -> JobFuture<'_> {
        Box::pin(async { Ok(Box::new(()) as JobOutput) })
    }
}

/// A job that panics on every attempt.
///
/// The panic is injected here rather than in production code: `PLAN.md` names an
/// "injected-panic test that proves token release", and a test that cannot introduce the
/// panic cannot prove anything about it.
struct Panics;

impl Job for Panics {
    fn run(&mut self, _context: &JobContext<'_>) -> JobFuture<'_> {
        Box::pin(async {
            panic!("injected panic: PG-E6-01 requires the queue to survive this");
        })
    }
}

/// A job that reports that it is running, then waits for its token.
struct WaitsForCancellation {
    running: Option<oneshot::Sender<()>>,
}

impl WaitsForCancellation {
    fn new(running: oneshot::Sender<()>) -> Self {
        Self {
            running: Some(running),
        }
    }
}

impl Job for WaitsForCancellation {
    fn run(&mut self, context: &JobContext<'_>) -> JobFuture<'_> {
        let running = self.running.take();
        let token = context.cancellation().clone();
        Box::pin(async move {
            if let Some(running) = running {
                let _ = running.send(());
            }
            token.cancelled().await;
            Err(JobError::Cancelled)
        })
    }
}

/// A job that defers until its attempt number reaches `succeed_on_attempt`.
struct DefersUntil {
    succeeded_on: u32,
}

impl Job for DefersUntil {
    fn run(&mut self, context: &JobContext<'_>) -> JobFuture<'_> {
        let attempt = context.attempt();
        let succeeded_on = self.succeeded_on;
        Box::pin(async move {
            if attempt >= succeeded_on {
                Ok(Box::new(attempt) as JobOutput)
            } else {
                Err(JobError::retryable("not yet"))
            }
        })
    }
}

/// A job that always defers, so its retry budget can be exhausted.
struct AlwaysDefers;

impl Job for AlwaysDefers {
    fn run(&mut self, _context: &JobContext<'_>) -> JobFuture<'_> {
        Box::pin(async { Err(JobError::retryable("still not yet")) })
    }
}

/// Waits until `predicate` holds, driving the runtime rather than sleeping.
///
/// Each turn yields to the engine and then arms a timer that has already expired. The
/// second half matters under a paused clock: yielding lets a ready task run but does not
/// itself advance time, and this is what gives the engine's own retry timer a chance to
/// expire at the instant the test stepped to.
async fn wait_until(mut predicate: impl FnMut() -> bool) -> bool {
    for _ in 0..512 {
        if predicate() {
            return true;
        }
        tokio::task::yield_now().await;
        let _ = tokio::time::timeout(Duration::from_micros(1), std::future::pending::<()>()).await;
    }
    predicate()
}

/// Waits until the handle reports `phase`, then returns the state it reached.
async fn wait_for_phase(handle: &mut JobHandle, phase: JobStateKind) -> JobState {
    for _ in 0..10_000 {
        let state = handle.state();
        if state.phase() == phase {
            return state;
        }
        if handle.changed().await.is_none() {
            break;
        }
    }
    panic!("the job never reached {phase:?}");
}

#[tokio::test]
async fn higher_priority_runs_first_and_equal_priority_is_fifo() {
    let scheduler = scheduler_with_one_worker();
    let (started, mut order) = mpsc::unbounded_channel();

    // All three go out as one batch and each holds its worker slot until its guard drops,
    // so the order the labels arrive in is exactly the queue's order: the engine sees the
    // whole batch before it starts anything, which is what makes priority win over arrival
    // time rather than over scheduling luck.
    let mut guards = Vec::new();
    let mut batch = Vec::new();
    for (priority, label) in [(1, 0_usize), (5, 1), (1, 2)] {
        let (work, guard) = Blocking::gated(&started, label);
        guards.push(guard);
        batch.push(job("test.ordering", priority, None, work));
    }
    scheduler.submit_all(batch).await.unwrap();
    // The guards go as soon as the batch is accepted, so each job finishes and the next
    // one gets the single worker slot.
    drop(guards);

    assert_eq!(order.recv().await, Some(1), "priority 5 before priority 1");
    assert_eq!(order.recv().await, Some(0), "FIFO within priority 1");
    assert_eq!(order.recv().await, Some(2), "FIFO within priority 1");
}

#[tokio::test]
async fn dedupe_coalesces_a_live_job_and_leaves_a_clear_key_alone() {
    let scheduler = scheduler_with_one_worker();
    let (release, released) = oneshot::channel();
    let (started, mut order) = mpsc::unbounded_channel();

    // Sent as one batch so the engine sees the duplicate while the first job is still
    // live, rather than being able to finish it between the two calls.
    let submitted = scheduler
        .submit_all(vec![
            job(
                "test.dedupe",
                1,
                Some("title:42"),
                Blocking::new(&started, 0, released),
            ),
            job("test.dedupe", 1, Some("title:42"), Noop),
            job("test.dedupe", 1, None, Noop),
            job("test.dedupe", 1, None, Noop),
        ])
        .await
        .unwrap();
    assert_eq!(
        submitted.len(),
        4,
        "a coalesced submission still returns a handle to the existing job"
    );
    let first = submitted[0].id();
    let keyless_one = submitted[2].id();
    let keyless_two = submitted[3].id();

    // Three of the four submissions become jobs: the duplicate is coalesced away rather
    // than queued, and only the keyless pair is allowed to be two.
    assert_eq!(
        scheduler.metrics().await.live,
        3,
        "the coalesced submission must not become a fourth job"
    );
    assert_eq!(order.recv().await, Some(0), "only one job started");
    assert_ne!(keyless_one, keyless_two, "`None` never dedupes");
    assert_ne!(first, keyless_one);
    let _ = release.send(());
}

#[tokio::test]
async fn a_dedupe_key_only_coalesces_its_own_key() {
    let scheduler = scheduler_with_one_worker();
    let (started, mut order) = mpsc::unbounded_channel();

    // Four submissions in one batch: a job with a key, a duplicate of it, a job with a
    // *different* key, and a keyless job. Only the duplicate may disappear.
    let (owner_work, owner_guard) = Blocking::gated(&started, 0);
    let (other_work, other_guard) = Blocking::gated(&started, 1);
    let submitted = scheduler
        .submit_all(vec![
            job("test.dedupe", 1, Some("title:1"), owner_work),
            job("test.dedupe", 1, Some("title:1"), Noop),
            job("test.dedupe", 1, Some("title:2"), other_work),
            job("test.dedupe", 1, None, Noop),
        ])
        .await
        .unwrap();
    // The duplicate coalesced instead of starting: the engine tracks three jobs, and the
    // coalesced handle resolves as cancelled because there is no second job to report on.
    // The job that owns the key is the one whose outcome matters.
    assert_eq!(
        scheduler.metrics().await.live,
        3,
        "a different key must queue its own job rather than join another key's job"
    );
    assert_eq!(order.recv().await, Some(0), "the first job took the worker");
    drop((owner_guard, other_guard));
    let mut submitted = submitted.into_iter();
    let _owner = submitted.next().expect("the first submission was accepted");
    let duplicate = submitted.next().expect("the duplicate has a handle");
    assert!(
        matches!(duplicate.wait().await.into_outcome(), JobOutcome::Cancelled),
        "a coalesced submission reports that it did not run"
    );
}

#[tokio::test]
async fn a_dedupe_key_is_released_when_its_job_finishes() {
    let scheduler = scheduler_with_one_worker();
    let first = scheduler
        .submit_if_absent(spec("test.dedupe", 1, Some("title:7")), Noop)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        first.wait().await.into_outcome(),
        JobOutcome::Succeeded(_)
    ));

    // The key became free again, so the next caller is not coalesced onto a dead job.
    let second = scheduler
        .submit_if_absent(spec("test.dedupe", 1, Some("title:7")), Noop)
        .await
        .unwrap();
    assert!(
        second.is_some(),
        "a finished job must not keep its dedupe key forever"
    );
}

#[tokio::test]
async fn a_queued_job_stops_existing_when_it_is_cancelled() {
    let scheduler = scheduler_with_one_worker();
    let (started, mut order) = mpsc::unbounded_channel();
    let (release, released) = oneshot::channel();

    // The blocker occupies the one worker, so the second job cannot start.
    scheduler
        .submit(
            spec("test.blocking", 1, None),
            Blocking::new(&started, 0, released),
        )
        .await
        .unwrap();
    // The second job is never released, so if it did start the test would see label 1.
    let (queued_work, _queued_guard) = Blocking::gated(&started, 1);
    let mut queued = scheduler
        .submit(spec("test.blocking", 1, None), queued_work)
        .await
        .unwrap();

    queued.cancel();
    let completion = queued.wait().await;
    assert!(
        matches!(completion.outcome(), JobOutcome::Cancelled),
        "got {:?}",
        completion.outcome()
    );
    let _ = release.send(());
    assert_eq!(
        order.recv().await,
        Some(0),
        "the cancelled job must never start"
    );
    assert!(
        order.try_recv().is_err(),
        "nothing else was waiting to start"
    );
}

#[tokio::test]
async fn a_running_job_observes_its_token_and_the_engine_keeps_working() {
    let scheduler = scheduler_with_one_worker();
    let (running, started) = oneshot::channel();
    let mut handle = scheduler
        .submit(
            spec("test.waits", 1, None),
            WaitsForCancellation::new(running),
        )
        .await
        .unwrap();

    let _ = started.await;
    handle.cancel();
    handle.cancelled().await;
    let completion = handle.wait().await;
    assert!(
        matches!(completion.outcome(), JobOutcome::Cancelled),
        "a job that observed its token must resolve as cancelled"
    );

    // The slot came back: the next job runs on the same one-worker scheduler.
    let next = scheduler
        .submit(spec("test.noop", 1, None), Noop)
        .await
        .unwrap()
        .wait()
        .await;
    assert!(matches!(next.outcome(), JobOutcome::Succeeded(_)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_panicking_job_fails_only_itself_and_releases_its_slot() {
    let scheduler = scheduler_with_one_worker();
    let panicked = scheduler
        .submit(spec("test.panics", 1, None), Panics)
        .await
        .unwrap();
    let token = panicked.cancellation().clone();

    let completion = panicked.wait().await;
    let failure = match completion.outcome() {
        JobOutcome::Failed(error) => error.clone(),
        other => panic!("a panicked attempt must resolve as failed, got {other:?}"),
    };
    assert!(
        failure.to_string().contains("panicked"),
        "the failure must say it was a panic: {failure}"
    );

    // The metrics request is served by the engine task itself, so reading it is also the
    // synchronisation point that proves the engine survived the panic.
    assert_eq!(
        scheduler.inflight().await,
        0,
        "the panicked attempt's slot must return to the pool"
    );
    let next = scheduler
        .submit(spec("test.noop", 1, None), Noop)
        .await
        .unwrap()
        .wait()
        .await;
    assert!(
        matches!(next.outcome(), JobOutcome::Succeeded(_)),
        "the scheduler must still run jobs after a panic"
    );

    // Shutting down is what releases the token of every job the scheduler owns, which is
    // the other half of "the panic did not leak its slot": the slot came back above, and
    // the token comes back here.
    scheduler.shutdown().await;
    assert!(
        token.is_cancelled(),
        "a stopped scheduler must release its jobs' tokens"
    );
}

#[tokio::test(start_paused = true)]
async fn a_deferred_job_retries_on_schedule_and_with_a_growing_delay() {
    let scheduler = scheduler_with_one_worker();
    let mut handle = scheduler
        .submit(
            spec("test.defers", 1, None),
            DefersUntil { succeeded_on: 3 },
        )
        .await
        .unwrap();

    let mut deadlines = Vec::new();
    for expected_attempt in 1..=2_u32 {
        let state = wait_for_phase(&mut handle, JobStateKind::Deferred).await;
        assert_eq!(state.attempts(), expected_attempt);
        let deadline = state.next_run_at().unwrap();
        let delay = state.retry_delay().unwrap();
        assert!(
            delay > Duration::ZERO,
            "a deferral must report the delay it was scheduled with"
        );
        assert!(
            state.deferred_at().is_some(),
            "a deferral must report the wall instant it was decided"
        );
        deadlines.push(deadline);

        // One millisecond before the deadline the job is still waiting. `advance` is the
        // only clock control used here: `sleep` would let Tokio's auto-advance skip
        // straight to the engine's timer, which would fire the retry behind the test's
        // back and turn this from a schedule assertion into a race.
        tokio::time::advance(Duration::from_millis(1)).await;
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            handle.state().phase(),
            JobStateKind::Deferred,
            "attempt {expected_attempt} ran before its scheduled deadline"
        );

        // Stepping to the deadline — and not one millisecond less — fires the retry, which
        // is what makes this an assertion about the schedule rather than about the clock.
        tokio::time::advance(deadline.saturating_duration_since(Instant::now())).await;
        let fired = wait_until(|| {
            let state = handle.state();
            state.phase() != JobStateKind::Deferred || state.attempts() > expected_attempt
        })
        .await;
        assert!(
            fired,
            "the retry for attempt {expected_attempt} did not fire at its deadline"
        );
    }

    assert!(
        deadlines[1] > deadlines[0],
        "each retry must be scheduled later than the last: {deadlines:?}"
    );

    let completion = handle.wait().await;
    assert!(
        matches!(completion.outcome(), JobOutcome::Succeeded(_)),
        "the third attempt was told to succeed, got {:?}",
        completion.outcome()
    );
    assert_eq!(
        completion.attempts(),
        3,
        "attempts count from one and the count is observable"
    );
}

#[tokio::test(start_paused = true)]
async fn a_job_that_always_defers_is_abandoned_when_its_retry_budget_runs_out() {
    let scheduler = Scheduler::new(
        SchedulerConfig {
            backoff: Arc::new(Backoff {
                initial: Duration::from_millis(1),
                ..Backoff::default()
            }),
            ..SchedulerConfig::new(clock(), Arc::new(CountingIds::default()))
        }
        .with_max_concurrency(NonZeroUsize::MIN),
    );
    let mut job_spec = spec("test.defers", 1, None);
    job_spec.max_attempts = 2;
    let mut handle = scheduler.submit(job_spec, AlwaysDefers).await.unwrap();

    wait_for_phase(&mut handle, JobStateKind::Deferred).await;
    tokio::time::advance(Duration::from_secs(1)).await;

    let completion = handle.wait().await;
    match completion.outcome() {
        JobOutcome::Failed(error) => {
            assert!(error.to_string().contains("still not yet"), "got {error}");
        }
        other => panic!("expected exhaustion to resolve as a failure, got {other:?}"),
    }
    assert_eq!(
        completion.attempts(),
        2,
        "a two-attempt budget must stop after two attempts"
    );
}

#[tokio::test]
async fn a_successful_job_resolves_with_its_value_and_its_attempt_count() {
    let scheduler = scheduler_with_one_worker();
    let handle = scheduler
        .submit(spec("test.value", 1, None), Noop)
        .await
        .unwrap();
    let completion: JobCompletion = handle.wait().await;
    assert_eq!(completion.attempts(), 1);
    let output = completion.succeeded().expect("the job succeeded");
    assert!(output.downcast::<()>().is_ok());
}

#[tokio::test]
async fn an_empty_scheduler_runs_nothing_and_reports_zero() {
    let scheduler = scheduler_with_one_worker();
    assert_eq!(
        scheduler.metrics().await,
        crate::SchedulerMetrics::default()
    );
    assert_eq!(scheduler.queued().await, 0);
    assert_eq!(scheduler.inflight().await, 0);
    // Shutting down with nothing to do must not hang or panic.
    scheduler.shutdown().await;
}

#[tokio::test]
async fn dropping_a_handle_does_not_cancel_the_job() {
    // `ARCHITECTURE.md` §7 rule 6: cancellation is requested, never implied by a dropped
    // subscriber. The job's own token is the proof: if dropping the handle had cancelled
    // it, the token would be tripped.
    let scheduler = scheduler_with_one_worker();
    let (running, started) = oneshot::channel();
    let handle = scheduler
        .submit(
            spec("test.waits", 1, None),
            WaitsForCancellation::new(running),
        )
        .await
        .unwrap();
    let token = handle.cancellation().clone();
    let _ = started.await;
    drop(handle);

    tokio::task::yield_now().await;
    assert!(
        !token.is_cancelled(),
        "dropping the only handle must not cancel user-visible work"
    );
}

#[tokio::test]
async fn a_shared_scheduler_can_be_shut_down_through_a_borrow() {
    // The application holds one `Arc<Scheduler>` and uses it for both submission and
    // shutdown, so `shutdown` must work through a shared borrow. The job below is queued
    // behind a worker that is still running, which is the case where the root token alone
    // is not enough: the engine has to be told to stop so the queued job never starts.
    let scheduler = Arc::new(scheduler_with_one_worker());
    let (running, started) = oneshot::channel();
    let blocker = scheduler
        .submit(
            spec("test.blocker", 1, None),
            WaitsForCancellation::new(running),
        )
        .await
        .unwrap();
    let _ = started.await;
    let queued = scheduler
        .submit(spec("test.queued", 1, None), Noop)
        .await
        .unwrap();
    let queued_token = queued.cancellation().clone();

    let shared = Arc::clone(&scheduler);
    shared.shutdown().await;

    assert!(
        queued_token.is_cancelled(),
        "a queued job's token must be released by shutdown, not only by its own handle"
    );
    assert!(
        blocker.cancellation().is_cancelled(),
        "a running job's token must be released by shutdown"
    );
    // A second shutdown finds no engine to wait for; it must return rather than hang.
    shared.shutdown().await;
}
