//! The Prowlarr adapter (see the crate documentation for the verified API shape).

use std::time::Duration;

use peregrine_domain::ReleaseName;
use serde::Deserialize;

use crate::error::IndexerError;
use crate::port::{Indexer, IndexerFuture, IndexerQuery, IndexerRelease, TorrentSource};

/// The most bytes this adapter will read from a search answer.
///
/// A hundred results carrying titles, sizes, and hashes is comfortably inside this; the cap is what
/// keeps a wrong or hostile answer from being read into memory before it is parsed.
pub const SEARCH_BODY_CAP: usize = 8 * 1024 * 1024;

/// How long a request may take before it is abandoned.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// One configured Prowlarr instance.
///
/// The credential is held as a string because this crate may not depend on the secrets crate; the
/// type deliberately does not implement Debug, so a key cannot reach a log through it.
pub struct Prowlarr {
    client: reqwest::Client,
    base: String,
    api_key: String,
}

impl Prowlarr {
    /// Builds an adapter against a base URL and an API key.
    ///
    /// The base URL is a parameter rather than a constant because Prowlarr is self-hosted: there is
    /// no default endpoint to point at, and storing one would be inventing a domain this build must
    /// not bundle (the crate documentation says why).
    ///
    /// # Errors
    ///
    /// InvalidBaseUrl when the URL is not http(s), Unconfigured when the key is blank, and
    /// Transport when no client could be built.
    pub fn new(base_url: &str, api_key: String) -> Result<Self, IndexerError> {
        let base = base_url.trim().trim_end_matches('/').to_owned();
        let url = reqwest::Url::parse(&base).map_err(|_| IndexerError::InvalidBaseUrl)?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(IndexerError::InvalidBaseUrl);
        }
        if api_key.trim().is_empty() {
            return Err(IndexerError::Unconfigured);
        }
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| IndexerError::Transport)?;
        Ok(Self {
            client,
            base,
            api_key,
        })
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// Reads a response under SEARCH_BODY_CAP, refusing a non-success status first.
    async fn read_capped(mut response: reqwest::Response) -> Result<Vec<u8>, IndexerError> {
        if !response.status().is_success() {
            return Err(IndexerError::Status(response.status().as_u16()));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| IndexerError::Transport)?
        {
            if body.len() + chunk.len() > SEARCH_BODY_CAP {
                return Err(IndexerError::BodyTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

impl Indexer for Prowlarr {
    fn check<'a>(&'a self) -> IndexerFuture<'a, ()> {
        Box::pin(async move {
            let response = self
                .client
                .get(self.endpoint("/api/v1/health"))
                .header("X-Api-Key", &self.api_key)
                .send()
                .await
                .map_err(|_| IndexerError::Transport)?;
            let _ = Self::read_capped(response).await?;
            Ok(())
        })
    }

    fn search<'a>(&'a self, query: &'a IndexerQuery) -> IndexerFuture<'a, Vec<IndexerRelease>> {
        Box::pin(async move {
            let mut url = reqwest::Url::parse(&self.endpoint("/api/v1/search"))
                .map_err(|_| IndexerError::InvalidBaseUrl)?;
            {
                let mut pairs = url.query_pairs_mut();
                pairs.append_pair("query", &query.text);
                pairs.append_pair("type", "search");
                pairs.append_pair("limit", &query.limit.to_string());
            }
            let response = self
                .client
                .get(url)
                .header("X-Api-Key", &self.api_key)
                .send()
                .await
                .map_err(|_| IndexerError::Transport)?;
            let body = Self::read_capped(response).await?;
            let wire: Vec<WireRelease> =
                serde_json::from_slice(&body).map_err(|_| IndexerError::SchemaDrift)?;
            Ok(wire
                .into_iter()
                .filter_map(WireRelease::into_release)
                .collect())
        })
    }
}

/// One entry of Prowlarr's ReleaseResource array. Private: no third-party shape escapes the adapter.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireRelease {
    title: Option<String>,
    size: Option<i64>,
    seeders: Option<i64>,
    leechers: Option<i64>,
    magnet_url: Option<String>,
    info_hash: Option<String>,
    indexer: Option<String>,
    publish_date: Option<String>,
    guid: Option<String>,
    protocol: Option<String>,
}

fn stated(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

impl WireRelease {
    /// The release this entry describes, or None when it is not a torrent with a usable source.
    fn into_release(self) -> Option<IndexerRelease> {
        // A usenet result has no info hash and no magnet, so a torrent engine cannot use it; dropping
        // it here keeps the port's vocabulary torrent-only rather than a flag every caller checks.
        if let Some(protocol) = self.protocol.as_deref() {
            if !protocol.eq_ignore_ascii_case("torrent") {
                return None;
            }
        }
        let raw_title = stated(self.title)?;
        let source = match (stated(self.magnet_url), stated(self.info_hash)) {
            (Some(magnet), _) => TorrentSource::Magnet(magnet),
            (None, Some(hash)) => TorrentSource::InfoHash(hash),
            (None, None) => return None,
        };
        Some(IndexerRelease {
            title: ReleaseName::parse(&raw_title),
            raw_title,
            size_bytes: self.size.and_then(|value| u64::try_from(value).ok()),
            seeders: self.seeders.and_then(|value| u32::try_from(value).ok()),
            leechers: self.leechers.and_then(|value| u32::try_from(value).ok()),
            source,
            indexer: stated(self.indexer),
            published: stated(self.publish_date),
            guid: stated(self.guid),
        })
    }
}
