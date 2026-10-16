//! The indexers namespace: configure Prowlarr, test it, and search it.
//!
//! Search answers a page of candidates a screen waits on, so it is a command rather than a job.
//! Configure, clear, and test touch the keyring and the network; the blocking keyring read is run in
//! spawn_blocking by the service rather than on the runtime's threads.
//!
//! Ranking is deliberately absent: no quality profile is stored yet and PG-E12-10 owns the release
//! decision, so these results are in the indexer's own order and nothing here pretends otherwise.

use peregrine_app::indexers;
use peregrine_app::{
    IndexerConfigError, IndexerRelease, IndexerStatus, MAX_LIMIT, Secret, TorrentSource,
};
use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;
use tauri::State;

use crate::error::{AppError, ErrorCode};
use crate::guard::guard_response;
use crate::runtime::RuntimeState;

/// What this installation has configured, as a settings screen renders it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct IndexerStatusDto {
    /// Whether both a base URL and a key are stored.
    pub configured: bool,
    /// The configured base URL, which is not a secret.
    pub base_url: Option<String>,
}

impl From<IndexerStatus> for IndexerStatusDto {
    fn from(value: IndexerStatus) -> Self {
        Self {
            configured: value.configured,
            base_url: value.base_url,
        }
    }
}

/// One release an indexer found, as a results list renders it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseCandidateDto {
    /// The release name as the indexer stated it.
    pub title: String,
    /// Size in bytes, when the indexer stated one.
    ///
    /// `Number` opts this `i64` into a TypeScript `number`, which specta otherwise refuses
    /// rather than losing precision; a release size stays well inside the 2^53 a `number` holds.
    #[specta(type = Option<Number>)]
    pub size_bytes: Option<i64>,
    /// Reported seeds.
    pub seeders: Option<u32>,
    /// Reported leeches.
    pub leechers: Option<u32>,
    /// Where the bytes come from: magnet or infoHash. A download path reads this.
    pub source: String,
    /// Which indexer answered.
    pub indexer: Option<String>,
    /// The indexer's publish date, as it stated it.
    pub published: Option<String>,
    /// The indexer's own identifier for the release.
    pub guid: Option<String>,
}

impl From<IndexerRelease> for ReleaseCandidateDto {
    fn from(value: IndexerRelease) -> Self {
        let source = match value.source {
            TorrentSource::Magnet(_) => "magnet",
            TorrentSource::InfoHash(_) => "infoHash",
        };
        Self {
            title: value.raw_title,
            size_bytes: value.size_bytes.and_then(|bytes| i64::try_from(bytes).ok()),
            seeders: value.seeders,
            leechers: value.leechers,
            source: source.to_owned(),
            indexer: value.indexer,
            published: value.published,
            guid: value.guid,
        }
    }
}

fn map_config(error: IndexerConfigError) -> AppError {
    match error {
        IndexerConfigError::Unconfigured => AppError::new(ErrorCode::InvalidInput)
            .with_detail("No indexer is configured yet. Add your Prowlarr URL and API key first."),
        other => AppError::new(ErrorCode::Internal).with_detail(other.to_string()),
    }
}

fn required(value: &str, empty: &str) -> Result<String, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::new(ErrorCode::InvalidInput).with_detail(empty.to_owned()));
    }
    Ok(trimmed.to_owned())
}

/// Reads what indexer is configured.
#[tauri::command]
#[specta::specta]
pub async fn indexer_status(state: State<'_, RuntimeState>) -> Result<IndexerStatusDto, AppError> {
    let store = state.runtime()?.secret_store();
    let status = indexers::status_async(store).await.map_err(map_config)?;
    Ok(guard_response(
        "indexers::status",
        IndexerStatusDto::from(status),
    ))
}

/// Stores a Prowlarr base URL and API key.
#[tauri::command]
#[specta::specta]
pub async fn set_indexer(
    state: State<'_, RuntimeState>,
    base_url: String,
    api_key: String,
) -> Result<IndexerStatusDto, AppError> {
    let base = required(
        &base_url,
        "Enter the Prowlarr URL, for example http://localhost:9696.",
    )?;
    let key = required(&api_key, "Paste your Prowlarr API key before saving.")?;
    let store = state.runtime()?.secret_store();
    let status = indexers::save_async(store, base, Secret::new(key))
        .await
        .map_err(map_config)?;
    Ok(guard_response(
        "indexers::set",
        IndexerStatusDto::from(status),
    ))
}

/// Removes the stored Prowlarr configuration.
#[tauri::command]
#[specta::specta]
pub async fn clear_indexer(state: State<'_, RuntimeState>) -> Result<IndexerStatusDto, AppError> {
    let store = state.runtime()?.secret_store();
    let status = indexers::clear_async(store).await.map_err(map_config)?;
    Ok(guard_response(
        "indexers::clear",
        IndexerStatusDto::from(status),
    ))
}

/// Asks a candidate Prowlarr configuration whether it works.
#[tauri::command]
#[specta::specta]
pub async fn test_indexer(
    state: State<'_, RuntimeState>,
    base_url: String,
    api_key: String,
) -> Result<bool, AppError> {
    let _runtime = state.runtime()?;
    let base = required(
        &base_url,
        "Enter the Prowlarr URL, for example http://localhost:9696.",
    )?;
    let key = required(&api_key, "Paste your Prowlarr API key before testing.")?;
    indexers::test_async(base, Secret::new(key))
        .await
        .map_err(|error| AppError::new(ErrorCode::InvalidInput).with_detail(error.to_string()))?;
    Ok(guard_response("indexers::test", true))
}

/// Searches the configured indexer for releases.
#[tauri::command]
#[specta::specta]
pub async fn search_releases(
    state: State<'_, RuntimeState>,
    query: String,
    limit: Option<u32>,
) -> Result<Vec<ReleaseCandidateDto>, AppError> {
    let text = required(&query, "Type something to search for.")?;
    let limit = limit.unwrap_or(indexers::DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let store = state.runtime()?.secret_store();
    let releases = indexers::search_async(store, text, limit)
        .await
        .map_err(map_config)?;
    Ok(guard_response(
        "indexers::search",
        releases
            .into_iter()
            .map(ReleaseCandidateDto::from)
            .collect(),
    ))
}
