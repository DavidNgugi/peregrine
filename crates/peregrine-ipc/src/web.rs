//! The LAN surface namespace (ADR 0028): start it, pair a device, and revoke one.
//!
//! Every command here is a thin call into `peregrine_app::WebService`; the pairing ceremony, the
//! token model, and the read-only API live in the core. The one decision this layer makes is which
//! of the core's refusals a screen can act on, and that mapping is `web_error`.

use std::net::IpAddr;

use peregrine_app::{PairedDevice, WebConfig, WebError, WebStatus};
use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;
use tauri::State;

use crate::error::{AppError, ErrorCode};
use crate::runtime::RuntimeState;

/// What the listener is doing, as Settings draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct WebStatusDto {
    /// Whether a socket is bound.
    pub running: bool,
    /// The bound address, when it is running.
    pub address: Option<String>,
    /// The bound port, when it is running.
    pub port: Option<u16>,
    /// How many pairing requests are waiting for a person.
    #[specta(type = Number)]
    pub pending: u32,
}

/// One pairing request the desktop prompt shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PendingPairDto {
    /// The secret the TV polls with.
    pub pair_id: String,
    /// The eight digits the person compares with the television.
    pub code: String,
    /// The name the client offered.
    pub name: String,
    /// Its platform, when it named one.
    pub platform: Option<String>,
    /// The address it came from.
    pub address: Option<String>,
    /// When it was created, in Unix milliseconds.
    #[specta(type = Number)]
    pub created_at_ms: i64,
    /// When it expires, in Unix milliseconds.
    #[specta(type = Number)]
    pub expires_at_ms: i64,
}

/// A paired device's id, as the list returned it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRef {
    /// The row id.
    #[specta(type = Number)]
    pub id: i64,
}

/// One device that may present a token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PairedDeviceDto {
    /// The row id a revoke takes.
    #[specta(type = Number)]
    pub id: i64,
    /// The name the person approved.
    pub name: String,
    /// Its platform, when it named one.
    pub platform: Option<String>,
    /// The scopes it was issued, as the row stores them.
    pub scopes: String,
    /// When it was paired, in Unix milliseconds.
    #[specta(type = Number)]
    pub created_at: i64,
    /// The last authenticated request, in Unix milliseconds.
    #[specta(type = Option<Number>)]
    pub last_seen_at: Option<i64>,
    /// The address it last presented.
    pub last_address: Option<String>,
    /// Whether the person revoked it.
    pub revoked: bool,
}

/// Starts the listener on one address and port.
///
/// # Errors
///
/// [`ErrorCode::InvalidInput`] for an address that is not one, or the unspecified address;
/// [`ErrorCode::Conflict`] when it is already listening or the port is taken.
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub async fn web_start(
    state: State<'_, RuntimeState>,
    address: String,
    port: u16,
) -> Result<WebStatusDto, AppError> {
    let bind: IpAddr = address.parse().map_err(|_| {
        AppError::new(ErrorCode::InvalidInput).with_detail("That is not an address to bind.")
    })?;
    let web = state.web()?;
    let mut config = WebConfig::new(bind, port);
    if let Some(directory) = state.web_app_dir() {
        config = config.with_app_dir(directory.clone());
    }
    let status = web.start(config).await.map_err(web_error)?;
    Ok(status_dto(&status))
}

/// Stops the listener, if it is running.
///
/// # Errors
///
/// [`ErrorCode::Internal`] when the runtime never started.
#[tauri::command]
#[specta::specta]
pub async fn web_stop(state: State<'_, RuntimeState>) -> Result<(), AppError> {
    state.web()?.stop().await;
    Ok(())
}

/// What the listener is doing.
///
/// # Errors
///
/// [`ErrorCode::Internal`] when the runtime never started.
#[tauri::command]
#[specta::specta]
pub async fn web_status(state: State<'_, RuntimeState>) -> Result<WebStatusDto, AppError> {
    let web = state.web()?;
    Ok(status_dto(&web.status().await))
}

/// The address this machine would be reached at on the local network, when it has one.
#[tauri::command]
#[specta::specta]
pub fn web_lan_address() -> Result<Option<String>, AppError> {
    Ok(detected_lan_address().map(|address| address.to_string()))
}

/// The pairing requests waiting for a person.
///
/// # Errors
///
/// [`ErrorCode::Internal`] when the runtime never started.
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn web_pairings(state: State<'_, RuntimeState>) -> Result<Vec<PendingPairDto>, AppError> {
    let web = state.web()?;
    Ok(web
        .pending_pairs()
        .into_iter()
        .map(|pair| PendingPairDto {
            pair_id: pair.pair_id,
            code: pair.code,
            name: pair.name,
            platform: pair.platform,
            address: pair.address,
            created_at_ms: pair.created_at_ms,
            expires_at_ms: pair.expires_at_ms,
        })
        .collect())
}

/// Approves one pairing request by its id.
///
/// # Errors
///
/// [`ErrorCode::NotFound`] for an id no request holds, [`ErrorCode::Conflict`] when it expired or
/// was already answered.
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub async fn web_approve(
    state: State<'_, RuntimeState>,
    pair_id: String,
    allow_acquire: Option<bool>,
) -> Result<PairedDeviceDto, AppError> {
    let device = state
        .web()?
        .approve(&pair_id, allow_acquire.unwrap_or(false))
        .await
        .map_err(web_error)?;
    Ok(device_dto(device))
}

/// Approves whichever request shows `code`.
///
/// # Errors
///
/// [`ErrorCode::NotFound`] when no request shows it, and the same refusals as [`web_approve`].
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub async fn web_approve_code(
    state: State<'_, RuntimeState>,
    code: String,
    allow_acquire: Option<bool>,
) -> Result<PairedDeviceDto, AppError> {
    let device = state
        .web()?
        .approve_code(&code, allow_acquire.unwrap_or(false))
        .await
        .map_err(web_error)?;
    Ok(device_dto(device))
}

/// Refuses one pairing request.
///
/// # Errors
///
/// [`ErrorCode::NotFound`] for an id no request holds.
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub fn web_deny(state: State<'_, RuntimeState>, pair_id: String) -> Result<(), AppError> {
    state.web()?.deny(&pair_id).map_err(web_error)
}

/// Every paired device, revoked ones included.
///
/// # Errors
///
/// [`ErrorCode::Internal`] when the read fails.
#[tauri::command]
#[specta::specta]
pub async fn web_devices(state: State<'_, RuntimeState>) -> Result<Vec<PairedDeviceDto>, AppError> {
    let devices = state.web()?.devices().await.map_err(web_error)?;
    Ok(devices.into_iter().map(device_dto).collect())
}

/// Revokes one device, refusing its next request and dropping its playback sessions.
///
/// # Errors
///
/// [`ErrorCode::Internal`] when the write fails.
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub async fn web_revoke(
    state: State<'_, RuntimeState>,
    device: DeviceRef,
) -> Result<bool, AppError> {
    state.web()?.revoke(device.id).await.map_err(web_error)
}

fn status_dto(status: &WebStatus) -> WebStatusDto {
    WebStatusDto {
        running: status.running,
        address: status.address.map(|address| address.ip().to_string()),
        port: status.address.map(|address| address.port()),
        pending: u32::try_from(status.pending).unwrap_or(u32::MAX),
    }
}

fn device_dto(device: PairedDevice) -> PairedDeviceDto {
    PairedDeviceDto {
        id: device.id,
        name: device.name,
        platform: device.platform,
        scopes: device.scopes,
        created_at: device.created_at,
        last_seen_at: device.last_seen_at,
        last_address: device.last_address,
        revoked: device.revoked_at.is_some(),
    }
}

fn web_error(error: WebError) -> AppError {
    match error {
        WebError::UnspecifiedBind => AppError::new(ErrorCode::InvalidInput)
            .with_detail("Choose one address to bind; every interface at once is not offered."),
        WebError::AlreadyRunning => {
            AppError::new(ErrorCode::Conflict).with_detail("The LAN surface is already listening.")
        }
        WebError::Bind { address, .. } => AppError::new(ErrorCode::Conflict).with_detail(format!(
            "{address} could not be bound; check the address and the port."
        )),
        WebError::PairUnknown => AppError::new(ErrorCode::NotFound)
            .with_detail("That pairing request is no longer waiting."),
        WebError::PairExpired => AppError::new(ErrorCode::Conflict)
            .with_detail("That pairing request expired; ask the TV to try again."),
        WebError::PairNotPending => AppError::new(ErrorCode::Conflict)
            .with_detail("That pairing request was already answered."),
        WebError::RateLimited => AppError::new(ErrorCode::Conflict)
            .with_detail("That address asked too many times; wait a minute."),
        WebError::PairingFull => AppError::new(ErrorCode::Conflict)
            .with_detail("Too many pairing requests are waiting; answer or dismiss one."),
        WebError::Unauthorized => AppError::new(ErrorCode::PermissionDenied)
            .with_detail("That device is not paired, or its access was revoked."),
        WebError::NoFile => {
            AppError::new(ErrorCode::NotFound).with_detail("That title has no copy on disk.")
        }
        WebError::UnknownTitle => {
            AppError::new(ErrorCode::NotFound).with_detail("That title is not in the library.")
        }
        WebError::TranscodeRequired => AppError::new(ErrorCode::Unsupported)
            .with_detail("That file needs a transcode this build does not have yet."),
        WebError::Unsupported => AppError::new(ErrorCode::Unsupported)
            .with_detail("A browser cannot demux that container."),
        other => AppError::new(ErrorCode::Internal).with_cause(other.to_string()),
    }
}

/// The local address the default route would use, without sending a packet.
///
/// `UdpSocket::connect` only selects a route; it does not put anything on the wire. The address is
/// offered as a default in the Settings field and is never bound without the person confirming it.
fn detected_lan_address() -> Option<IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    socket.local_addr().ok().map(|address| address.ip())
}

#[cfg(test)]
mod tests {
    use super::web_error;
    use crate::error::ErrorCode;
    use peregrine_app::WebError;

    #[test]
    fn a_bad_bind_is_an_input_error_and_a_taken_port_is_a_conflict() {
        assert_eq!(
            web_error(WebError::UnspecifiedBind).code,
            ErrorCode::InvalidInput
        );
        assert_eq!(
            web_error(WebError::AlreadyRunning).code,
            ErrorCode::Conflict
        );
    }

    #[test]
    fn an_unknown_device_is_a_permission_refusal_not_a_not_found() {
        assert_eq!(
            web_error(WebError::Unauthorized).code,
            ErrorCode::PermissionDenied
        );
    }
}
