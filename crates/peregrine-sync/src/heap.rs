//! The queue's ordering.
//!
//! `BinaryHeap` pops its greatest element, so "greatest" is defined here as the job that
//! should run first: the highest priority, and within one priority the earliest
//! submission. A deferred job waits in a second heap ordered by deadline instead, so a
//! retry that is not due yet can never be popped ahead of ready work.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use tokio::time::Instant;
use ulid::Ulid;

use crate::job::JobKind;

/// The submission order that breaks ties within a priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Sequence(u64);

impl Sequence {
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }
}

/// The fields the queue orders and retries by, separate from the job itself.
///
/// A retry re-enters the heap using this value, so a job's priority and dedupe key are
/// fixed for its whole life: a job cannot change its own priority by deferring.
#[derive(Debug, Clone)]
pub struct JobSpec {
    /// The caller's name for the work, used in logs.
    pub kind: JobKind,
    /// Higher runs first; equal priorities are FIFO.
    pub priority: i32,
    /// When set, jobs that share this key coalesce onto one live job.
    ///
    /// `None` never dedupes, which matches how the schema treats a nullable key: an
    /// absent key is not a wildcard that every keyless job shares.
    pub dedupe_key: Option<String>,
    /// How many attempts, including the first, the job may make before it is abandoned.
    pub max_attempts: u32,
}

/// A job the engine is holding, ready or waiting for its retry deadline.
///
/// The work itself is *not* here: it lives in the job's handle entry so that one boxed
/// job survives a deferral and runs again. An entry that owned the work could only run
/// its first attempt, because starting an attempt moves the work into its task.
pub(crate) struct Queued {
    pub(crate) spec: JobSpec,
    pub(crate) job: Ulid,
    pub(crate) sequence: Sequence,
    pub(crate) attempts: u32,
    pub(crate) ready_at: Instant,
}

/// Jobs the engine may start right now.
#[derive(Default)]
pub(crate) struct JobHeap {
    entries: BinaryHeap<Queued>,
}

impl JobHeap {
    /// Adds a job, including a retry that is ready to run again.
    pub(crate) fn push(&mut self, entry: Queued) {
        self.entries.push(entry);
    }

    /// The job that should run next, if anything is waiting.
    pub(crate) fn peek(&self) -> Option<&Queued> {
        self.entries.peek()
    }

    /// Pops the job that should run next, if anything is waiting.
    pub(crate) fn pop(&mut self) -> Option<Queued> {
        self.entries.pop()
    }

    /// Whether anything is waiting.
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many jobs are waiting.
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Extracts the job with `job` from anywhere in the heap.
    ///
    /// A cancellation names a job, not a position, and a queue of this size is small
    /// enough that a linear rebuild costs less than a position index that every retry
    /// would have to repair.
    pub(crate) fn remove(&mut self, job: Ulid) -> Option<Queued> {
        let mut removed = None;
        let drained: Vec<Queued> = self.entries.drain().collect();
        for entry in drained {
            if entry.job == job {
                removed = Some(entry);
            } else {
                self.entries.push(entry);
            }
        }
        removed
    }
}

impl Ord for Queued {
    fn cmp(&self, other: &Self) -> Ordering {
        // Highest priority first, then lowest sequence first, which is FIFO.
        self.spec
            .priority
            .cmp(&other.spec.priority)
            .then_with(|| other.sequence.0.cmp(&self.sequence.0))
    }
}

impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Queued {
    fn eq(&self, other: &Self) -> bool {
        self.spec.priority == other.spec.priority && self.sequence == other.sequence
    }
}

impl Eq for Queued {}

/// Jobs waiting for a retry deadline.
#[derive(Default)]
pub(crate) struct DeferredHeap {
    entries: BinaryHeap<std::cmp::Reverse<(Instant, Ulid, Queued)>>,
}

impl DeferredHeap {
    /// Adds a job that is not due yet.
    ///
    /// The deadline is the heap key and the ULID breaks ties, which `Instant` alone
    /// cannot: two jobs deferred in the same nanosecond would otherwise share an order.
    pub(crate) fn push(&mut self, entry: Queued) {
        self.entries
            .push(std::cmp::Reverse((entry.ready_at, entry.job, entry)));
    }

    /// How many jobs are waiting for a retry.
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// The earliest deadline, which is what the engine arms its timer for.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.entries.peek().map(|entry| entry.0.0)
    }

    /// Moves every job whose deadline has passed into `ready`.
    ///
    /// Returns how many moved, so the caller knows whether running the queue again can
    /// make progress.
    pub(crate) fn take_due(&mut self, now: Instant, ready: &mut JobHeap) -> usize {
        let mut moved = 0;
        while let Some(std::cmp::Reverse((deadline, _, _))) = self.entries.peek() {
            if *deadline > now {
                break;
            }
            if let Some(std::cmp::Reverse((_, _, entry))) = self.entries.pop() {
                ready.push(entry);
                moved += 1;
            }
        }
        moved
    }

    /// Extracts the job with `job` from anywhere in the heap.
    pub(crate) fn remove(&mut self, job: Ulid) -> Option<Queued> {
        let mut removed = None;
        let drained: Vec<std::cmp::Reverse<(Instant, Ulid, Queued)>> =
            self.entries.drain().collect();
        for std::cmp::Reverse((deadline, id, entry)) in drained {
            if id == job {
                removed = Some(entry);
            } else {
                self.entries.push(std::cmp::Reverse((deadline, id, entry)));
            }
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(priority: i32) -> JobSpec {
        JobSpec {
            kind: "test.noop".to_owned(),
            priority,
            dedupe_key: None,
            max_attempts: 1,
        }
    }

    fn queued(priority: i32, sequence: u64) -> Queued {
        Queued {
            spec: spec(priority),
            job: Ulid::from_parts(0, u128::from(sequence)),
            sequence: Sequence::new(sequence),
            attempts: 0,
            ready_at: Instant::now(),
        }
    }

    #[test]
    fn higher_priority_pops_first_and_equal_priority_is_fifo() {
        let mut heap = JobHeap::default();
        heap.push(queued(1, 0));
        heap.push(queued(5, 1));
        heap.push(queued(1, 2));
        heap.push(queued(5, 3));

        let order: Vec<u64> = std::iter::from_fn(|| heap.pop())
            .map(|entry| entry.sequence.0)
            .collect();
        assert_eq!(
            order,
            vec![1, 3, 0, 2],
            "priority first, then submission order"
        );
    }

    #[test]
    fn cancelling_a_queued_job_removes_only_that_job() {
        let mut heap = JobHeap::default();
        for sequence in 0..4 {
            heap.push(queued(1, sequence));
        }
        let removed = heap.remove(Ulid::from_parts(0, 2));
        assert_eq!(removed.map(|entry| entry.sequence.0), Some(2));
        assert_eq!(heap.len(), 3);
        assert!(
            heap.remove(Ulid::from_parts(0, 2)).is_none(),
            "removal is once"
        );
    }

    #[test]
    fn a_deferred_job_is_not_ready_until_its_deadline() {
        let now = Instant::now();
        let mut deferred = DeferredHeap::default();
        let mut ready = JobHeap::default();
        let mut late = queued(1, 0);
        late.ready_at = now + std::time::Duration::from_secs(10);
        deferred.push(late);

        assert_eq!(deferred.take_due(now, &mut ready), 0);
        assert!(ready.is_empty());
        assert_eq!(
            deferred.take_due(now + std::time::Duration::from_secs(9), &mut ready),
            0
        );
        assert_eq!(
            deferred.take_due(now + std::time::Duration::from_secs(10), &mut ready),
            1
        );
        assert_eq!(ready.len(), 1);
    }
}
