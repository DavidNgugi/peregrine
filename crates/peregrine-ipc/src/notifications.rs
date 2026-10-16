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

/// A centre row's id, as the listing returned it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NotificationRef {
    /// The centre row's id.
    #[specta(type = Number)]
    pub id: i64,
}

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
    /// The subject title's kind (`movie` or `show`), when the notification names a title.
    ///
    /// Carried explicitly because the stored deep link has no kind, and the app's title route
    /// needs one: guessing would open the wrong screen.
    pub title_kind: Option<String>,
    /// The subject title's row id, when the notification names a title.
    #[specta(type = Option<Number>)]
    pub subject_id: Option<i64>,
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
            title_kind: entry.title_kind.map(|kind| kind.as_str().to_owned()),
            subject_id: entry.subject_id,
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

/// Marks one centre row read.
///
/// # Errors
///
/// `INTERNAL` when the centre could not be written; the cause is logged and the message stays
/// generic.
#[tauri::command]
#[specta::specta]
pub async fn mark_notification_read(
    state: State<'_, RuntimeState>,
    notification: NotificationRef,
) -> Result<bool, AppError> {
    let runtime = state.runtime()?;
    runtime
        .mark_notification_read(notification.id)
        .await
        .map_err(|error| AppError::new(ErrorCode::Internal).with_cause(error.to_string()))
}

/// Marks every unread centre row read, answering how many changed.
///
/// # Errors
///
/// `INTERNAL` when the centre could not be written.
#[tauri::command]
#[specta::specta]
pub async fn mark_all_notifications_read(state: State<'_, RuntimeState>) -> Result<u32, AppError> {
    let runtime = state.runtime()?;
    let changed = runtime
        .mark_all_notifications_read()
        .await
        .map_err(|error| AppError::new(ErrorCode::Internal).with_cause(error.to_string()))?;
    Ok(u32::try_from(changed).unwrap_or(u32::MAX))
}

/// Dismisses one centre row.
///
/// # Errors
///
/// `INTERNAL` when the centre could not be written.
#[tauri::command]
#[specta::specta]
pub async fn dismiss_notification(
    state: State<'_, RuntimeState>,
    notification: NotificationRef,
) -> Result<bool, AppError> {
    let runtime = state.runtime()?;
    runtime
        .dismiss_notification(notification.id)
        .await
        .map_err(|error| AppError::new(ErrorCode::Internal).with_cause(error.to_string()))
}
