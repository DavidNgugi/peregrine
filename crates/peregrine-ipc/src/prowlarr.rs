//! The Prowlarr namespace: the managed component pack, and its state (docs/adr/0024).
//!
//! Install and start are jobs, because one streams tens of megabytes and the other waits on a
//! health check; this layer names the work and answers the job. The manual base URL and key stay
//! the custom path (a remote or homelab instance), so the two live side by side in Settings.

use peregrine_app::ProwlarrStatus;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;

use crate::error::AppError;
use crate::guard::guard_response;
use crate::jobs::JobStarted;
use crate::runtime::RuntimeState;

/// The managed indexer manager's state, as a settings screen draws it. Never the key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProwlarrStatusDto {
    /// Whether this build has a pinned Prowlarr for the running platform.
    pub supported: bool,
    /// Whether the verified pack is on disk.
    pub installed: bool,
    /// Whether a managed instance is running.
    pub running: bool,
    /// The pinned version this build installs.
    pub version: String,
    /// The loopback endpoint, when one is running.
    pub base_url: Option<String>,
}

impl From<ProwlarrStatus> for ProwlarrStatusDto {
    fn from(value: ProwlarrStatus) -> Self {
        Self {
            supported: value.supported,
            installed: value.installed,
            running: value.running,
            version: value.version.to_owned(),
            base_url: value.base_url,
        }
    }
}

/// Reads the managed indexer manager's state.
#[tauri::command]
#[specta::specta]
pub async fn prowlarr_status(
    state: State<'_, RuntimeState>,
) -> Result<ProwlarrStatusDto, AppError> {
    let status = state.runtime()?.prowlarr_status();
    Ok(guard_response(
        "prowlarr::status",
        ProwlarrStatusDto::from(status),
    ))
}

/// Downloads and verifies the pinned Prowlarr pack, and answers the job doing it.
#[tauri::command]
#[specta::specta]
pub async fn install_prowlarr(state: State<'_, RuntimeState>) -> Result<JobStarted, AppError> {
    let started = state.runtime()?.start_prowlarr_install().await?;
    Ok(guard_response(
        "prowlarr::install",
        JobStarted::from(started),
    ))
}

/// Starts the installed Prowlarr and stores the endpoint and key it generated.
#[tauri::command]
#[specta::specta]
pub async fn start_prowlarr(state: State<'_, RuntimeState>) -> Result<JobStarted, AppError> {
    let started = state.runtime()?.start_prowlarr().await?;
    Ok(guard_response("prowlarr::start", JobStarted::from(started)))
}

/// Stops the managed Prowlarr, and answers the state after the stop.
#[tauri::command]
#[specta::specta]
pub async fn stop_prowlarr(state: State<'_, RuntimeState>) -> Result<ProwlarrStatusDto, AppError> {
    let runtime = state.runtime()?;
    runtime.stop_prowlarr();
    Ok(guard_response(
        "prowlarr::stop",
        ProwlarrStatusDto::from(runtime.prowlarr_status()),
    ))
}
