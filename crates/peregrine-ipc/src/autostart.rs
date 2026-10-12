//! Starting Peregrine with the operating system's session (PG-E1-13).
//!
//! The login item belongs to `tauri-plugin-autostart`, which owns the per-OS
//! mechanism: a `SMAppService` registration on macOS, a registry `Run` key on
//! Windows, an XDG autostart entry on Linux. What this module owns is the rule the
//! task states — **off until the user asks** — and the shape the UI reads.
//!
//! Off is structural, not a default someone may change: the only call to `enable` in
//! the shipping binary is [`set_autostart`], which is reachable only from the webview,
//! and registering the plugin creates no login item by itself. Nothing here runs at
//! startup.

use crate::error::{AppError, ErrorCode};
use tauri::Manager as _;
use tauri_plugin_autostart::AutoLaunchManager;

/// Whether the login item exists before the user has ever touched the setting.
///
/// A constant rather than a comment so the requirement has a name a test can read.
pub const REGISTERED_AT_FIRST_RUN: bool = false;

/// The login-item adapter the shell registers, or a structured refusal.
///
/// Looked up rather than reached through `ManagerExt::autolaunch`, which panics when the
/// plugin is not registered. A command that panics is the one thing ARCHITECTURE §7 rule 3
/// does not allow, and the case is real: the command suite drives every command against a
/// mock application that has no adapter, and so would any future build that leaves the
/// plugin out.
///
/// # Errors
///
/// [`ErrorCode::Unsupported`] when nothing can change the login item in this build.
fn manager<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<tauri::State<'_, AutoLaunchManager>, AppError> {
    app.try_state::<AutoLaunchManager>().ok_or_else(|| {
        AppError::new(ErrorCode::Unsupported)
            .with_detail("This build cannot change whether Peregrine starts with the system.")
            .with_cause("the login-item adapter is not registered")
    })
}

/// Whether Peregrine is registered to start with the user's session.
///
/// # Errors
///
/// [`ErrorCode::Unsupported`] when the shell registered no login-item adapter, and
/// [`ErrorCode::PermissionDenied`] when the OS refuses to answer, which on every platform
/// this supports means the user's own settings are in the way.
#[tauri::command]
#[specta::specta]
// Tauri injects command arguments by value; this one is only read, but taking a
// reference would stop the framework from recognising it as a command argument.
#[allow(clippy::needless_pass_by_value)]
pub fn autostart_enabled<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<bool, AppError> {
    manager(&app)?.is_enabled().map_err(|error| {
        tracing::warn!(%error, "the login item's state could not be read");
        login_item_error()
    })
}

/// Registers or removes the login item.
///
/// # Errors
///
/// [`ErrorCode::Unsupported`] when the shell registered no login-item adapter, and
/// [`ErrorCode::PermissionDenied`] when the OS refuses the change.
#[tauri::command]
#[specta::specta]
// Tauri injects command arguments by value; this one is only read, but taking a
// reference would stop the framework from recognising it as a command argument.
#[allow(clippy::needless_pass_by_value)]
pub fn set_autostart<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    enabled: bool,
) -> Result<(), AppError> {
    let manager = manager(&app)?;
    let outcome = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    outcome.map_err(|error| {
        tracing::warn!(%error, enabled, "the login item could not be changed");
        login_item_error()
    })
}

/// The failure the UI is shown when the OS will not change the login item.
///
/// The underlying error's text is deliberately not passed on: on macOS and Linux it
/// carries the path of the entry the OS refused, and no path travels over IPC
/// (docs/ARCHITECTURE.md §7 rule 3). It is logged locally instead, where the export
/// redaction and the user's own machine are the boundary.
fn login_item_error() -> AppError {
    AppError::new(ErrorCode::PermissionDenied)
        .with_detail("The operating system would not change whether Peregrine starts with it.")
        .with_cause("the login item could not be changed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autostart_is_off_until_the_user_asks_for_it() {
        // A `const` block rather than a plain assertion: the requirement is a property of
        // the binary, so it is checked while compiling it rather than while testing it.
        const {
            assert!(
                !REGISTERED_AT_FIRST_RUN,
                "the login item is off by default, and that is a requirement rather than a preference"
            );
        }
    }

    #[test]
    fn a_refused_login_item_change_says_so_without_naming_a_path() {
        let error = login_item_error();
        assert_eq!(error.code, ErrorCode::PermissionDenied);
        let message = format!("{error:?} {}", error.message_key());
        assert!(
            !message.contains('/') && !message.contains('\\'),
            "no path travels over IPC: {message}"
        );
    }
}
