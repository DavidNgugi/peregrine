#![forbid(unsafe_code)]
//! The `PlayerBackend` port and its mpv adapter, the session state machine, the
//! probe cache, resume policy, next-up, watch-progress reporting, and sidecar
//! orchestration.
//!
//! No decoding happens in-process, and position writes are throttled and
//! coalesced.
