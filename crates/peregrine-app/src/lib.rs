#![forbid(unsafe_code)]
//! The services that compose the ports into use cases: library, discovery,
//! acquisition, playback, subtitles, settings, casting, notifications, accounts,
//! and maintenance.
//!
//! This is the only place where multiple ports are combined.

mod observability;

pub use observability::{LogConfig, Observability, ObservabilityError};
