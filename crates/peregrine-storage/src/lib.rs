#![forbid(unsafe_code)]
//! Migrations, repositories, FTS5 search, KV settings, the job journal, and the
//! cache tables.
//!
//! Repositories carry no business rules, every catalogue and library read takes an
//! `AccountScope`, and all writes go through a single writer connection.
