//! What a caller hands the scheduler, and what the scheduler hands back.
//!
//! The handle is the only thing a caller holds: the engine owns the queue, the running
//! set, and the dedupe index, so a caller cannot observe a half-applied transition.

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use ulid::Ulid;

use crate::error::JobError;

/// The name a caller gives its work.
///
/// The scheduler only uses it for logging and for grouping in a snapshot, so it is free
/// text; it must never contain a secret (ARCHITECTURE §12). It is owned because a job
/// spec outlives the call that built it.
pub type JobKind = String;

/// What a job is allowed to do.
///
/// The context carries no queue state, and in particular no way to enqueue: a job that
/// could enqueue is a job that can deadlock the engine from inside it.
#[derive(Debug)]
pub struct JobContext<'a> {
    job: Ulid,
    kind: &'a str,
    attempt: u32,
    cancellation: &'a CancellationToken,
}

impl<'a> JobContext<'a> {
    pub(crate) const fn new(
        job: Ulid,
        kind: &'a str,
        attempt: u32,
        cancellation: &'a CancellationToken,
    ) -> Self {
        Self {
            job,
            kind,
            attempt,
            cancellation,
        }
    }

    /// The job's identity, stable across its retries.
    pub const fn id(&self) -> Ulid {
        self.job
    }

    /// The kind the caller supplied at submission.
    pub const fn kind(&self) -> &'a str {
        self.kind
    }

    /// Which attempt this is, counted from one.
    pub const fn attempt(&self) -> u32 {
        self.attempt
    }

    /// The job's cancellation token.
    ///
    /// `PG-E6-01` requires cooperative cancellation, so a job selects on this token at
    /// every await point where stopping early is safe. A job that ignores it is still
    /// reported as cancelled once it returns, but it holds its slot until then.
    pub const fn cancellation(&self) -> &'a CancellationToken {
        self.cancellation
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
}

/// The value a successful job produces.
///
/// `Any` lets the scheduler stay generic over payloads without a type parameter that
/// every caller would have to name; the queue moves the value but never inspects it.
pub type JobOutput = Box<dyn Any + Send>;

/// What one attempt of a job produced.
pub type JobResult = Result<JobOutput, JobError>;

/// The future one attempt runs.
///
/// It is boxed because the queue stores jobs behind `dyn Job`, and a trait with a
/// return-position `impl Trait` method is not dyn compatible on this toolchain. One
/// allocation per attempt is nothing next to the IO the attempt performs.
pub type JobFuture<'a> = Pin<Box<dyn Future<Output = JobResult> + Send + 'a>>;

/// A unit of deferred work.
///
/// `run` is called once per attempt and owns its retry decision: returning
/// [`JobError::Retryable`] defers the job and the scheduler reschedules it with backoff,
/// [`JobError::Cancelled`] reports cooperative cancellation, and [`JobError::Failed`]
/// ends the job.
pub trait Job: Send + 'static {
    /// Performs the work of one attempt.
    ///
    /// The future must be `Send` because the engine runs each attempt as its own task,
    /// which is what lets one attempt panic without taking the queue down with it.
    fn run(&mut self, context: &JobContext<'_>) -> JobFuture<'_>;
}

/// The externally visible lifecycle of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStateKind {
    /// Accepted and waiting for a worker slot.
    Queued,
    /// An attempt is executing.
    Running,
    /// The last attempt deferred and a retry is scheduled.
    Deferred,
    /// Terminal: the last attempt succeeded.
    Succeeded,
    /// Terminal: the last attempt failed, or panicked, and no retry remains.
    Failed,
    /// Terminal: cancellation was observed, or the job never started.
    Cancelled,
}

/// A snapshot of one job's lifecycle.
///
/// Snapshots are values, not a stream: a caller that cares about a transition waits for
/// the state it wants rather than diffing successive snapshots.
#[derive(Debug, Clone)]
pub struct JobState {
    job: Ulid,
    kind: JobKind,
    attempts: u32,
    state: State,
}

#[derive(Debug, Clone)]
enum State {
    Queued,
    Running,
    Deferred {
        next_run_at: Instant,
        deferred_at: jiff::Timestamp,
        delay: Duration,
    },
    Succeeded,
    Failed(JobError),
    Cancelled,
}

/// The scheduler's final answer for one job.
#[derive(Debug)]
pub(crate) struct Resolution {
    pub(crate) outcome: Outcome,
}

#[derive(Debug)]
pub(crate) enum Outcome {
    Succeeded(JobOutput),
    Failed(JobError),
    Cancelled,
}

impl JobState {
    pub(crate) const fn queued(job: Ulid, kind: JobKind, attempts: u32) -> Self {
        Self {
            job,
            kind,
            attempts,
            state: State::Queued,
        }
    }

    pub(crate) const fn running(job: Ulid, kind: JobKind, attempts: u32) -> Self {
        Self {
            job,
            kind,
            attempts,
            state: State::Running,
        }
    }

    pub(crate) fn deferred(
        job: Ulid,
        kind: JobKind,
        attempts: u32,
        deferred_at: jiff::Timestamp,
        next_run_at: Instant,
        delay: Duration,
    ) -> Self {
        Self {
            job,
            kind,
            attempts,
            state: State::Deferred {
                next_run_at,
                deferred_at,
                delay,
            },
        }
    }

    pub(crate) const fn succeeded(job: Ulid, kind: JobKind, attempts: u32) -> Self {
        Self {
            job,
            kind,
            attempts,
            state: State::Succeeded,
        }
    }

    pub(crate) fn failed(job: Ulid, kind: JobKind, attempts: u32, error: JobError) -> Self {
        Self {
            job,
            kind,
            attempts,
            state: State::Failed(error),
        }
    }

    pub(crate) const fn cancelled(job: Ulid, kind: JobKind, attempts: u32) -> Self {
        Self {
            job,
            kind,
            attempts,
            state: State::Cancelled,
        }
    }

    /// The job's identity.
    pub const fn id(&self) -> Ulid {
        self.job
    }

    /// The kind the caller supplied at submission.
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// How many attempts have been made, counted from zero while the job has not started.
    pub const fn attempts(&self) -> u32 {
        self.attempts
    }

    /// The lifecycle phase.
    pub const fn phase(&self) -> JobStateKind {
        match self.state {
            State::Queued => JobStateKind::Queued,
            State::Running => JobStateKind::Running,
            State::Deferred { .. } => JobStateKind::Deferred,
            State::Succeeded => JobStateKind::Succeeded,
            State::Failed(_) => JobStateKind::Failed,
            State::Cancelled => JobStateKind::Cancelled,
        }
    }

    /// Whether the job will not run again.
    pub const fn is_terminal(&self) -> bool {
        matches!(
            self.state,
            State::Succeeded | State::Failed(_) | State::Cancelled
        )
    }

    /// When the next attempt is due, while the job is deferred.
    ///
    /// `PG-E6-01` requires the retry schedule to be observable. This deadline is the
    /// monotonic instant the engine will wait on, so a test can assert the retry fires
    /// at that instant and not before.
    pub const fn next_run_at(&self) -> Option<Instant> {
        match self.state {
            State::Deferred { next_run_at, .. } => Some(next_run_at),
            _ => None,
        }
    }

    /// The delay the pending retry was scheduled with.
    pub const fn retry_delay(&self) -> Option<Duration> {
        match self.state {
            State::Deferred { delay, .. } => Some(delay),
            _ => None,
        }
    }

    /// When the deferral was decided, from the injected clock.
    ///
    /// Adding [`JobState::retry_delay`] to this gives the wall-clock instant the retry is
    /// due, which is what a UI shows and a journal stores; `next_run_at` is the monotonic
    /// deadline the engine itself waits on.
    pub const fn deferred_at(&self) -> Option<jiff::Timestamp> {
        match self.state {
            State::Deferred { deferred_at, .. } => Some(deferred_at),
            _ => None,
        }
    }

    /// The failure that ended the job, when it failed.
    pub const fn failure(&self) -> Option<&JobError> {
        match &self.state {
            State::Failed(error) => Some(error),
            _ => None,
        }
    }
}

/// The scheduler's answer for one job.
#[derive(Debug)]
pub enum JobOutcome {
    /// The job finished its work.
    Succeeded(JobOutput),
    /// The job failed permanently, or panicked, and will not run again. The job's
    /// attempt count is on the state that accompanied this outcome.
    Failed(JobError),
    /// The job was cancelled, either before it started or while running.
    Cancelled,
}

/// The outcome and the attempt count that produced it.
#[derive(Debug)]
pub struct JobCompletion {
    outcome: JobOutcome,
    attempts: u32,
}

impl JobCompletion {
    pub(crate) const fn new(outcome: JobOutcome, attempts: u32) -> Self {
        Self { outcome, attempts }
    }

    /// What happened.
    pub const fn outcome(&self) -> &JobOutcome {
        &self.outcome
    }

    /// How many attempts were made in total.
    pub const fn attempts(&self) -> u32 {
        self.attempts
    }

    /// Consumes the completion and returns what happened.
    pub fn into_outcome(self) -> JobOutcome {
        self.outcome
    }

    /// Consumes the completion and returns the successful value, if there is one.
    pub fn succeeded(self) -> Option<JobOutput> {
        match self.outcome {
            JobOutcome::Succeeded(output) => Some(output),
            JobOutcome::Failed(_) | JobOutcome::Cancelled => None,
        }
    }
}

/// A caller's handle to one submitted job.
///
/// **Dropping a handle does not cancel the job.** `ARCHITECTURE.md` §7 rule 6 makes this
/// explicit: cancellation is requested, never implied, so dropping the last handle to a
/// running job leaves it running to completion and the engine simply has nobody left to
/// notify. A caller that wants the job to stop calls [`JobHandle::cancel`].
#[derive(Debug)]
pub struct JobHandle {
    job: Ulid,
    kind: JobKind,
    cancellation: CancellationToken,
    states: watch::Receiver<JobState>,
    completion: oneshot::Receiver<Resolution>,
    cancels: Option<mpsc::Sender<Ulid>>,
}

impl JobHandle {
    pub(crate) const fn new(
        job: Ulid,
        kind: JobKind,
        cancellation: CancellationToken,
        states: watch::Receiver<JobState>,
        completion: oneshot::Receiver<Resolution>,
        cancels: mpsc::Sender<Ulid>,
    ) -> Self {
        Self {
            job,
            kind,
            cancellation,
            states,
            completion,
            cancels: Some(cancels),
        }
    }

    /// The job's identity.
    pub const fn id(&self) -> Ulid {
        self.job
    }

    /// The kind the caller supplied at submission.
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// The latest observed state.
    pub fn state(&self) -> JobState {
        self.states.borrow().clone()
    }

    /// The job's cancellation token.
    pub const fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    /// Waits for the job's final answer.
    ///
    /// A deferred job keeps this future pending across its retries; only a terminal
    /// state resolves it.
    pub async fn wait(self) -> JobCompletion {
        match self.completion.await {
            Ok(resolution) => {
                let attempts = self.states.borrow().attempts();
                JobCompletion::new(resolution.outcome.into_public(), attempts)
            }
            // The engine is the only sender, and it resolves every live job before it
            // exits, so a closed channel means the caller is already gone. Reporting a
            // cancellation is the honest answer rather than a hang.
            Err(_) => JobCompletion::new(JobOutcome::Cancelled, self.states.borrow().attempts()),
        }
    }

    /// Waits for the next state transition and returns the new state.
    ///
    /// Returns `None` once the engine has dropped the job, which happens only after a
    /// terminal state was published.
    pub async fn changed(&mut self) -> Option<JobState> {
        self.states.changed().await.ok()?;
        Some(self.states.borrow().clone())
    }

    /// Resolves as soon as the job's cancellation token is tripped.
    ///
    /// This fires for a running job while its own code is still winding down, which is
    /// what makes a cancellation observable before the attempt returns.
    pub async fn cancelled(&self) {
        self.cancellation.cancelled().await;
    }

    /// Requests cancellation.
    ///
    /// Two things happen, and both are needed. The token is tripped immediately so a
    /// running job sees it at its next await point without waiting for the engine. The
    /// engine is also told the job's id, because a *queued* job must stop existing rather
    /// than take a worker slot only to fail its first cancellation check. Both are
    /// idempotent, and cancelling a finished job does nothing.
    pub fn cancel(&mut self) {
        self.cancellation.cancel();
        if let Some(cancels) = self.cancels.as_ref() {
            // A bounded channel that is momentarily full must not make cancellation a
            // blocking operation: the token is already tripped, so the engine will
            // observe it even if this particular message is dropped. The engine also
            // re-checks the token before it starts an attempt.
            match cancels.try_send(self.job) {
                Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    self.cancels = None;
                }
            }
        }
    }
}

impl Resolution {
    pub(crate) const fn new(outcome: Outcome) -> Self {
        Self { outcome }
    }
}

impl Outcome {
    /// Erases the engine-private wrapper so callers see only public types.
    fn into_public(self) -> JobOutcome {
        match self {
            Self::Succeeded(output) => JobOutcome::Succeeded(output),
            Self::Failed(error) => JobOutcome::Failed(error),
            Self::Cancelled => JobOutcome::Cancelled,
        }
    }
}
