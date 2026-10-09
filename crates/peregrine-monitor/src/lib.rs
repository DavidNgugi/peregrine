#![forbid(unsafe_code)]
//! The `Monitor` port and the watching policy: per-scope monitors, the
//! availability state machine, evidence-based transitions, and the daily digest.
//!
//! A transition without recorded evidence is a bug. Enqueues jobs rather than
//! scheduling its own polls. Post-1.0: see docs/adr/0016.
