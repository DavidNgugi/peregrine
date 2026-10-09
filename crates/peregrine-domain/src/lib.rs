#![forbid(unsafe_code)]
//! Entities, value objects, invariants, quality scoring, release-name parsing,
//! subtitle timing and format models, state machines, and domain events.
//!
//! No IO, no async, no clock, and no randomness: every time and ID is injected, so
//! every rule here is testable without a database, a network, or a GUI.

pub mod id;
pub mod ports;
pub mod release;

pub use id::{ExternalId, ExternalIdError, Provider};
pub use ports::{Clock, IdGenerator};
pub use release::{
    AudioCodec, EpisodeSpan, HdrFormat, ReleaseName, ReleaseSource, RepackKind, Resolution,
    VideoCodec,
};
