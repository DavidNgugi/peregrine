#![forbid(unsafe_code)]
//! The job scheduler: priority queues, resource token buckets, incremental change
//! sync, dataset ingestion, and the image cache.
//!
//! Every job is cooperatively cancellable, and no job holds a write transaction
//! across a network call.
