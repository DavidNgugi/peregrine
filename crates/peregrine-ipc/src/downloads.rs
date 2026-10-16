//! The downloads namespace: acquire one title, and move a queued job.
//!
//! Acquire is a command that starts a job, not a command that downloads: the indexer search, the
//! metadata fetch, and the engine's add all leave the machine, so the work belongs to the job
//! (AGENTS R10) and this layer only names the title. The three move commands are thin on purpose —
//! the state machine and the engine calls are the service's, and this layer names an id and maps
//! the answer.

use peregrine_app::DownloadJobRow;
use peregrine_app::acquisition::AcquisitionError;
use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;
use tauri::State;

use crate::error::{AppError, ErrorCode};
use crate::guard::guard_response;
use crate::jobs::JobStarted;
use crate::library::TitleRef;
use crate::runtime::RuntimeState;

/// A download job's id, as a listing returned it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DownloadRef {
    /// The download job's row id.
    #[specta(type = Number)]
    pub id: i64,
}

/// One rule the safety gate reported, as a screen says it.
///
/// The reason is the domain's stable spelling rather than a sentence: the UI owns the wording, and
/// renaming a reason is a data migration, so the wire must not bake in a label.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SafetyFindingDto {
    /// The rule's stable spelling, which the screen turns into a sentence.
    pub reason: String,
    /// The entry the rule applies to, when it names one.
    pub entry: Option<String>,
}

/// One download job, as the Downloads screen draws it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DownloadJobDto {
    /// The row id, which every move command takes.
    #[specta(type = Number)]
    pub id: i64,
    /// The title this download fills.
    #[specta(type = Number)]
    pub title_id: i64,
    /// The title's display name, when the library can answer it.
    pub title: Option<String>,
    /// The release the indexer advertised.
    pub release_title: String,
    /// The state machine's current state, as its stored spelling.
    pub state: String,
    /// Completion in permille, where 1000 is the whole payload.
    #[specta(type = Number)]
    pub progress_permille: i64,
    /// The size the indexer claimed, when it claimed one.
    #[specta(type = Option<Number>)]
    pub reported_size_bytes: Option<u64>,
    /// Payload bytes received so far.
    #[specta(type = Number)]
    pub downloaded_bytes: u64,
    /// Last reported download rate, in bytes per second.
    #[specta(type = Number)]
    pub download_rate_bps: u64,
    /// Peers the last statistics tick was connected to.
    pub peers_connected: u32,
    /// Peers the swarm was believed to hold.
    pub peers_swarm: u32,
    /// Seconds the transfer was expected to take, when there was an estimate.
    #[specta(type = Option<Number>)]
    pub eta_seconds: Option<u64>,
    /// Unix milliseconds when the job was admitted.
    #[specta(type = Number)]
    pub added_at: i64,
    /// The engine's stable failure code, when it failed.
    pub error_code: Option<String>,
    /// The engine's failure message, already redacted.
    pub error_message: Option<String>,
    /// The safety gate's findings, read from the report the row persists. Non-empty for a row the
    /// gate parked as `blocked`, which is the only way a screen can explain the refusal.
    pub findings: Vec<SafetyFindingDto>,
}

impl DownloadJobDto {
    /// The row, plus the title name a read supplied when it could.
    fn of(row: &DownloadJobRow, title: Option<String>) -> Self {
        Self {
            id: row.id,
            title_id: row.title_id,
            title,
            release_title: row.release_title.clone(),
            state: row.state.as_str().to_owned(),
            progress_permille: row.progress_permille,
            reported_size_bytes: row.reported_size_bytes,
            downloaded_bytes: row.downloaded_bytes,
            download_rate_bps: row.download_rate_bps,
            peers_connected: row.peers_connected,
            peers_swarm: row.peers_swarm,
            eta_seconds: row.eta_seconds,
            added_at: row.added_at.as_millisecond(),
            error_code: row.error_code.clone(),
            error_message: row.error_message.clone(),
            findings: row
                .safety_report
                .as_ref()
                .map(|report| {
                    report
                        .findings()
                        .iter()
                        .map(|finding| SafetyFindingDto {
                            reason: finding.reason().as_str().to_owned(),
                            entry: finding.entry().map(str::to_owned),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

/// The title's display name, when the library can answer it; a missing name is not a failed list.
async fn title_name(runtime: &peregrine_app::runtime::Runtime, title_id: i64) -> Option<String> {
    runtime
        .library_title(title_id)
        .await
        .ok()
        .flatten()
        .map(|detail| detail.entry.row.title.name)
}

/// Maps an acquisition refusal onto the wire error.
///
/// The sentence is the service's own, so a screen says what happened rather than a code's short
/// word; only the code is chosen here.
fn acquisition_error(error: AcquisitionError) -> AppError {
    match error {
        AcquisitionError::TitleUnavailable | AcquisitionError::NotFound => {
            AppError::new(ErrorCode::NotFound).with_detail(error.to_string())
        }
        AcquisitionError::Duplicate => {
            AppError::new(ErrorCode::Conflict).with_detail(error.to_string())
        }
        AcquisitionError::OptInStillRequired | AcquisitionError::NoOverride => {
            AppError::new(ErrorCode::PermissionDenied).with_detail(error.to_string())
        }
        AcquisitionError::DiskFull { .. } => {
            AppError::new(ErrorCode::InvalidInput).with_detail(error.to_string())
        }
        AcquisitionError::UnsupportedSource => {
            AppError::new(ErrorCode::Unsupported).with_detail(error.to_string())
        }
        AcquisitionError::SavePath { .. }
        | AcquisitionError::SpaceUnknown { .. }
        | AcquisitionError::Engine(_)
        | AcquisitionError::AddRefused(_)
        | AcquisitionError::TransitionRefused { .. }
        | AcquisitionError::Storage(_) => {
            AppError::new(ErrorCode::Internal).with_detail(error.to_string())
        }
    }
}

/// Starts an automatic acquisition for one title, and answers the job doing it.
///
/// # Errors
///
/// ErrorCode::Internal when the runtime is not running or the journal refuses the row, and
/// ErrorCode::Unsupported when the torrent session did not start.
#[tauri::command]
#[specta::specta]
pub async fn acquire(
    state: State<'_, RuntimeState>,
    title: TitleRef,
    min_seeders: Option<u32>,
) -> Result<JobStarted, AppError> {
    let started = state
        .runtime()?
        .start_acquire(
            peregrine_app::runtime::installation_scope(),
            title.id,
            min_seeders.unwrap_or(1),
        )
        .await?;
    Ok(guard_response(
        "downloads::acquire",
        JobStarted::from(started),
    ))
}

/// The download queue, newest first.
#[tauri::command]
#[specta::specta]
pub async fn list_downloads(
    state: State<'_, RuntimeState>,
) -> Result<Vec<DownloadJobDto>, AppError> {
    let runtime = state.runtime()?;
    let rows = runtime.download_jobs().await?;
    let mut downloads = Vec::with_capacity(rows.len());
    for row in &rows {
        downloads.push(DownloadJobDto::of(
            row,
            title_name(runtime, row.title_id).await,
        ));
    }
    Ok(guard_response("downloads::list", downloads))
}

/// Pauses one download and answers its row as it stands after the move.
#[tauri::command]
#[specta::specta]
pub async fn pause_download(
    state: State<'_, RuntimeState>,
    download: DownloadRef,
) -> Result<DownloadJobDto, AppError> {
    let runtime = state.runtime()?;
    runtime
        .acquisition()?
        .pause(download.id)
        .await
        .map_err(acquisition_error)?;
    let row = row_after(runtime, download.id).await?;
    let title = title_name(runtime, row.title_id).await;
    Ok(guard_response(
        "downloads::pause",
        DownloadJobDto::of(&row, title),
    ))
}

/// Resumes one stopped download.
#[tauri::command]
#[specta::specta]
pub async fn resume_download(
    state: State<'_, RuntimeState>,
    download: DownloadRef,
) -> Result<DownloadJobDto, AppError> {
    let runtime = state.runtime()?;
    runtime
        .acquisition()?
        .resume(download.id)
        .await
        .map_err(acquisition_error)?;
    let row = row_after(runtime, download.id).await?;
    let title = title_name(runtime, row.title_id).await;
    Ok(guard_response(
        "downloads::resume",
        DownloadJobDto::of(&row, title),
    ))
}

/// Removes one download.
#[tauri::command]
#[specta::specta]
pub async fn remove_download(
    state: State<'_, RuntimeState>,
    download: DownloadRef,
) -> Result<DownloadJobDto, AppError> {
    let runtime = state.runtime()?;
    runtime
        .acquisition()?
        .remove(download.id)
        .await
        .map_err(acquisition_error)?;
    let row = row_after(runtime, download.id).await?;
    let title = title_name(runtime, row.title_id).await;
    Ok(guard_response(
        "downloads::remove",
        DownloadJobDto::of(&row, title),
    ))
}

/// The row a move left behind, or a typed refusal when it vanished.
async fn row_after(
    runtime: &peregrine_app::runtime::Runtime,
    id: i64,
) -> Result<DownloadJobRow, AppError> {
    runtime.download_job(id).await?.ok_or_else(|| {
        AppError::new(ErrorCode::NotFound).with_detail("That download no longer exists.")
    })
}
