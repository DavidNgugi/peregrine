#![forbid(unsafe_code)]
//! Migrations, repositories, FTS5 search, the maintenance job, KV settings, the job journal,
//! and the cache tables.
//!
//! Repositories carry no business rules, every catalogue and library read takes an
//! `AccountScope`, all writes go through a single writer connection, and every list query
//! is keyset-paginated through [`PageRequest`] — `cargo xtask check-offset` keeps it that
//! way.
//!
//! [`Database`] is the handle: `open` checks an existing file with `PRAGMA integrity_check`
//! (quarantining it and rebuilding when it is damaged), copies it before applying the first
//! pending migration, and hands the write connection to a task while reads are served by a
//! small pool. [`Database::recovery`] reports what that check found. No `sqlx` type appears
//! in this crate's public surface, so the driver stays an implementation detail of the
//! adapter.

mod backup;
mod catalogue;
mod database;
mod error;
mod fts;
mod image_cache;
mod integrity;
mod jobs;
mod json;
mod library;
mod maintenance;
mod migrations;
mod notify;
mod page;
mod pragmas;
mod retention;
#[cfg(test)]
mod test_support;
mod usage;
mod watch;
mod writer;

pub use catalogue::{
    MetadataState, StubTitle, TitleExternalId, TitleIdentity, TitleKind, TitleSubtype,
};
pub use database::Database;
pub use error::StorageError;
pub use image_cache::ImageCacheEntry;
pub use integrity::Recovery;
pub use jobs::{JobChange, JobPhase, JobRecord, NewJob};
pub use library::{LibraryRootRow, MediaFileRow, MediaItemRow, SubtitleTrackRow, UnmatchedFileRow};
pub use maintenance::{
    MaintenanceOutcome, MaintenancePlan, MaintenanceRequest, MaintenanceStep, StepOutcome,
    StepRecord,
};
pub use notify::NotificationRepository;
pub use page::{Cursor, MAX_PAGE_ROWS, PageRequest};
pub use retention::{
    RETENTION_POLICY, Requirement, RetentionFailure, RetentionOutcome, RetentionPlan,
    RetentionPolicyEntry, RetentionRule, TableRemoval, TimeWindow,
};
pub use usage::{DiskUsage, LibraryUsageRow, MeasuredFrom, Tier, TierUsage};
pub use watch::{WatchProgressRow, WatchlistRow};
pub use writer::NewEvent;
