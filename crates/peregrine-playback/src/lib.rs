#![forbid(unsafe_code)]
//! The `PlayerBackend` port and its mpv adapter, the session state machine, the
//! probe cache, resume policy, next-up, watch-progress reporting, and sidecar
//! orchestration.
//!
//! No decoding happens in-process, and position writes are throttled and
//! coalesced.
//!
//! **What exists today is the session and its loopback listener** — the first implementation of the
//! video surface ADR [0023](../../../docs/adr/0023-in-app-video-surface.md) decides. The bytes travel
//! over loopback HTTP and the webview's own media stack decodes them, so nine of the ten files in a real
//! library play with no sidecar, no download, and no media code in this process.
//!
//! **Still owed here, named rather than implied:** the `PlayerBackend` port and its mpv adapter, the
//! probe cache, resume policy, next-up, watch-progress reporting, and ffmpeg orchestration
//! (`PG-E16-01`…`PG-E16-22`). [`PlaybackSession`] is written so that the mpv adapter answers the same
//! question with a surface instead of a URL, which is what keeps a screen from having to know which one
//! is live.

mod session;

pub use session::{PlaybackError, PlaybackSession};
