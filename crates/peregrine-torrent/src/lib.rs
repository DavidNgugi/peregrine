#![forbid(unsafe_code)]
//! The `TorrentEngine` port and its librqbit adapter, torrent and magnet
//! ingestion, the safety inspection pipeline, piece priority for streaming, and
//! seeding policy.
//!
//! Inspection completes before a single byte is written, and nothing outside the
//! configured download root is ever touched.
