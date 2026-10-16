//! The port a release indexer answers through, and the vocabulary it answers in.

use std::future::Future;
use std::pin::Pin;

use peregrine_domain::ReleaseName;

use crate::error::IndexerError;

/// One indexer call's future. Boxed for the reason `ProviderFuture` is: the trait is used as a
/// trait object, which a bare `async fn` is not.
pub type IndexerFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, IndexerError>> + Send + 'a>>;

/// What to ask an indexer for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexerQuery {
    /// The text to search for.
    pub text: String,
    /// The most results to ask for.
    pub limit: u32,
}

/// Where a release's bytes can be fetched from.
///
/// `Magnet` is preferred when an indexer gives both because it carries the trackers; `InfoHash`
/// is what a caller has to work with when it does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TorrentSource {
    /// A magnet URI.
    Magnet(String),
    /// A bare info hash.
    InfoHash(String),
    /// An HTTP(S) URL for the .torrent file itself. Prowlarr's magnetUrl is frequently its own
    /// proxy download URL rather than a magnet; a caller fetches the bytes before handing them to
    /// the engine, because treating the URL as a magnet asks the engine to GET it and fail.
    TorrentUrl(String),
}

/// What fetching a release's HTTP URL produced.
///
/// Prowlarr's proxy URL answers 301 with a `magnet:` Location for a torrent indexer -- a scheme an
/// HTTP client cannot follow -- and the .torrent body for one that serves a file directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchedTorrent {
    /// The .torrent file's own bytes.
    Bytes(Vec<u8>),
    /// The magnet the proxy redirected to.
    Magnet(String),
}

/// One release an indexer found, in this application's vocabulary.
///
/// The title is carried twice on purpose: `raw_title` is what the indexer said, which is what a
/// person compares, and `title` is the parsed form (`PG-E4-04`). The raw string is kept because
/// the parsed form is lossy and a screen still has to show what was offered.
#[derive(Debug, Clone)]
pub struct IndexerRelease {
    /// The release name as the indexer stated it.
    pub raw_title: String,
    /// The same name through this application's release parser.
    pub title: ReleaseName,
    /// The release's size in bytes, when the indexer stated one.
    pub size_bytes: Option<u64>,
    /// How many seeds the indexer reported.
    pub seeders: Option<u32>,
    /// How many leeches the indexer reported.
    pub leechers: Option<u32>,
    /// Where the bytes can be fetched from.
    pub source: TorrentSource,
    /// Which indexer this result came from.
    pub indexer: Option<String>,
    /// When the indexer says the release was published, as it stated it.
    pub published: Option<String>,
    /// The indexer's own identifier for the release.
    pub guid: Option<String>,
}

/// A release indexer: search only. Adding a torrent is the acquisition service's, not an indexer's.
pub trait Indexer: Send + Sync {
    /// Searches for releases matching `query`.
    fn search<'a>(&'a self, query: &'a IndexerQuery) -> IndexerFuture<'a, Vec<IndexerRelease>>;

    /// Asks whether the configured endpoint and credential work.
    fn check<'a>(&'a self) -> IndexerFuture<'a, ()>;

    /// Fetches the .torrent bytes behind a TorrentSource::TorrentUrl.
    fn fetch_torrent<'a>(&'a self, url: &'a str) -> IndexerFuture<'a, FetchedTorrent>;
}
