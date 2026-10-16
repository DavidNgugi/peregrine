//! Prowlarr: what the user configured, and searching it.
//!
//! The configuration is two keyring entries because a base URL is configuration and an API key is a
//! credential; SecretKey::endpoint names the first and SecretKey::provider the second, which is the
//! shape a metadata provider key already uses. Nothing here logs, formats, or returns a key.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use peregrine_indexers::{
    ConfiguredIndexer, Indexer, IndexerDefinition, IndexerError, IndexerQuery, IndexerRelease,
    NewIndexer, Prowlarr,
};
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

/// The app's data directory, so the managed instance's own `config.xml` can be read as a
/// keychain fallback. Set once by the composition root.
static MANAGED_DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Records where the managed instance keeps its data. Called once, by `Runtime::start`.
pub fn set_managed_data_dir(data_dir: PathBuf) {
    let _ = MANAGED_DATA_DIR.set(data_dir);
}

/// The managed instance's key from its own `config.xml`, or None when this build has none.
fn managed_credentials() -> Option<(String, String)> {
    let data_dir = MANAGED_DATA_DIR.get()?;
    crate::prowlarr::managed_credentials(data_dir)
}

fn load(store: &dyn SecretStore) -> Result<Option<(String, Secret)>, SecretStoreError> {
    let stored = store.get(&endpoint_key()).and_then(|endpoint| {
        endpoint.map_or(Ok(None), |endpoint| {
            store
                .get(&api_key_key())
                .map(|key| key.map(|key| (endpoint, key)))
        })
    });
    // The managed instance's key can be refused by the keyring — a locked keychain, or an update
    // prompt a background process cannot answer — and the app owns that config.xml, so search reads
    // it here rather than depending on the keychain. A custom Prowlarr's key is in the keyring and
    // nowhere else, so a refusal with no managed file still surfaces as a store error.
    match stored {
        Ok(Some((endpoint, key))) => Ok(Some((endpoint.expose_secret().to_owned(), key))),
        Ok(None) => Ok(managed_credentials().map(|(url, key)| (url, Secret::new(key)))),
        Err(error) => match managed_credentials() {
            Some((url, key)) => Ok(Some((url, Secret::new(key)))),
            None => Err(error),
        },
    }
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

/// The adapter over what is stored, or a typed refusal when nothing is.
async fn configured_adapter(store: Arc<dyn SecretStore>) -> Result<Prowlarr, IndexerConfigError> {
    let loaded = tokio::task::spawn_blocking(move || load(store.as_ref()))
        .await
        .map_err(|_| IndexerConfigError::Store(SecretStoreError::Failed))??;
    let Some((base_url, api_key)) = loaded else {
        return Err(IndexerConfigError::Unconfigured);
    };
    Ok(build_adapter(&base_url, &api_key)?)
}

/// The indexers already configured in Prowlarr.
pub async fn list_indexers_async(
    store: Arc<dyn SecretStore>,
) -> Result<Vec<ConfiguredIndexer>, IndexerConfigError> {
    Ok(configured_adapter(store).await?.list_indexers().await?)
}

/// The definitions the configured Prowlarr can add.
pub async fn catalog_async(
    store: Arc<dyn SecretStore>,
) -> Result<Vec<IndexerDefinition>, IndexerConfigError> {
    Ok(configured_adapter(store).await?.indexer_catalog().await?)
}

/// One definition by key, or `None` when the catalog no longer offers it.
pub async fn definition_async(
    store: Arc<dyn SecretStore>,
    key: String,
) -> Result<Option<IndexerDefinition>, IndexerConfigError> {
    Ok(catalog_async(store)
        .await?
        .into_iter()
        .find(|definition| definition.key == key))
}

/// Adds one indexer to the configured Prowlarr.
pub async fn add_indexer_async(
    store: Arc<dyn SecretStore>,
    key: String,
    name: String,
    fields: Vec<(String, String)>,
) -> Result<ConfiguredIndexer, IndexerConfigError> {
    let request = NewIndexer { key, name, fields };
    Ok(configured_adapter(store)
        .await?
        .add_indexer(&request)
        .await?)
}

/// Removes one indexer from the configured Prowlarr by its own id.
pub async fn remove_indexer_async(
    store: Arc<dyn SecretStore>,
    id: i64,
) -> Result<(), IndexerConfigError> {
    Ok(configured_adapter(store).await?.remove_indexer(id).await?)
}

/// Fetches the .torrent bytes behind a release's HTTP source, for the releases whose Prowlarr
/// magnetUrl is a proxy download URL rather than a magnet.
pub async fn fetch_torrent_async(
    store: Arc<dyn SecretStore>,
    url: String,
) -> Result<peregrine_indexers::FetchedTorrent, IndexerConfigError> {
    Ok(configured_adapter(store).await?.fetch_torrent(&url).await?)
}
