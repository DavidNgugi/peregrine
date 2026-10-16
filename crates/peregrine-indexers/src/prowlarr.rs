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

/// How much of a non-success answer is read to recover the indexer's own explanation.
const REJECTION_BODY_CAP: usize = 64 * 1024;

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
    async fn read_capped(&self, mut response: reqwest::Response) -> Result<Vec<u8>, IndexerError> {
        if !response.status().is_success() {
            return Err(self.rejection(response).await);
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

    /// Reads the indexer's own explanation out of a non-success answer.
    ///
    /// Prowlarr's validation body is an array of `{ propertyName, errorMessage }`, which tells the
    /// user which field is wrong rather than only the status. Nothing a request carried is echoed:
    /// the key is redacted if it appears, and the base URL is never part of the body.
    async fn rejection(&self, mut response: reqwest::Response) -> IndexerError {
        let status = response.status().as_u16();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.ok().flatten() {
            if body.len() + chunk.len() > REJECTION_BODY_CAP {
                break;
            }
            body.extend_from_slice(&chunk);
        }
        let detail = validation_detail(&body);
        if detail.is_empty() {
            return IndexerError::Status(status);
        }
        IndexerError::Rejected {
            status,
            detail: if self.api_key.is_empty() {
                detail
            } else {
                detail.replace(&self.api_key, "***")
            },
        }
    }
}

/// Recovers the messages from Prowlarr's validation body, or a short text fallback.
fn validation_detail(body: &[u8]) -> String {
    if let Ok(serde_json::Value::Array(items)) = serde_json::from_slice::<serde_json::Value>(body) {
        let mut parts = Vec::new();
        for item in items {
            let name = item.get("propertyName").and_then(serde_json::Value::as_str);
            let message = item.get("errorMessage").and_then(serde_json::Value::as_str);
            match (name, message) {
                (Some(name), Some(message)) => parts.push(format!("{name}: {message}")),
                (None, Some(message)) => parts.push(message.to_owned()),
                (Some(name), None) => parts.push(name.to_owned()),
                (None, None) => {}
            }
        }
        if !parts.is_empty() {
            return parts.join("; ");
        }
    }
    String::from_utf8_lossy(body)
        .trim()
        .chars()
        .take(300)
        .collect()
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
            let _ = self.read_capped(response).await?;
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
            let body = self.read_capped(response).await?;
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

/// The most bytes an admin answer may carry. The indexer schema is every definition Prowlarr knows
/// with every field, so it is far larger than a search answer.
pub const ADMIN_BODY_CAP: usize = 64 * 1024 * 1024;

/// One indexer already configured in Prowlarr, as the settings list renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredIndexer {
    /// Prowlarr's own identity for the row.
    pub id: i64,
    /// The user's name for it.
    pub name: String,
    /// Whether Prowlarr is using it.
    pub enable: bool,
}

/// One selectable option of a `select` field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexerOption {
    /// The stored value.
    pub value: String,
    /// What a person sees.
    pub label: String,
}

/// One field of an indexer definition, as a form renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexerField {
    /// The field's name in Prowlarr's post body.
    pub name: String,
    /// The human label.
    pub label: String,
    /// The control kind Prowlarr states: `textbox`, `password`, `number`, `checkbox`, `select`, ...
    pub kind: String,
    /// The current or default value.
    pub value: Option<String>,
    /// The choices for a `select`.
    pub options: Vec<IndexerOption>,
    /// Whether Prowlarr marks it advanced (the form hides these by default).
    pub advanced: bool,
    /// Whether the value is a credential, so the form renders it as a password.
    pub secret: bool,
}

/// One addable definition from `/api/v1/indexer/schema`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexerDefinition {
    /// The identity an add uses to select this definition again: implementation plus contract.
    pub key: String,
    /// The definition's name.
    pub name: String,
    /// Its one-line description.
    pub description: Option<String>,
    /// Its fields, in the order Prowlarr lists them.
    pub fields: Vec<IndexerField>,
}

/// What an add sends: which definition, what to call it, and the field values the user set.
#[derive(Debug, Clone)]
pub struct NewIndexer {
    /// The definition key the add resolves against a fresh schema read.
    pub key: String,
    /// The name to store it under.
    pub name: String,
    /// Field name and value pairs that override the definition's defaults.
    pub fields: Vec<(String, String)>,
}

/// One entry of Prowlarr's `IndexerResource`, private to the adapter. The same shape is used for a
/// configured row, a schema definition, and a post body.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireIndexer {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    enable: Option<bool>,
    #[serde(default)]
    implementation: Option<String>,
    #[serde(default)]
    config_contract: Option<String>,
    #[serde(default)]
    definition_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    fields: Vec<WireField>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireField {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    value: Option<serde_json::Value>,
    #[serde(default)]
    advanced: Option<bool>,
    #[serde(default)]
    privacy: Option<String>,
    #[serde(default)]
    select_options: Vec<WireSelectOption>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireSelectOption {
    #[serde(default)]
    value: Option<serde_json::Value>,
    #[serde(default)]
    name: Option<String>,
}

fn json_text(value: serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text),
        serde_json::Value::Number(number) => Some(number.to_string()),
        serde_json::Value::Bool(flag) => Some(flag.to_string()),
        serde_json::Value::Null => None,
        other => Some(other.to_string()),
    }
}

fn definition_key(value: &serde_json::Value) -> Option<String> {
    let implementation = value.get("implementation")?.as_str()?;
    let contract = value.get("configContract")?.as_str()?;
    Some(format!("{implementation}::{contract}"))
}

impl WireIndexer {
    fn into_configured(self) -> ConfiguredIndexer {
        ConfiguredIndexer {
            id: self.id.unwrap_or_default(),
            name: stated(self.name).unwrap_or_default(),
            enable: self.enable.unwrap_or(true),
        }
    }

    fn into_definition(self) -> Option<IndexerDefinition> {
        let implementation = stated(self.implementation)?;
        let contract = stated(self.config_contract)?;
        let name = stated(self.definition_name).or_else(|| stated(self.name))?;
        Some(IndexerDefinition {
            key: format!("{implementation}::{contract}"),
            name,
            description: stated(self.description),
            fields: self
                .fields
                .into_iter()
                .filter_map(WireField::into_field)
                .collect(),
        })
    }
}

impl WireField {
    fn into_field(self) -> Option<IndexerField> {
        let name = stated(self.name)?;
        let label = stated(self.label).unwrap_or_else(|| name.clone());
        let kind = stated(self.kind).unwrap_or_else(|| "textbox".to_owned());
        let secret = matches!(self.privacy.as_deref(), Some("apiKey" | "password"));
        let options = self
            .select_options
            .into_iter()
            .filter_map(|option| {
                let value = option.value.and_then(json_text)?;
                let label = stated(option.name).unwrap_or_else(|| value.clone());
                Some(IndexerOption { value, label })
            })
            .collect();
        Some(IndexerField {
            name,
            label,
            kind,
            value: self.value.and_then(json_text),
            options,
            advanced: self.advanced.unwrap_or(false),
            secret,
        })
    }
}

impl Prowlarr {
    /// Reads an admin answer under a cap large enough for the whole indexer schema.
    async fn read_admin(&self, mut response: reqwest::Response) -> Result<Vec<u8>, IndexerError> {
        if !response.status().is_success() {
            return Err(self.rejection(response).await);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| IndexerError::Transport)?
        {
            if body.len() + chunk.len() > ADMIN_BODY_CAP {
                return Err(IndexerError::BodyTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    async fn schema(&self) -> Result<Vec<serde_json::Value>, IndexerError> {
        let response = self
            .client
            .get(self.endpoint("/api/v1/indexer/schema"))
            .header("X-Api-Key", &self.api_key)
            .send()
            .await
            .map_err(|_| IndexerError::Transport)?;
        let body = self.read_admin(response).await?;
        serde_json::from_slice(&body).map_err(|_| IndexerError::SchemaDrift)
    }

    /// The indexers already configured in Prowlarr.
    pub async fn list_indexers(&self) -> Result<Vec<ConfiguredIndexer>, IndexerError> {
        let response = self
            .client
            .get(self.endpoint("/api/v1/indexer"))
            .header("X-Api-Key", &self.api_key)
            .send()
            .await
            .map_err(|_| IndexerError::Transport)?;
        let body = self.read_admin(response).await?;
        let wire: Vec<WireIndexer> =
            serde_json::from_slice(&body).map_err(|_| IndexerError::SchemaDrift)?;
        Ok(wire.into_iter().map(WireIndexer::into_configured).collect())
    }

    /// The definitions this Prowlarr can add.
    pub async fn indexer_catalog(&self) -> Result<Vec<IndexerDefinition>, IndexerError> {
        let wire: Vec<WireIndexer> =
            serde_json::from_value(serde_json::Value::Array(self.schema().await?))
                .map_err(|_| IndexerError::SchemaDrift)?;
        Ok(wire
            .into_iter()
            .filter_map(WireIndexer::into_definition)
            .collect())
    }

    /// Adds one indexer, resolving the definition against a fresh schema read and applying the
    /// caller's name and field overrides.
    pub async fn add_indexer(
        &self,
        request: &NewIndexer,
    ) -> Result<ConfiguredIndexer, IndexerError> {
        let mut entries = self.schema().await?;
        let entry = entries
            .iter_mut()
            .find(|value| definition_key(value).as_deref() == Some(request.key.as_str()))
            .ok_or(IndexerError::SchemaDrift)?;
        if let Some(object) = entry.as_object_mut() {
            object.insert(
                "name".to_owned(),
                serde_json::Value::String(request.name.clone()),
            );
            if let Some(fields) = object
                .get_mut("fields")
                .and_then(serde_json::Value::as_array_mut)
            {
                for field in fields.iter_mut() {
                    let Some(field_name) = field
                        .get("name")
                        .and_then(|n| n.as_str())
                        .map(str::to_owned)
                    else {
                        continue;
                    };
                    if let Some((_, value)) =
                        request.fields.iter().find(|(name, _)| *name == field_name)
                        && let Some(field_object) = field.as_object_mut()
                    {
                        field_object
                            .insert("value".to_owned(), serde_json::Value::String(value.clone()));
                    }
                }
            }
        }
        let posted = self
            .client
            .post(self.endpoint("/api/v1/indexer"))
            .header("X-Api-Key", &self.api_key)
            .json(&*entry)
            .send()
            .await
            .map_err(|_| IndexerError::Transport)?;
        let body = self.read_admin(posted).await?;
        let wire: WireIndexer =
            serde_json::from_slice(&body).map_err(|_| IndexerError::SchemaDrift)?;
        Ok(wire.into_configured())
    }

    /// Removes one configured indexer by Prowlarr's own id.
    pub async fn remove_indexer(&self, id: i64) -> Result<(), IndexerError> {
        let response = self
            .client
            .delete(self.endpoint(&format!("/api/v1/indexer/{id}")))
            .header("X-Api-Key", &self.api_key)
            .send()
            .await
            .map_err(|_| IndexerError::Transport)?;
        if !response.status().is_success() {
            return Err(IndexerError::Status(response.status().as_u16()));
        }
        Ok(())
    }
}
