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
//! **What also exists now is the external hand-off:** [`MpvPlayer`] spawns the operating system's
//! mpv with a fixed flag set for a container the webview cannot demux, and stops the child it
//! started. It is deliberately not the embedded surface ADR 0023 §4 describes — mpv opens its own
//! window — and the probe cache, resume policy, next-up, watch-progress reporting, and ffmpeg
//! orchestration remain owed (`PG-E16-02`…`PG-E16-22`). [`PlaybackSession`] is written so that a
//! future embedded adapter answers the same question with a surface instead of a URL, which is what
//! keeps a screen from having to know which one is live.

pub mod chapters;
mod mpv;
mod session;

pub use mpv::MpvPlayer;
pub use session::{PlaybackError, PlaybackSession};
