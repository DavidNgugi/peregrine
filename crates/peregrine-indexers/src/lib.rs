#![forbid(unsafe_code)]
//! The `Indexer` port and its Prowlarr, Jackett, local-file, and built-in
//! adapters, result normalization, and info-hash de-duplication.
//!
//! Adapters ship disabled with no bundled default domains, and no rate-limited
//! endpoint is ever polled on a schedule.
