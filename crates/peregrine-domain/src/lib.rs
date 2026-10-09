#![forbid(unsafe_code)]
//! Entities, value objects, invariants, quality scoring, release-name parsing,
//! subtitle timing and format models, state machines, and domain events.
//!
//! No IO, no async, no clock, and no randomness: every time and ID is injected, so
//! every rule here is testable without a database, a network, or a GUI.
