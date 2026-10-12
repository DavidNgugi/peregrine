#![forbid(unsafe_code)]
//! Per-OS path resolution, permission requests, tray, dock and taskbar badges,
//! autostart, notifications, power-save inhibition, the OS authentication
//! adapters, and sidecar resolution and verification.
//!
//! One cfg-gated module per OS behind a common interface. An adapter reports which
//! mechanism answered, and reports unavailability rather than faking success.
//!
//! The production [`Clock`](peregrine_domain::Clock) lives here for the same reason:
//! the domain may not read a real time, so the one adapter that does is a leaf.
//!
//! [`sidecar`] is here because resolving and hashing a binary is IO, and because a
//! component that fails verification has to become a named missing capability
//! rather than an error surfaced mid-job.

pub mod clock;
pub mod sidecar;

pub use clock::SystemClock;
pub use sidecar::VerifiedSidecar;
