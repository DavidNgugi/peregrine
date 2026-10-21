//! The backup commands: write a copy of the household's data, and stage a restore.
//!
//! Both are thin: the runtime owns where the file goes and what is in it. A restore restarts
//! the app, because the live database has a writer task and a reader pool and the swap has to
//! happen before either exists (ADR 0033). The restart is requested only after the staged file
//! validated and was copied aside, so a refusal leaves the installation running.

use std::path::Path;

use peregrine_app::BackupInfo;
use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;
use tauri::{AppHandle, State};

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

/// Writes a backup of this installation into a folder the user chose.
///
/// # Errors
///
/// [`AppError`] with `InvalidInput` when the destination is not a folder, and
/// `PermissionDenied` when the file cannot be written.
#[tauri::command]
#[specta::specta]
pub async fn create_backup(
    state: State<'_, RuntimeState>,
    directory: String,
) -> Result<BackupDto, AppError> {
    let report = state
        .runtime()?
        .create_backup(Path::new(&directory))
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
    let info = state.runtime()?.restore_backup(Path::new(&source)).await?;
    let restored = RestoreDto::from(info);
    app.request_restart();
    Ok(guard_response("backup::restore_backup", restored))
}
