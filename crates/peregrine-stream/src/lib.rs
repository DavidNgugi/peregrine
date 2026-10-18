#![forbid(unsafe_code)]
//! The loopback and LAN media servers: byte-range delivery, subtitle sidecars,
//! cast streams, token authentication, per-session lifetime, and bandwidth
//! accounting.
//!
//! Loopback and LAN listeners are separate bind addresses with separate token
//! scopes, and the LAN listener exists only while casting.
//!
//! **What exists today is the loopback half, for one file at a time.** [`MediaServer`] binds `127.0.0.1`
//! on an ephemeral port and serves exactly the file it was opened for — a library file through
//! `ServeFile`, or a file whose download is still running through the engine — and stops when its
//! session does. That is what lets a webview `<video>` element play without the asset protocol
//! exposing the library root (ADR [0023](../../../docs/adr/0023-in-app-video-surface.md), SECURITY.md
//! T10).
//! on an ephemeral port, serves exactly the file it was opened for, and stops when its session does —
//! which is what lets a webview `<video>` element play a library file without the asset protocol
//! exposing the library root (ADR [0023](../../../docs/adr/0023-in-app-video-surface.md), SECURITY.md
//! T10).
//!
//! **Sidecar subtitles are served from the same listener, under the same token.** A session is opened
//! with the sidecar files it may serve — each already a `SafePath` — and the route names a
//! `subtitle_track` row id rather than a file, so a request cannot ask for a path the session was not
//! opened with.
//!
//! Still owed here: the LAN listener and its separate token scope (`PG-E19`), cast streams (`PG-E18`),
//! and bandwidth accounting (`PG-E10-16`).

mod error;
mod server;
mod thumbnail;

pub use error::StreamError;
pub use server::{Capture, MediaReader, MediaServer, SidecarSubtitle, TorrentMedia};
pub use thumbnail::{ThumbnailError, ThumbnailSink};
