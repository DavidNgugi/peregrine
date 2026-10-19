//! The Usenet settings namespace: providers, their test, and the archive policy.
//!
//! No command accepts or returns a password after it is stored. A save may carry a new password
//! once; a read carries only whether the provider is configured and its last health.

use peregrine_app::usenet::{UsenetConfigError, UsenetServerInput, UsenetStatus};
use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;
use tauri::State;

use crate::error::{AppError, ErrorCode};
use crate::guard::guard_response;
use crate::runtime::RuntimeState;

/// One configured provider, as the settings list renders it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UsenetServerDto {
    /// The row id.
    #[specta(type = Number)]
    pub id: i64,
    /// The display name.
    pub name: String,
    /// The host.
    pub host: String,
    /// The port.
    pub port: u16,
    /// Whether the connection uses implicit TLS.
    pub tls: bool,
    /// The account name.
    pub username: String,
    /// How many connections the engine may open.
    pub connections: u16,
    /// Lower values are tried first.
    #[specta(type = Number)]
    pub priority: i64,
    /// Whether the engine may contact it.
    pub enabled: bool,
    /// The last known health, as its stored spelling.
    pub health: String,
}

impl From<peregrine_app::usenet::ConfiguredUsenetServer> for UsenetServerDto {
    fn from(value: peregrine_app::usenet::ConfiguredUsenetServer) -> Self {
        Self {
            id: value.id,
            name: value.name.as_str().to_owned(),
            host: value.host.as_str().to_owned(),
            port: value.port,
            tls: value.tls,
            username: value.username,
            connections: value.connections,
            priority: value.priority,
            enabled: value.enabled,
            health: value.health.as_str().to_owned(),
        }
    }
}

/// What the settings screen reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)] // Four independent yes/no facts the settings screen shows.
pub struct UsenetStatusDto {
    /// Whether at least one enabled provider exists.
    pub configured: bool,
    /// The providers, primary first.
    pub servers: Vec<UsenetServerDto>,
    /// Whether Usenet archives may be extracted.
    pub allow_archives: bool,
    /// Whether the archives are removed after a successful import.
    pub delete_archives_after_import: bool,
    /// Whether the archive extractor was found.
    pub extract_available: bool,
    /// Whether the repair tool was found.
    pub repair_available: bool,
}

impl From<UsenetStatus> for UsenetStatusDto {
    fn from(value: UsenetStatus) -> Self {
        Self {
            configured: value.configured,
            servers: value.servers.into_iter().map(Into::into).collect(),
            allow_archives: value.allow_archives,
            delete_archives_after_import: value.delete_archives_after_import,
            extract_available: value.extract_available,
            repair_available: value.repair_available,
        }
    }
}

/// The fields a save carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UsenetServerInputDto {
    /// The row being edited, or null for a new provider.
    #[specta(type = Option<Number>)]
    pub id: Option<i64>,
    /// The display name.
    pub name: String,
    /// The host.
    pub host: String,
    /// The port.
    pub port: u16,
    /// Whether to use implicit TLS.
    pub tls: bool,
    /// The account name.
    pub username: String,
    /// A new password, when the user typed one. Never read back.
    pub password: Option<String>,
    /// The connection count.
    pub connections: u16,
    /// Lower is tried first.
    #[specta(type = Number)]
    pub priority: i64,
    /// Whether the engine may contact it.
    pub enabled: bool,
}

impl From<UsenetServerInputDto> for UsenetServerInput {
    fn from(value: UsenetServerInputDto) -> Self {
        Self {
            id: value.id,
            name: value.name,
            host: value.host,
            port: value.port,
            tls: value.tls,
            username: value.username,
            password: value.password,
            connections: value.connections,
            priority: value.priority,
            enabled: value.enabled,
        }
    }
}

fn map_config(error: &UsenetConfigError) -> AppError {
    let code = match error {
        UsenetConfigError::Invalid(_) | UsenetConfigError::Unconfigured => ErrorCode::InvalidInput,
        UsenetConfigError::Transport(_) => ErrorCode::Network,
        UsenetConfigError::Secret(_) | UsenetConfigError::Storage(_) => ErrorCode::Internal,
    };
    AppError::new(code).with_detail(error.to_string())
}

/// Reads the configured providers and the archive policy.
#[tauri::command]
#[specta::specta]
pub async fn usenet_status(state: State<'_, RuntimeState>) -> Result<UsenetStatusDto, AppError> {
    let status = state
        .runtime()?
        .usenet_status()
        .await
        .map_err(|error| map_config(&error))?;
    Ok(guard_response("usenet::status", status.into()))
}

/// Inserts or updates one provider.
#[tauri::command]
#[specta::specta]
pub async fn save_usenet_server(
    state: State<'_, RuntimeState>,
    server: UsenetServerInputDto,
) -> Result<UsenetStatusDto, AppError> {
    let status = state
        .runtime()?
        .save_usenet_server(server.into())
        .await
        .map_err(|error| map_config(&error))?;
    Ok(guard_response("usenet::save", status.into()))
}

/// Removes one provider and its keyring entry.
#[tauri::command]
#[specta::specta]
pub async fn remove_usenet_server(
    state: State<'_, RuntimeState>,
    server: UsenetServerInputDto,
) -> Result<UsenetStatusDto, AppError> {
    let id = server.id.ok_or_else(|| {
        AppError::new(ErrorCode::InvalidInput).with_detail("a provider id is required")
    })?;
    let status = state
        .runtime()?
        .remove_usenet_server(id)
        .await
        .map_err(|error| map_config(&error))?;
    Ok(guard_response("usenet::remove", status.into()))
}

/// Tests one provider without storing it.
#[tauri::command]
#[specta::specta]
pub async fn test_usenet_server(
    state: State<'_, RuntimeState>,
    server: UsenetServerInputDto,
) -> Result<(), AppError> {
    state
        .runtime()?
        .test_usenet_server(server.into())
        .await
        .map_err(|error| map_config(&error))?;
    Ok(())
}

/// Writes the archive policy and answers the fresh status.
#[tauri::command]
#[specta::specta]
pub async fn set_usenet_archives(
    state: State<'_, RuntimeState>,
    allow_archives: bool,
    delete_archives_after_import: bool,
) -> Result<UsenetStatusDto, AppError> {
    let runtime = state.runtime()?;
    runtime
        .set_usenet_archives(allow_archives, delete_archives_after_import)
        .await
        .map_err(|error| map_config(&error))?;
    let status = runtime
        .usenet_status()
        .await
        .map_err(|error| map_config(&error))?;
    Ok(guard_response("usenet::archives", status.into()))
}
/// Re-inspects a blocked Usenet job with the archive opt-in.
#[tauri::command]
#[specta::specta]
pub async fn consent_usenet_download(
    state: State<'_, RuntimeState>,
    download: crate::downloads::DownloadRef,
) -> Result<String, AppError> {
    let state = state
        .runtime()?
        .consent_usenet(download.id)
        .await
        .map_err(|error| AppError::new(ErrorCode::InvalidInput).with_detail(error.to_string()))?;
    Ok(guard_response("usenet::consent", state.as_str().to_owned()))
}
