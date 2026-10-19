#![forbid(unsafe_code)]
//! The `TorrentEngine` port and its librqbit adapter, torrent and magnet
//! ingestion, the safety inspection pipeline, piece priority for streaming, and
//! seeding policy.
//!
//! Inspection completes before a single byte is written, and nothing outside the
//! configured download root is ever touched.
//!
//! # What is here and what is not
//!
//! The port is here: [`TorrentEngine`], the [`EngineDescriptor`] every engine publishes for
//! Diagnostics, the [`AddRequest`] whose only constructor cannot be built without an allowed
//! safety report, the [`AddIntent`] that says whether a payload is already on disk, the
//! [`EngineLimits`] a running session applies without a restart, the [`EngineError`] and
//! [`AddRefused`] vocabularies, and the [`TorrentStats`] a progress row draws. Those are what
//! PG-E10-01 asks for, and the conformance suite that runs over them lives in `peregrine-testkit`.
//!
//! **The adapter is here.** [`LibrqbitEngine`] implements the port over a librqbit session, and
//! [`TorrentMetadataFetch`] is the metadata-only fetch `PG-E10-03` asks for. No librqbit type
//! reaches a public signature, and the adapter's module doc records the exact 9.0.1 API it is
//! written against and the things that engine cannot do. A second adapter (libtorrent bindings,
//! ADR 0005) stays a leaf-crate change.
//!
//! Nor is the rest of the epic: the per-file selection the adapter applies on disk (PG-E10-04),
//! the sequential read-ahead window and the piece-priority driver that moves it (PG-E10-05,
//! PG-E10-06), the statistics stream (PG-E10-07), reattachment from a stored [`EngineState`]
//! (PG-E10-08), the seeding policy that decides when a finished torrent stops (PG-E10-09), the
//! runtime application of [`EngineLimits`] (PG-E10-10), and proxy and interface binding
//! (PG-E10-11).
//!
//! The safety inspection itself is also not here: it is
//! [`peregrine_domain::inspect::PayloadInspector`], which judges a torrent's description on
//! metadata alone. This crate's [`AddRequest`] is where that verdict becomes structural — there is
//! no spelling of an add that does not carry one — which is SECURITY T12 and `FLOWS` F6 as a type
//! rather than as a rule for every caller to remember.

pub mod engine;
pub mod error;
pub mod librqbit;
pub mod stats;

pub use engine::{
    AddIntent, AddRequest, EngineDescriptor, EngineFuture, EngineLimits, EngineState,
    FetchedMetadata, FileSelection, StreamChunk, StreamRequest, TorrentEngine, TorrentHandle,
    TorrentMetadataFetch, TorrentSource, TorrentStream,
};
pub use error::{AddRefused, EngineError};
pub use librqbit::LibrqbitEngine;
pub use stats::{ByteRate, Bytes, PeerCounts, Ratio, TorrentHealth, TorrentStats, Transfer};
