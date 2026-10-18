//! What this machine looks like from the internet, asked only when the reader asks.
//!
//! The command exists for one job: confirming that the VPN or tunnel a person started is actually
//! carrying their traffic. It is not a health check, it runs on no timer, and the answer is not
//! cached or logged (AGENTS R23).

use tauri::State;

use crate::error::AppError;
use crate::guard::guard_response;
use crate::runtime::RuntimeState;

/// The address the internet sees, or a refusal that says the check could not be made.
///
/// # Errors
///
/// [`AppError`] when the runtime is not running or the echo request fails.
#[tauri::command]
#[specta::specta]
pub async fn public_ip(state: State<'_, RuntimeState>) -> Result<String, AppError> {
    let ip = state.runtime()?.public_ip().await?;
    Ok(guard_response("network::public_ip", ip))
}
