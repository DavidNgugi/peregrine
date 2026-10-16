#![forbid(unsafe_code)]
//! The `Indexer` port and its Prowlarr adapter.
//!
//! # Source
//!
//! Prowlarr v1 search is `GET /api/v1/search` with the query parameters `query`, `type`,
//! `indexerIds`, `categories`, `limit`, and `offset`, authenticated by an `X-Api-Key`
//! header (the spec also accepts an `apikey` query parameter; this adapter uses the header so
//! the key never reaches a URL). The response is a `ReleaseResource` array. Verified against
//! the OpenAPI the project publishes at
//! `https://raw.githubusercontent.com/Prowlarr/Prowlarr/develop/src/Prowlarr.Api.V1/openapi.json`
//! (path `/api/v1/search`; security scheme `X-Api-Key`, `in: header`) on 2026-10-16. The
//! fields this adapter reads are `title`, `size`, `seeders`, `leechers`, `magnetUrl`,
//! `infoHash`, `indexer`, `publishDate`, `guid`, and `protocol` (`unknown | usenet | torrent`).
//!
//! Adapters ship disabled with no bundled default domain, and nothing here polls on a schedule.

mod error;
mod port;
mod prowlarr;

pub use error::IndexerError;
pub use port::{Indexer, IndexerFuture, IndexerQuery, IndexerRelease, TorrentSource};
pub use prowlarr::Prowlarr;
