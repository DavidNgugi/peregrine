#![forbid(unsafe_code)]
//! Keyring reads and writes, in-memory secret wrappers, and the log redaction
//! filter.
//!
//! A secret value is never debug-printed, never serialized to the database, and
//! never included in an error string.
//!
//! The keyring is behind [`SecretStore`], and which settings hold a secret is a
//! question for the settings registry (`PG-E20-04`), not for this module.

mod redact;
mod secret;
pub mod store;

pub use redact::RedactingFormat;
pub use secret::Secret;
pub use store::{KeyringStore, SecretKey, SecretStore, SecretStoreError};
