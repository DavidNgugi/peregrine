//! The indexers namespace: configure Prowlarr, test it, and search it.
//!
//! Search answers a page of candidates a screen waits on, so it is a command rather than a job.
//! Configure, clear, and test touch the keyring and the network; the blocking keyring read is run in
//! `spawn_blocking` by the service rather than on the runtime's threads.
//!
//! Ranking is deliberately absent: no quality profile is stored yet and PG-E12-10 owns the release
//! decision, so these results are in the indexer's own order and nothing here pretends otherwise.

use std::sync::Arc;

use peregrine_app::indexers;
use peregrine_app::{
    ConfiguredIndexer, IndexerConfigError, IndexerDefinition, IndexerField, IndexerOption,
    IndexerRelease, IndexerStatus, MAX_LIMIT, Secret, TorrentSource,
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
            TorrentSource::TorrentUrl(_) => "torrentUrl",
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

/// One indexer already configured in Prowlarr.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ConfiguredIndexerDto {
    /// Prowlarr's own id, which removal names it by.
    ///
    /// `Number` opts this `i64` into a TypeScript `number`; a row id stays inside 2^53.
    #[specta(type = Number)]
    pub id: i64,
    /// The name it is stored under.
    pub name: String,
    /// Whether Prowlarr is using it.
    pub enable: bool,
}

impl From<ConfiguredIndexer> for ConfiguredIndexerDto {
    fn from(value: ConfiguredIndexer) -> Self {
        Self {
            id: value.id,
            name: value.name,
            enable: value.enable,
        }
    }
}

/// One selectable option of a `select` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct IndexerOptionDto {
    /// The stored value.
    pub value: String,
    /// What a person sees.
    pub label: String,
}

impl From<IndexerOption> for IndexerOptionDto {
    fn from(value: IndexerOption) -> Self {
        Self {
            value: value.value,
            label: value.label,
        }
    }
}

/// One field of an addable definition, as a form renders it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct IndexerFieldDto {
    /// The field's name in Prowlarr's post body.
    pub name: String,
    /// The human label.
    pub label: String,
    /// The control kind Prowlarr states.
    pub kind: String,
    /// The current or default value.
    pub value: Option<String>,
    /// The choices for a `select`.
    pub options: Vec<IndexerOptionDto>,
    /// Whether Prowlarr marks it advanced, which the form hides.
    pub advanced: bool,
    /// Whether the value is a credential, which the form masks.
    pub secret: bool,
}

impl From<IndexerField> for IndexerFieldDto {
    fn from(value: IndexerField) -> Self {
        Self {
            name: value.name,
            label: value.label,
            kind: value.kind,
            value: value.value,
            options: value
                .options
                .into_iter()
                .map(IndexerOptionDto::from)
                .collect(),
            advanced: value.advanced,
            secret: value.secret,
        }
    }
}

/// One addable definition from Prowlarr's schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct IndexerDefinitionDto {
    /// The identity an add selects it by.
    pub key: String,
    /// The definition's name.
    pub name: String,
    /// Its one-line description.
    pub description: Option<String>,
    /// Its fields, in Prowlarr's order.
    pub fields: Vec<IndexerFieldDto>,
}

impl From<IndexerDefinition> for IndexerDefinitionDto {
    fn from(value: IndexerDefinition) -> Self {
        Self {
            key: value.key,
            name: value.name,
            description: value.description,
            fields: value
                .fields
                .into_iter()
                .map(IndexerFieldDto::from)
                .collect(),
        }
    }
}

/// One addable definition as the picker lists it, with its fields left out.
///
/// The fields are large — every definition carries dozens — and are read only for the one the user
/// chooses, so a catalog of several hundred stays inside the IPC payload budget (AGENTS R11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct IndexerSummaryDto {
    /// The definition key an add or a detail read names.
    pub key: String,
    /// The definition's name.
    pub name: String,
}

impl From<IndexerDefinition> for IndexerSummaryDto {
    fn from(value: IndexerDefinition) -> Self {
        // The description is deliberately not carried: 657 of them pushed the catalog over the
        // 64 KB IPC payload budget (AGENTS R11), and the picker shows only the name. The one
        // chosen definition's detail, description included, is read on demand.
        Self {
            key: value.key,
            name: value.name,
        }
    }
}

/// One field value an add sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct IndexerFieldValueDto {
    /// The field's name.
    pub name: String,
    /// The value the user set.
    pub value: String,
}

/// What an add sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AddIndexerRequest {
    /// The definition key.
    pub key: String,
    /// The name to store it under.
    pub name: String,
    /// The field values the user set.
    pub fields: Vec<IndexerFieldValueDto>,
}

/// Identifies one configured indexer for removal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct IndexerRef {
    /// Prowlarr's own id.
    #[specta(type = Number)]
    pub id: i64,
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

/// Lists the indexers already configured in Prowlarr.
#[tauri::command]
#[specta::specta]
pub async fn list_indexers(
    state: State<'_, RuntimeState>,
) -> Result<Vec<ConfiguredIndexerDto>, AppError> {
    let store = state.runtime()?.secret_store();
    let rows = indexers::list_indexers_async(store)
        .await
        .map_err(map_config)?;
    Ok(guard_response(
        "indexers::list",
        rows.into_iter().map(ConfiguredIndexerDto::from).collect(),
    ))
}

/// Lists the definitions the configured Prowlarr can add, without their fields.
#[tauri::command]
#[specta::specta]
pub async fn indexer_catalog(
    state: State<'_, RuntimeState>,
) -> Result<Vec<IndexerSummaryDto>, AppError> {
    let store = state.runtime()?.secret_store();
    let rows = indexers::catalog_async(store).await.map_err(map_config)?;
    Ok(guard_response(
        "indexers::catalog",
        rows.into_iter().map(IndexerSummaryDto::from).collect(),
    ))
}

/// One definition's fields, read only when the user picks it.
#[tauri::command]
#[specta::specta]
pub async fn indexer_definition(
    state: State<'_, RuntimeState>,
    key: String,
) -> Result<Option<IndexerDefinitionDto>, AppError> {
    let key = required(&key, "Choose an indexer to add.")?;
    let store = state.runtime()?.secret_store();
    let found = indexers::definition_async(store, key)
        .await
        .map_err(map_config)?;
    Ok(guard_response(
        "indexers::definition",
        found.map(IndexerDefinitionDto::from),
    ))
}

/// Adds one indexer and answers the refreshed list.
#[tauri::command]
#[specta::specta]
pub async fn add_indexer(
    state: State<'_, RuntimeState>,
    request: AddIndexerRequest,
) -> Result<Vec<ConfiguredIndexerDto>, AppError> {
    let name = required(&request.name, "Name the indexer before adding it.")?;
    let fields = request
        .fields
        .into_iter()
        .map(|field| (field.name, field.value))
        .collect();
    let store = state.runtime()?.secret_store();
    indexers::add_indexer_async(store.clone(), request.key, name, fields)
        .await
        .map_err(map_config)?;
    let rows = indexers::list_indexers_async(store)
        .await
        .map_err(map_config)?;
    Ok(guard_response(
        "indexers::add",
        rows.into_iter().map(ConfiguredIndexerDto::from).collect(),
    ))
}

/// Removes one indexer and answers the refreshed list.
#[tauri::command]
#[specta::specta]
pub async fn remove_indexer(
    state: State<'_, RuntimeState>,
    indexer: IndexerRef,
) -> Result<Vec<ConfiguredIndexerDto>, AppError> {
    let store = state.runtime()?.secret_store();
    indexers::remove_indexer_async(Arc::clone(&store), indexer.id)
        .await
        .map_err(map_config)?;
    let rows = indexers::list_indexers_async(store)
        .await
        .map_err(map_config)?;
    Ok(guard_response(
        "indexers::remove",
        rows.into_iter().map(ConfiguredIndexerDto::from).collect(),
    ))
}
