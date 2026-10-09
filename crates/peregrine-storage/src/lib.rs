#![forbid(unsafe_code)]
//! Migrations, repositories, FTS5 search, KV settings, the job journal, and the
//! cache tables.
//!
//! Repositories carry no business rules, every catalogue and library read takes an
//! `AccountScope`, and all writes go through a single writer connection.
//!
//! [`Database`] is the handle: `open` applies the embedded migrations on the one write
//! connection and hands that connection to a task, while reads are served by a small
//! pool. No `sqlx` type appears in this crate's public surface, so the driver stays an
//! implementation detail of the adapter.

mod database;
mod error;
mod migrations;
mod pragmas;
mod writer;

pub use database::Database;
pub use error::StorageError;
pub use writer::NewEvent;
