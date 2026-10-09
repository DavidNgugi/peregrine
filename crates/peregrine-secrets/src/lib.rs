#![forbid(unsafe_code)]
//! Keyring reads and writes, in-memory secret wrappers, and the log redaction
//! filter.
//!
//! A secret value is never debug-printed, never serialized to the database, and
//! never included in an error string.
