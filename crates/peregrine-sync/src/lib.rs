#![forbid(unsafe_code)]
//! The job scheduler: priority queues, resource token buckets, incremental change
//! sync, dataset ingestion, and the image cache.
//!
//! Every job is cooperatively cancellable, and no job holds a write transaction
//! across a network call.
//!
//! The queue is [`Scheduler`]: one task owns the ready heap, the deferred set, the dedupe
//! index, and the running set, and every mutation arrives as a message on one channel —
//! the shape `peregrine-storage`'s writer uses, for the same reason. A caller gets back a
//! [`JobHandle`] that observes the job's lifecycle across however many retries it takes.

mod backoff;
pub mod catalogue;
mod core;
mod error;
mod heap;
mod images;
mod job;
#[cfg(test)]
mod tests;

pub use backoff::Backoff;
pub use catalogue::{
    CacheError, ListedTitle, RefreshedTitle, SyncError, SyncOutcome, cache_image, match_title,
    record_list_answer, refresh_title, store_cast, sync_search, upsert_list_candidate,
};
pub use core::{
    DEFAULT_MAX_ATTEMPTS, DEFAULT_MAX_CONCURRENCY, Scheduler, SchedulerClasses, SchedulerConfig,
    SchedulerMetrics, Submission,
};
pub use error::{JobError, SchedulerError};
pub use heap::JobSpec;
pub use images::{CachedImage, ImageCache, ImageCacheError, ImageKey, ImageVariant};
pub use job::{
    Job, JobCompletion, JobContext, JobFuture, JobHandle, JobKind, JobOutcome, JobOutput,
    JobResult, JobState, JobStateKind,
};
