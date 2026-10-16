//! Prowlarr: what the user configured, and searching it.
//!
//! The configuration is two keyring entries because a base URL is configuration and an API key is a
//! credential; SecretKey::endpoint names the first and SecretKey::provider the second, which is the
//! shape a metadata provider key already uses. Nothing here logs, formats, or returns a key.

use std::sync::Arc;

use peregrine_indexers::{Indexer, IndexerError, IndexerQuery, IndexerRelease, Prowlarr};
use peregrine_secrets::{Secret, SecretKey, SecretStore, SecretStoreError};

/// The name Prowlarr's entries are stored under.
pub const PROWLARR: &str = "prowlarr";

/// How many results a search asks for unless the caller says otherwise.
pub const DEFAULT_LIMIT: u32 = 100;

/// The most results a caller may ask for.
pub const MAX_LIMIT: u32 = 300;

/// Why a configuration could not be read, or a search could not run.
#[derive(Debug, thiserror::Error)]
pub enum IndexerConfigError {
    /// The keyring could not be read or written.
    #[error("the credential store could not be read")]
    Store(#[from] SecretStoreError),
    /// The indexer refused or could not answer.
    #[error(transparent)]
    Indexer(#[from] IndexerError),
    /// Nothing is configured, so no request was made.
    #[error("no indexer is configured")]
    Unconfigured,
}

/// What the settings screen shows about the indexer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexerStatus {
    /// Whether both entries are stored.
    pub configured: bool,
    /// The configured base URL, which is not a secret.
    pub base_url: Option<String>,
}

/// The key the base URL is stored under.
#[must_use]
pub fn endpoint_key() -> SecretKey {
    SecretKey::endpoint(PROWLARR)
}

/// The key the API key is stored under.
#[must_use]
pub fn api_key_key() -> SecretKey {
    SecretKey::provider(PROWLARR)
}

fn load(store: &dyn SecretStore) -> Result<Option<(String, Secret)>, SecretStoreError> {
    let Some(endpoint) = store.get(&endpoint_key())? else {
        return Ok(None);
    };
    let Some(key) = store.get(&api_key_key())? else {
        return Ok(None);
    };
    Ok(Some((endpoint.expose_secret().to_owned(), key)))
}

/// Reads whether an indexer is configured. Blocking; a caller on the async runtime wraps it.
pub fn status(store: &dyn SecretStore) -> Result<IndexerStatus, SecretStoreError> {
    Ok(match load(store)? {
        Some((base_url, _)) => IndexerStatus {
            configured: true,
            base_url: Some(base_url),
        },
        None => IndexerStatus {
            configured: false,
            base_url: None,
        },
    })
}

/// Stores a base URL and an API key. Blocking; a caller on the async runtime wraps it.
pub fn save(
    store: &dyn SecretStore,
    base_url: &str,
    api_key: &Secret,
) -> Result<(), SecretStoreError> {
    store.set(&endpoint_key(), &Secret::new(base_url))?;
    store.set(&api_key_key(), api_key)
}

/// Removes both entries. Blocking; a caller on the async runtime wraps it.
pub fn clear(store: &dyn SecretStore) -> Result<(), SecretStoreError> {
    store.delete(&endpoint_key())?;
    store.delete(&api_key_key())
}

fn build_adapter(base_url: &str, api_key: &Secret) -> Result<Prowlarr, IndexerError> {
    Prowlarr::new(base_url, api_key.expose_secret().to_owned())
}

/// Reads the status, running the blocking keyring read off the runtime's threads.
pub async fn status_async(
    store: Arc<dyn SecretStore>,
) -> Result<IndexerStatus, IndexerConfigError> {
    tokio::task::spawn_blocking(move || status(store.as_ref()))
        .await
        .map_err(|_| IndexerConfigError::Store(SecretStoreError::Failed))?
        .map_err(Into::into)
}

/// Stores the configuration and answers the new status.
pub async fn save_async(
    store: Arc<dyn SecretStore>,
    base_url: String,
    api_key: Secret,
) -> Result<IndexerStatus, IndexerConfigError> {
    let writer = Arc::clone(&store);
    tokio::task::spawn_blocking(move || save(writer.as_ref(), &base_url, &api_key))
        .await
        .map_err(|_| IndexerConfigError::Store(SecretStoreError::Failed))??;
    status_async(store).await
}

/// Removes the configuration and answers the new status.
pub async fn clear_async(store: Arc<dyn SecretStore>) -> Result<IndexerStatus, IndexerConfigError> {
    let writer = Arc::clone(&store);
    tokio::task::spawn_blocking(move || clear(writer.as_ref()))
        .await
        .map_err(|_| IndexerConfigError::Store(SecretStoreError::Failed))??;
    status_async(store).await
}

/// Probes a candidate configuration without storing it.
pub async fn test_async(base_url: String, api_key: Secret) -> Result<(), IndexerError> {
    build_adapter(&base_url, &api_key)?.check().await
}

/// Searches the configured indexer.
pub async fn search_async(
    store: Arc<dyn SecretStore>,
    text: String,
    limit: u32,
) -> Result<Vec<IndexerRelease>, IndexerConfigError> {
    let reader = Arc::clone(&store);
    let loaded = tokio::task::spawn_blocking(move || load(reader.as_ref()))
        .await
        .map_err(|_| IndexerConfigError::Store(SecretStoreError::Failed))??;
    let Some((base_url, api_key)) = loaded else {
        return Err(IndexerConfigError::Unconfigured);
    };
    let indexer = build_adapter(&base_url, &api_key)?;
    Ok(indexer.search(&IndexerQuery { text, limit }).await?)
}
