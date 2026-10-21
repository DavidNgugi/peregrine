//! The backup commands: write a copy of the household's data, and stage a restore.
//!
//! Both are thin: the runtime owns where the file goes and what is in it. A restore restarts
//! the app, because the live database has a writer task and a reader pool and the swap has to
//! happen before either exists (ADR 0033). The restart is requested only after the staged file
//! validated and was copied aside, so a refusal leaves the installation running.

use std::path::Path;

use peregrine_app::BackupInfo;
use serde::{Deserialize, Serialize};
use serde_json::json;
use specta::Type;
use specta_typescript::Number;
use tauri::{AppHandle, Emitter as _, State};

use crate::error::AppError;
use crate::guard::guard_response;
use crate::runtime::RuntimeState;

/// What a backup that was just written carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BackupDto {
    /// Where the file is, as the user chose the folder.
    pub path: String,
    /// How large the file is.
    #[specta(type = Number)]
    pub bytes: u64,
    /// The newest migration the copy holds.
    #[specta(type = Number)]
    pub schema_version: i64,
}

/// What a staged restore found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RestoreDto {
    /// The newest migration the staged copy holds, which the next launch will migrate.
    #[specta(type = Number)]
    pub schema_version: i64,
}

impl From<BackupInfo> for RestoreDto {
    fn from(info: BackupInfo) -> Self {
        Self {
            schema_version: info.schema_version,
        }
    }
}

/// The topic the Settings section listens on while a copy runs.
const PROGRESS_TOPIC: &str = "backup://progress";

/// The versioned envelope every progress event carries.
///
/// The same `{ schemaVersion, payload }` shape every other topic uses, so the webview's one
/// reader accepts it and a payload from a build it does not understand is dropped rather than
/// rendered as `undefined`. The events are phase boundaries, not a stream: there are at most
/// four per backup, which is less work than the spinner they accompany.
fn progress_payload(
    operation: &'static str,
    phase: &'static str,
    permille: u16,
) -> serde_json::Value {
    json!({
        "schemaVersion": 1,
        "payload": {
            "operation": operation,
            "phase": phase,
            "permille": permille,
        },
    })
}

/// Publishes one phase of a backup or restore.
fn emit_progress<R: tauri::Runtime>(
    app: &AppHandle<R>,
    operation: &'static str,
    phase: &'static str,
    permille: u16,
) {
    let _ = app.emit(PROGRESS_TOPIC, progress_payload(operation, phase, permille));
}

/// Writes a backup of this installation into a folder the user chose.
///
/// # Errors
///
/// [`AppError`] with `InvalidInput` when the destination is not a folder, and
/// `PermissionDenied` when the file cannot be written.
#[tauri::command]
#[specta::specta]
pub async fn create_backup<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, RuntimeState>,
    directory: String,
) -> Result<BackupDto, AppError> {
    let report = state
        .runtime()?
        .create_backup_with_progress(Path::new(&directory), &|phase| {
            emit_progress(&app, "backup", phase.as_str(), phase.permille());
        })
        .await?;
    Ok(guard_response(
        "backup::create_backup",
        BackupDto {
            path: report.path.display().to_string(),
            bytes: report.bytes,
            schema_version: report.schema_version,
        },
    ))
}

/// Validates a chosen backup, stages it, and restarts so the next launch opens it.
///
/// # Errors
///
/// [`AppError`] with `InvalidInput` when the file is not a backup this build can restore,
/// and `PermissionDenied` when it cannot be copied. Nothing changes on either path.
#[tauri::command]
#[specta::specta]
pub async fn restore_backup<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, RuntimeState>,
    source: String,
) -> Result<RestoreDto, AppError> {
    let info = state
        .runtime()?
        .restore_backup_with_progress(Path::new(&source), &|phase| {
            emit_progress(&app, "restore", phase.as_str(), phase.permille());
        })
        .await?;
    let restored = RestoreDto::from(info);
    app.request_restart();
    Ok(guard_response("backup::restore_backup", restored))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The webview's reader accepts one envelope shape, so this pins the keys the Settings
    /// section parses rather than trusting the `json!` call site to stay in step.
    #[test]
    fn progress_payload_is_versioned_and_names_the_phase() {
        let value = progress_payload("backup", "cleaning", 650);

        assert_eq!(value["schemaVersion"], 1);
        assert_eq!(value["payload"]["operation"], "backup");
        assert_eq!(value["payload"]["phase"], "cleaning");
        assert_eq!(value["payload"]["permille"], 650);
        assert_eq!(PROGRESS_TOPIC, "backup://progress");
    }
}
