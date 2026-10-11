#![forbid(unsafe_code)]
//! Migrations, repositories, FTS5 search, KV settings, the job journal, and the
//! cache tables.
//!
//! Repositories carry no business rules, every catalogue and library read takes an
//! `AccountScope`, all writes go through a single writer connection, and every list query
//! is keyset-paginated through [`PageRequest`] — `cargo xtask check-offset` keeps it that
//! way.
//!
//! [`Database`] is the handle: `open` applies the embedded migrations on the one write
//! connection and hands that connection to a task, while reads are served by a small
//! pool. No `sqlx` type appears in this crate's public surface, so the driver stays an
//! implementation detail of the adapter.

mod catalogue;
mod database;
mod error;
mod jobs;
mod migrations;
mod page;
mod pragmas;
mod writer;

pub use catalogue::{
    MetadataState, StubTitle, TitleExternalId, TitleIdentity, TitleKind, TitleSubtype,
};
pub use database::Database;
pub use error::StorageError;
pub use jobs::{JobChange, JobPhase, JobRecord, NewJob};
pub use page::{Cursor, MAX_PAGE_ROWS, PageRequest};
pub use writer::NewEvent;
