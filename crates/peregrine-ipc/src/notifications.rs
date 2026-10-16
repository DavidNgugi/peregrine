//! The notification centre namespace: the rows R31 makes the record.
//!
//! A notification is written by the core and read here; the OS toast is a side effect that
//! `PG-E1-14` owns, so this surface is how a user sees one at all today.

use peregrine_app::{CentreEntry, Severity};
use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;
use tauri::State;

use crate::error::{AppError, ErrorCode};
use crate::guard::guard_response;
use crate::runtime::RuntimeState;

/// One centre row, as the shell draws it.
///
/// No magnet, path, or secret can appear here: the row stores rendered copy that the domain
/// already scanned before it was written (R11/R31).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NotificationDto {
    /// The row id.
    #[specta(type = Number)]
    pub id: i64,
    /// The catalogue event's key.
    pub event: String,
    /// `normal` or `critical`, the stored spelling.
    pub severity: String,
    /// The stored title.
    pub title: String,
    /// The stored body, when there is one.
    pub body: Option<String>,
    /// The app route this notification opens, when it names one.
    pub deep_link: Option<String>,
    /// When it was created, in Unix milliseconds UTC.
    #[specta(type = Number)]
    pub created_at: i64,
    /// Whether the user has read it.
    pub read: bool,
}

impl NotificationDto {
    fn of(entry: CentreEntry) -> Self {
        Self {
            id: entry.id.0,
            event: entry.event,
            severity: match entry.severity {
                Severity::Normal => "normal".to_owned(),
                Severity::Critical => "critical".to_owned(),
            },
            title: entry.title,
            body: entry.body,
            deep_link: entry.deep_link,
            created_at: entry.created_at,
            read: entry.read,
        }
    }
}

/// The most recent centre rows, newest first.
///
/// # Errors
///
/// `INTERNAL` when the centre could not be read; the cause is logged, and the message stays
/// generic because a storage failure is not the user's to act on.
#[tauri::command]
#[specta::specta]
pub async fn list_notifications(
    state: State<'_, RuntimeState>,
    limit: u32,
) -> Result<Vec<NotificationDto>, AppError> {
    let runtime = state.runtime()?;
    let rows = runtime
        .list_notifications(limit.clamp(1, 100))
        .await
        .map_err(|error| AppError::new(ErrorCode::Internal).with_cause(error.to_string()))?;
    Ok(guard_response(
        "notifications::list",
        rows.into_iter().map(NotificationDto::of).collect(),
    ))
}
