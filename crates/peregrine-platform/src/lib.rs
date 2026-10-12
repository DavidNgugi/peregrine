#![forbid(unsafe_code)]
//! Per-OS path resolution, permission requests, tray, dock and taskbar badges,
//! autostart, notifications, power-save inhibition, the OS authentication
//! adapters, and sidecar resolution and verification.
//!
//! One cfg-gated module per OS behind a common interface. An adapter reports which
//! mechanism answered, and reports unavailability rather than faking success.
//!
//! The production [`Clock`](peregrine_domain::Clock) and
//! [`IdGenerator`](peregrine_domain::IdGenerator) live here for the same reason: the
//! domain may not read a real time or a random source, so the two adapters that do are
//! leaves.
//!
//! [`sidecar`] is here because resolving and hashing a binary is IO, and because a
//! component that fails verification has to become a named missing capability
//! rather than an error surfaced mid-job.

pub mod clock;
pub mod ids;
pub mod sidecar;

pub use clock::SystemClock;
pub use ids::UlidGenerator;
pub use sidecar::VerifiedSidecar;
