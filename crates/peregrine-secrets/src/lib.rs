#![forbid(unsafe_code)]
//! Keyring reads and writes, in-memory secret wrappers, and the log redaction
//! filter.
//!
//! A secret value is never debug-printed, never serialized to the database, and
//! never included in an error string.
//!
//! Keyring storage is not implemented here yet: it arrives with the settings
//! registry in `PG-E20-04`, which is what names the settings that hold secrets.

mod redact;
mod secret;

pub use redact::RedactingFormat;
pub use secret::Secret;
