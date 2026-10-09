#![forbid(unsafe_code)]
//! Entities, value objects, invariants, quality scoring, release-name parsing,
//! subtitle timing and format models, state machines, and domain events.
//!
//! No IO, no async, no clock, and no randomness: every time and ID is injected, so
//! every rule here is testable without a database, a network, or a GUI.

pub mod id;
pub mod path;
pub mod ports;
pub mod quality;
pub mod release;

pub use id::{ExternalId, ExternalIdError, Provider};
pub use path::{MAX_COMPONENT_BYTES, PathGuard, PathGuardError, SafePath};
pub use ports::{Clock, IdGenerator};
pub use quality::{
    GroupRules, QualityProfile, QualityProfileError, QualityRank, QualityRules, SizeCaps,
};
pub use release::{
    AudioCodec, EpisodeSpan, HdrFormat, ReleaseName, ReleaseSource, RepackKind, Resolution,
    VideoCodec,
};
