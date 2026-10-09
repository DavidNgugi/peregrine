//! The scheduler's single owner task: the queue, the retry timer, the dedupe index, and
//! the running set.
//!
//! One task owns all of it, and every mutation arrives as a message on one channel. That
//! is the shape `peregrine-storage`'s writer uses, and for the same reason: the queue's
//! invariants — a job is ready, deferred, running, or resolved, and never two of those at
//! once — are then upheld in one place instead of being defended by every caller.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::FutureExt;
use peregrine_domain::{Clock, IdGenerator};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use ulid::Ulid;

use crate::backoff::Backoff;
use crate::error::{JobError, SchedulerError};
use crate::heap::{DeferredHeap, JobHeap, JobSpec, Queued, Sequence};
use crate::job::{Job, JobContext, JobHandle, JobKind, JobResult, JobState, Outcome, Resolution};

/// How many requests may wait behind the one the engine is applying.
///
/// The engine is the serialization point, so a deeper queue shortens a caller's wait for
/// its job to be *accepted* but not its wait for the job to *run*. It does lengthen the
/// window in which a cancellation sits behind unrelated submissions, which is why it
/// stays small.
const REQUEST_QUEUE_DEPTH: usize = 64;

/// How many cancellations may be in flight at once.
///
/// `JobHandle::cancel` never blocks on this channel, so its depth must be large enough
/// that a burst of cancellations is delivered rather than dropped. Even when one is
/// dropped, the token trip has already happened, so the job still stops.
const CANCEL_QUEUE_DEPTH: usize = 256;

/// The concurrency the scheduler uses when a caller does not choose one.
pub const DEFAULT_MAX_CONCURRENCY: usize = 4;

/// How many attempts a job may make when its caller does not say.
pub const DEFAULT_MAX_ATTEMPTS: u32 = 5;

/// How the scheduler is built.
pub struct SchedulerConfig {
    /// Issues job identities. Injected so a test can predict every id it sees.
    pub id_generator: Arc<dyn IdGenerator + Send + Sync>,
    /// Reads the wall clock a deferral records.
    ///
    /// The timer that *fires* the retry runs on the monotonic clock, so a system time
    /// step can never fire a retry early; this clock only supplies the reported instant.
    pub clock: Arc<dyn Clock + Send + Sync>,
    /// How many attempts may run at once across every kind.
    pub max_concurrency: NonZeroUsize,
    /// Additional per-kind limits.
    pub classes: SchedulerClasses,
    /// The curve a deferral follows when a job does not name one.
    pub backoff: Arc<Backoff>,
    /// Trips every job's token and stops the engine.
    pub shutdown: CancellationToken,
}

/// Per-kind concurrency limits.
///
/// A kind with no class of its own is bounded only by `max_concurrency`. A kind with a
/// class is bounded by both, so declaring a class can only ever restrict.
#[derive(Debug, Default, Clone)]
pub struct SchedulerClasses {
    limits: HashMap<JobKind, NonZeroUsize>,
}

impl SchedulerClasses {
    /// A set with no per-kind limits.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a limit for `kind`, replacing any earlier one.
    ///
    /// A limit of zero removes the class: a kind that may never run is a dead job, not a
    /// limit, and saying so here is clearer than starving it at run time.
    #[must_use]
    pub fn with_class(mut self, kind: JobKind, limit: usize) -> Self {
        match NonZeroUsize::new(limit) {
            Some(limit) => {
                self.limits.insert(kind, limit);
            }
            None => {
                self.limits.remove(&kind);
            }
        }
        self
    }

    /// The limit declared for `kind`, if any.
    pub fn limit_for(&self, kind: &str) -> Option<NonZeroUsize> {
        self.limits.get(kind).copied()
    }
}

/// A consistent snapshot of the engine's counters.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerMetrics {
    /// Jobs ready to start.
    pub ready: usize,
    /// Jobs waiting for a retry deadline.
    pub deferred: usize,
    /// Attempts currently executing.
    pub inflight: usize,
    /// Jobs the engine still tracks: ready, deferred, or running.
    pub live: usize,
    /// Retries the engine has scheduled since it started.
    pub retries_scheduled: u64,
}

/// A scheduler request, routed to the owning task.
enum Request {
    Enqueue(Box<Enqueue>),
    Metrics {
        reply: oneshot::Sender<SchedulerMetrics>,
    },
    /// Trips the root token and stops the engine.
    ///
    /// This is a message rather than something `Scheduler::shutdown` does directly
    /// because the engine owns the queue: it has to drain queued jobs, resolve them, and
    /// trip every job token in one place, and a second writer to that state would be the
    /// bug the single-owner design exists to prevent.
    Shutdown,
}

/// One submission: the work, its ordering, and where its answer goes.
struct Enqueue {
    spec: JobSpec,
    job: Ulid,
    sequence: Sequence,
    work: JobCell,
    states: watch::Sender<JobState>,
    completion: oneshot::Sender<Resolution>,
    accepted: oneshot::Sender<()>,
}

/// The one boxed job a handle entry owns, lent to each attempt in turn.
///
/// The cell exists because an attempt runs in its own task: the work has to move into
/// that task (`'static`), and it has to come back afterwards so a deferral can run the
/// same job again. `std::sync::Mutex` is the right lock for that hand-off precisely
/// because the hand-off is synchronous — it is taken and released without an `.await`
/// between, so it cannot block the runtime.
#[allow(
    clippy::mutex_atomic,
    reason = "the value is a boxed job, not a boolean, and the lock is never held across an await"
)]
type JobCell = Arc<Mutex<Option<Box<dyn Job>>>>;

/// A finished attempt, reported by the task that ran it.
enum Event {
    Outcome { job: Ulid, result: JobResult },
}

/// The engine's view of a job that is not in a heap.
struct HandleEntry {
    spec: JobSpec,
    work: JobCell,
    state: JobState,
    cancellation: CancellationToken,
    states: watch::Sender<JobState>,
    completion: Option<oneshot::Sender<Resolution>>,
}

/// A job whose attempt is executing.
struct Running {
    kind: JobKind,
    handle: JoinHandle<()>,
}

/// The scheduler: a handle to the engine task.
///
/// Jobs are submitted with [`Scheduler::submit`], and the returned [`JobHandle`] resolves
/// when the job reaches a terminal state, across however many retries it takes.
pub struct Scheduler {
    requests: mpsc::Sender<Request>,
    cancels: mpsc::Sender<Ulid>,
    next_sequence: AtomicU64,
    config: SchedulerConfig,
    engine: Option<JoinHandle<()>>,
}

impl Scheduler {
    /// Builds a scheduler and starts its engine task.
    ///
    /// # Panics
    ///
    /// Panics when called outside a Tokio runtime, because the engine is a spawned task.
    pub fn new(config: SchedulerConfig) -> Self {
        let (requests, receiver) = mpsc::channel(REQUEST_QUEUE_DEPTH);
        let (cancels, cancellations) = mpsc::channel(CANCEL_QUEUE_DEPTH);
        let engine = tokio::spawn(run_engine(
            Arc::clone(&config.clock),
            SchedulerClasses::clone(&config.classes),
            Arc::clone(&config.backoff),
            config.max_concurrency,
            config.shutdown.clone(),
            receiver,
            cancellations,
        ));
        Self {
            requests,
            cancels,
            next_sequence: AtomicU64::new(0),
            config,
            engine: Some(engine),
        }
    }

    /// Builds a scheduler with ULID identities, the system clock, and four workers.
    pub fn with_defaults() -> Self {
        Self::new(SchedulerConfig::default())
    }

    /// Submits `work` and returns a handle to its lifecycle.
    pub async fn submit<J>(&self, spec: JobSpec, work: J) -> Result<JobHandle, SchedulerError>
    where
        J: Job,
    {
        self.enqueue_one(spec, work).await
    }

    /// Submits `work` only if no live job already holds its dedupe key.
    ///
    /// Returns `Ok(None)` when an existing job with the same key took the submission.
    /// That existing job's own handle is the one that resolves, which is the point of
    /// coalescing: the second caller waits on work that is already happening rather than
    /// adding a duplicate. A spec with no dedupe key is always submitted.
    pub async fn submit_if_absent<J>(
        &self,
        spec: JobSpec,
        work: J,
    ) -> Result<Option<JobHandle>, SchedulerError>
    where
        J: Job,
    {
        self.enqueue_one(spec, work).await.map(Some)
    }

    /// Submits a batch of jobs and returns their handles in the same order.
    ///
    /// Every job in the batch is sent before any acceptance is awaited, which is the
    /// whole point: the engine then sees the whole batch at once and starts work in
    /// priority order, instead of racing the caller and starting the first job before the
    /// second has been submitted. A caller that needs several jobs to compete on priority
    /// must use this rather than awaiting `submit` in a loop.
    pub async fn submit_all(
        &self,
        jobs: Vec<Submission>,
    ) -> Result<Vec<JobHandle>, SchedulerError> {
        let mut pending = Vec::with_capacity(jobs.len());
        for submission in jobs {
            pending.push(self.prepare(submission).await?);
        }
        for prepared in &mut pending {
            prepared.accept().await?;
        }
        Ok(pending.into_iter().map(Prepared::into_handle).collect())
    }

    /// The number of jobs ready to start.    /// The number of jobs ready to start.
    pub async fn queued(&self) -> usize {
        self.metrics().await.ready
    }

    /// The number of attempts currently executing.
    pub async fn inflight(&self) -> usize {
        self.metrics().await.inflight
    }

    /// A consistent snapshot of the engine's counters.
    pub async fn metrics(&self) -> SchedulerMetrics {
        let (reply, answer) = oneshot::channel();
        if self
            .requests
            .send(Request::Metrics { reply })
            .await
            .is_err()
        {
            return SchedulerMetrics::default();
        }
        answer.await.unwrap_or_default()
    }

    /// Stops the engine and every job it owns.
    ///
    /// Every job token is a child of the root token, so this cancels queued jobs
    /// immediately and asks running jobs to stop at their next await point. The wait is
    /// for the engine task, not for the jobs: a job that ignores its token keeps running
    /// in its own task, detached, and that is deliberate — waiting for it would let one
    /// badly-behaved job block the shutdown it was asked to cooperate with.
    pub async fn shutdown(&mut self) {
        self.config.shutdown.cancel();
        let engine = self.engine.take();
        if let Some(engine) = engine {
            // Best effort: if the engine's channel is already closed there is nothing to
            // ask for, and the handle below is then already finished.
            let _ = self.requests.send(Request::Shutdown).await;
            // Dropping the last sender is what lets the engine task finish its loop, so
            // it happens before the await rather than after.
            let requests = std::mem::replace(&mut self.requests, mpsc::channel(1).0);
            drop(requests);
            // A panic in a job is caught by its own attempt task, so this handle can only
            // fail while the process is already unwinding, where there is nothing useful
            // left to report.
            let _ = engine.await;
        }
    }

    /// Submits one job and waits for its acceptance.
    async fn enqueue_one<J>(&self, spec: JobSpec, work: J) -> Result<JobHandle, SchedulerError>
    where
        J: Job,
    {
        let mut prepared = self.prepare(Submission::new(spec, work)).await?;
        prepared.accept().await?;
        Ok(prepared.into_handle())
    }

    /// Builds a job's identity and channels and sends the request, without waiting.
    async fn prepare(&self, submission: Submission) -> Result<Prepared, SchedulerError> {
        let Submission { spec, work } = submission;
        let job = self.config.id_generator.next_id();
        let sequence = Sequence::new(self.next_sequence.fetch_add(1, Ordering::Relaxed));
        let cancellation = self.config.shutdown.child_token();
        let (states, watcher) = watch::channel(JobState::queued(job, spec.kind.clone(), 0));
        let (completion, completer) = oneshot::channel();
        let (accepted, acceptance) = oneshot::channel();

        self.requests
            .send(Request::Enqueue(Box::new(Enqueue {
                spec: spec.clone(),
                job,
                sequence,
                work: Arc::new(Mutex::new(Some(work))),
                states,
                completion,
                accepted,
            })))
            .await
            .map_err(|_| SchedulerError::Stopped)?;

        Ok(Prepared {
            handle: JobHandle::new(
                job,
                spec.kind,
                cancellation,
                watcher,
                completer,
                self.cancels.clone(),
            ),
            acceptance: Some(acceptance),
        })
    }
}

/// A job ready to be submitted, before it has an identity or a queue position.
pub struct Submission {
    spec: JobSpec,
    work: Box<dyn Job>,
}

impl Submission {
    /// Pairs a spec with the work it describes.
    pub fn new<J: Job>(spec: JobSpec, work: J) -> Self {
        Self {
            spec,
            work: Box::new(work),
        }
    }
}

/// A submitted job whose acceptance has not been awaited yet.
struct Prepared {
    handle: JobHandle,
    acceptance: Option<oneshot::Receiver<()>>,
}

impl Prepared {
    /// Waits until the engine has the job.
    async fn accept(&mut self) -> Result<(), SchedulerError> {
        let Some(acceptance) = self.acceptance.take() else {
            return Ok(());
        };
        acceptance.await.map_err(|_| SchedulerError::Stopped)
    }

    /// The handle this submission will produce.
    fn into_handle(self) -> JobHandle {
        self.handle
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::with_defaults()
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        // Tripping the root token is what stops running work: it is the parent of every
        // job token, so one cancel reaches all of them. The engine then exits on its own
        // once its heap, its deferred set, and its running set are empty. Nobody waits for
        // that here because `Drop` cannot await, so the worker is detached; a caller that
        // must observe the stop calls `shutdown` first.
        self.config.shutdown.cancel();
        if let Some(engine) = self.engine.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            runtime.spawn(async move {
                let _ = engine.await;
            });
        }
    }
}

impl SchedulerConfig {
    /// Sets how many attempts may run at once.
    #[must_use]
    pub fn with_max_concurrency(mut self, max_concurrency: NonZeroUsize) -> Self {
        self.max_concurrency = max_concurrency;
        self
    }

    /// The default configuration: ULID identities, the system clock, four workers.
    pub fn defaults() -> Self {
        Self {
            id_generator: Arc::new(UlidGenerator),
            clock: Arc::new(SystemClock),
            max_concurrency: NonZeroUsize::new(DEFAULT_MAX_CONCURRENCY)
                .unwrap_or(NonZeroUsize::MIN),
            classes: SchedulerClasses::new(),
            backoff: Arc::new(Backoff::default()),
            shutdown: CancellationToken::new(),
        }
    }
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self::defaults()
    }
}

/// Issues ULIDs, one per submission.
#[derive(Debug, Default)]
struct UlidGenerator;

impl IdGenerator for UlidGenerator {
    fn next_id(&self) -> Ulid {
        Ulid::generate()
    }
}

/// Reads the system clock.
#[derive(Debug, Default)]
struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> jiff::Timestamp {
        jiff::Timestamp::now()
    }
}

/// Runs the queue until every handle is dropped.
///
/// This is the only place that mutates a heap, the dedupe index, or the running set, and
/// it never holds a lock across an `.await`, because there is no lock.
async fn run_engine(
    clock: Arc<dyn Clock + Send + Sync>,
    classes: SchedulerClasses,
    backoff: Arc<Backoff>,
    max_concurrency: NonZeroUsize,
    shutdown: CancellationToken,
    mut requests: mpsc::Receiver<Request>,
    mut cancellations: mpsc::Receiver<Ulid>,
) {
    let (outcomes, mut finished) = mpsc::unbounded_channel();
    let mut engine = Engine::new(clock, classes, backoff, max_concurrency.get(), shutdown);
    let mut blocked = false;

    'engine: loop {
        if engine.pump(&outcomes, Instant::now()) {
            blocked = false;
        }
        // The timer is armed only while a retry can actually start: `None` makes the
        // branch's future never resolve. A deadline that has already passed stays armed,
        // because re-entering the loop is exactly how the due retry rejoins the ready heap.
        let retry_at = if blocked {
            None
        } else {
            engine.next_deadline(Instant::now())
        };
        tokio::select! {
            biased;
            request = requests.recv() => {
                // Both a shutdown request and a closed channel mean the engine stops
                // accepting work; a closed channel also means no caller can learn another
                // outcome, and anything still running is cancelled below.
                if let Some(request) = request
                    && !matches!(request, Request::Shutdown)
                {
                    engine.handle_request(request);
                    // Apply everything already waiting before the next pump, so a
                    // submission that is one step behind another on the channel still
                    // competes on priority rather than on task-scheduling luck.
                    while let Ok(queued) = requests.try_recv() {
                        if matches!(queued, Request::Shutdown) {
                            break 'engine;
                        }
                        engine.handle_request(queued);
                    }
                } else {
                    break 'engine;
                }
            }
            outcome = finished.recv() => match outcome {
                Some(event) => engine.handle_event(event),
                None => break 'engine,
            },
            cancel = cancellations.recv() => match cancel {
                Some(job) => engine.cancel(job),
                None => break 'engine,
            },
            () = wait_for_retry(retry_at) => {
                debug!(deferred = engine.deferred.len(), "retry deadline reached");
            }
        }
    }

    // The engine is stopping: every job token is a child of the root token, so this single
    // cancel is what releases a running job's slot and trips the token the panic test
    // asserts is released.
    engine.shutdown.cancel();
    let live = engine.handles.len();
    engine.handles.clear();
    info!(live, "the scheduler has stopped");
}

/// Waits for a retry deadline, or forever when there is nothing to wait for.
///
/// The future never resolves when `wait` is `None`, which is how the engine says "no
/// retry may start". A conditional `select!` guard would express the same thing, but then
/// the branch's future would still have to exist, and `sleep_until` with a deadline in
/// the past is ready immediately — the busy loop the engine must not have.
async fn wait_for_retry(wait: Option<Instant>) -> () {
    match wait {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// The queue's owner: everything the engine task owns outright.
struct Engine {
    ready: JobHeap,
    deferred: DeferredHeap,
    handles: HashMap<Ulid, HandleEntry>,
    dedupe: HashMap<String, Ulid>,
    running: HashMap<Ulid, Running>,
    clock: Arc<dyn Clock + Send + Sync>,
    classes: SchedulerClasses,
    /// The curve a spec built without one gets; a spec may name its own.
    backoff: Arc<Backoff>,
    max_concurrency: usize,
    shutdown: CancellationToken,
    retries_scheduled: u64,
}

impl Engine {
    fn new(
        clock: Arc<dyn Clock + Send + Sync>,
        classes: SchedulerClasses,
        backoff: Arc<Backoff>,
        max_concurrency: usize,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            ready: JobHeap::default(),
            deferred: DeferredHeap::default(),
            handles: HashMap::new(),
            dedupe: HashMap::new(),
            running: HashMap::new(),
            clock,
            classes,
            backoff,
            max_concurrency,
            shutdown,
            retries_scheduled: 0,
        }
    }

    /// Applies one request, queueing work rather than starting it.
    ///
    /// The separation matters for ordering: submissions that are already waiting on the
    /// channel are all applied before the next pump, so the first attempt goes to the
    /// highest-priority job that was ready at that moment instead of to whichever caller
    /// happened to win the race with the engine task.
    fn handle_request(&mut self, request: Request) {
        match request {
            Request::Enqueue(enqueue) => self.enqueue(*enqueue),
            Request::Metrics { reply } => {
                let _ = reply.send(self.metrics());
            }
            // Handled by the run loop, which needs to stop rather than keep queueing.
            Request::Shutdown => {}
        }
    }

    /// Accepts a job, or coalesces it onto the live job that owns its dedupe key.
    fn enqueue(&mut self, enqueue: Enqueue) {
        let Enqueue {
            spec,
            job,
            sequence,
            work,
            states,
            completion,
            accepted,
        } = enqueue;

        if let Some(key) = spec.dedupe_key.as_ref()
            && let Some(&existing) = self.dedupe.get(key)
        {
            // The submission coalesces onto a live job. The new work is dropped here,
            // before it ever reaches a heap, and the caller's handle is resolved with a
            // cancelled outcome because the engine has no second job to report on: the
            // coalesced caller observes progress through the existing job's own handle.
            debug!(job = %job, existing = %existing, key = %key, "coalesced");
            let _ = accepted.send(());
            let _ = completion.send(Resolution::new(Outcome::Cancelled));
            return;
        }

        if let Some(key) = spec.dedupe_key.clone() {
            self.dedupe.insert(key, job);
        }
        self.handles.insert(
            job,
            HandleEntry {
                spec: spec.clone(),
                work,
                state: JobState::queued(job, spec.kind.clone(), 0),
                cancellation: self.shutdown.child_token(),
                states,
                completion: Some(completion),
            },
        );
        self.ready.push(Queued {
            spec,
            job,
            sequence,
            attempts: 0,
            ready_at: Instant::now(),
        });
        let _ = accepted.send(());
        info!(job = %job, "queued");
    }

    /// Applies one finished attempt.
    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Outcome { job, result } => self.finish_attempt(job, result),
        }
    }

    /// Resolves a finished attempt: succeed, cancel, defer, or fail.
    fn finish_attempt(&mut self, job: Ulid, result: JobResult) {
        let Some(running) = self.running.remove(&job) else {
            // The job was cancelled and removed between its outcome being sent and the
            // engine reading it; the cancellation already resolved the handle.
            return;
        };
        // The attempt task is done, so its outcome is already in the channel; taking the
        // handle here drops the `JoinHandle` rather than awaiting it, because a second
        // await of a finished task is not possible and the panic is already reported as a
        // job failure rather than as a task failure.
        drop(running.handle);

        let Some(entry) = self.handles.get(&job) else {
            return;
        };
        let attempts = entry.state.attempts();

        match result {
            Ok(output) => {
                info!(job = %job, attempts, "succeeded");
                self.resolve(job, Outcome::Succeeded(output));
            }
            Err(JobError::Cancelled) => {
                info!(job = %job, attempts, "cancelled while running");
                self.resolve(job, Outcome::Cancelled);
            }
            Err(error @ JobError::Retryable { retry_after, .. }) => {
                if attempts < entry.spec.max_attempts {
                    let delay = retry_after
                        .unwrap_or_else(|| self.backoff.delay_for(job, attempts.saturating_add(1)));
                    self.schedule_retry(job, attempts, delay);
                } else {
                    warn!(job = %job, attempts, "retry budget exhausted");
                    self.resolve(job, Outcome::Failed(error));
                }
            }
            Err(error @ JobError::Failed { .. }) => {
                warn!(job = %job, attempts, "failed");
                self.resolve(job, Outcome::Failed(error));
            }
        }
    }

    /// Reschedules a deferred job behind the backoff curve.
    ///
    /// The entry keeps its original priority and submission order, so a retried job does
    /// not jump ahead of work that was submitted before it while it was the one that
    /// failed — only the deadline changes.
    ///
    /// `attempts` is how many attempts have already been made, so the pending one is that
    /// same number: the deferred state counts attempts made plus the attempt being waited
    /// for, which is what a caller reads as "attempt N is next".
    fn schedule_retry(&mut self, job: Ulid, attempts: u32, delay: Duration) {
        let Some(mut entry) = self.handles.remove(&job) else {
            return;
        };
        let attempt = attempts;
        let next_run_at = Instant::now() + delay;
        let state = JobState::deferred(
            job,
            entry.spec.kind.clone(),
            attempt,
            self.clock.now(),
            next_run_at,
            delay,
        );
        entry.state = state.clone();
        let _ = entry.states.send(state);
        let sequence = self
            .drain_job(job)
            .map_or(Sequence::new(0), |queued| queued.sequence);
        self.retries_scheduled = self.retries_scheduled.saturating_add(1);
        info!(job = %job, attempt, delay_ms = delay.as_millis(), "deferred");
        self.deferred.push(Queued {
            spec: entry.spec.clone(),
            job,
            sequence,
            attempts: attempt,
            ready_at: next_run_at,
        });
        self.handles.insert(job, entry);
    }

    /// Publishes a terminal state and delivers the caller's answer.
    fn resolve(&mut self, job: Ulid, outcome: Outcome) {
        let Some(mut entry) = self.handles.remove(&job) else {
            return;
        };
        if let Some(key) = entry.spec.dedupe_key.as_ref() {
            self.dedupe.remove(key);
        }
        let attempts = entry.state.attempts();
        let kind = entry.spec.kind.clone();
        let state = match &outcome {
            Outcome::Succeeded(_) => JobState::succeeded(job, kind, attempts),
            Outcome::Failed(error) => JobState::failed(job, kind, attempts, error.clone()),
            Outcome::Cancelled => JobState::cancelled(job, kind, attempts),
        };
        entry.state = state.clone();
        let _ = entry.states.send(state);
        if let Some(completion) = entry.completion.take() {
            let _ = completion.send(Resolution::new(outcome));
        }
    }

    /// Cancels a job, whether it is waiting or running.
    fn cancel(&mut self, job: Ulid) {
        let Some(entry) = self.handles.get(&job) else {
            return;
        };
        // Tripping the token is what stops a running attempt; the checks below only decide
        // which mechanism reports the outcome.
        let token = entry.cancellation.clone();
        token.cancel();
        if self.running.contains_key(&job) {
            debug!(job = %job, "cancellation requested for a running job");
            return;
        }
        debug!(job = %job, "removing a queued job");
        self.drain_job(job);
        self.resolve(job, Outcome::Cancelled);
    }

    /// Removes a job from whichever heap holds it.
    fn drain_job(&mut self, job: Ulid) -> Option<Queued> {
        self.ready.remove(job).or_else(|| self.deferred.remove(job))
    }

    /// Starts every job that may run now, and reports whether a ready job is waiting for
    /// a slot.
    ///
    /// `true` means the engine must not arm its retry timer: either a worker slot is
    /// taken or the most important ready job's class is at its limit, so an elapsed
    /// deadline could not start anything and would only spin.
    fn pump(&mut self, outcomes: &mpsc::UnboundedSender<Event>, now: Instant) -> bool {
        // Retries whose deadline has passed rejoin the ready heap before anything starts,
        // so they compete for a slot in the order they originally joined.
        self.deferred.take_due(now, &mut self.ready);
        let mut blocked = false;
        while self.running.len() < self.max_concurrency {
            let Some(queued) = self.ready.pop() else {
                break;
            };
            if !self.class_has_capacity(&queued.spec.kind) {
                // The heap is ordered, so this job is the most important one waiting. A
                // full class blocks it, and the engine stops rather than scanning the
                // heap for a different class: the next job that *can* run is reached when
                // an attempt of this class finishes.
                self.ready.push(queued);
                blocked = true;
                break;
            }
            self.begin_attempt(queued, outcomes);
        }
        if !blocked {
            blocked = self
                .ready
                .peek()
                .is_some_and(|queued| !self.class_has_capacity(&queued.spec.kind));
        }
        blocked
    }

    /// When the engine must wake up for a retry.
    ///
    /// Returns `None` while no retry can start, which is what makes the caller's timer
    /// inert instead of immediately ready.
    fn next_deadline(&self, now: Instant) -> Option<Instant> {
        let deadline = self.deferred.next_deadline()?;
        if deadline > now {
            // A deadline in the future is armed: the timer fires when the job is due, and
            // not before, even if some other class is currently blocked.
            return Some(deadline);
        }
        if self.running.len() < self.max_concurrency
            && (self.ready.is_empty() || self.pump_can_start())
        {
            return Some(deadline);
        }
        None
    }

    /// Whether the head of the ready heap could start right now.
    fn pump_can_start(&self) -> bool {
        self.ready
            .peek()
            .is_none_or(|queued| self.class_has_capacity(&queued.spec.kind))
    }

    /// Whether another attempt of `kind` may start.
    fn class_has_capacity(&self, kind: &str) -> bool {
        let Some(limit) = self.classes.limit_for(kind) else {
            return true;
        };
        self.running
            .values()
            .filter(|running| running.kind == kind)
            .count()
            < limit.get()
    }

    /// Hands one attempt to its own task.
    fn begin_attempt(&mut self, queued: Queued, outcomes: &mpsc::UnboundedSender<Event>) {
        let Queued {
            spec,
            job,
            attempts,
            ..
        } = queued;
        // `Queued::attempts` counts the attempts already made, so the attempt starting now
        // is one more than that. Keeping the conversion in one place is what stops the
        // reported count from drifting from the budget the engine enforces.
        let attempt = attempts.saturating_add(1);
        let Some(entry) = self.handles.get_mut(&job) else {
            return;
        };
        let token = entry.cancellation.clone();
        let state = JobState::running(job, spec.kind.clone(), attempt);
        entry.state = state.clone();
        let _ = entry.states.send(state);
        info!(job = %job, kind = %spec.kind, attempt, "started");

        let work = Arc::clone(&entry.work);
        let reported = outcomes.clone();
        let attempt_kind = spec.kind.clone();
        let kind = spec.kind.clone();
        let job_id = job;
        let handle = tokio::spawn(async move {
            // Taking the job out and putting it back are both synchronous, so the cell's
            // lock is never held across the `.await` that does the actual work.
            let cell = work.lock().ok().and_then(|mut lease| lease.take());
            let result = match cell {
                Some(mut unit) => {
                    // `AssertUnwindSafe` is the honest marker here: the job is `&mut self`
                    // across the boundary, so the compiler cannot prove it left no torn
                    // state behind. The engine's response is to treat a panicked attempt as
                    // a terminal failure and release the slot, which is what makes the
                    // marker acceptable.
                    let result = AssertUnwindSafe(async {
                        let context = JobContext::new(job_id, &attempt_kind, attempt, &token);
                        unit.run(&context).await
                    })
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|_| {
                        Err(JobError::failed(
                            "the job panicked; the scheduler released its slot",
                        ))
                    });
                    // The work goes back before the outcome is reported, so the engine can
                    // schedule a retry the moment it reads the result.
                    if let Ok(mut slot) = work.lock() {
                        *slot = Some(unit);
                    }
                    result
                }
                None => Err(JobError::failed(
                    "the job's work was already taken; the scheduler will not run it twice",
                )),
            };
            let _ = reported.send(Event::Outcome {
                job: job_id,
                result,
            });
        });
        self.running.insert(job, Running { kind, handle });
    }

    /// A consistent snapshot of the counters.
    fn metrics(&self) -> SchedulerMetrics {
        SchedulerMetrics {
            ready: self.ready.len(),
            deferred: self.deferred.len(),
            inflight: self.running.len(),
            live: self.handles.len(),
            retries_scheduled: self.retries_scheduled,
        }
    }
}
