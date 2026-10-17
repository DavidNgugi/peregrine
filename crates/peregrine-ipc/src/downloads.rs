//! The downloads namespace: acquire one title, and move a queued job.
//!
//! Acquire is a command that starts a job, not a command that downloads: the indexer search, the
//! metadata fetch, and the engine's add all leave the machine, so the work belongs to the job
//! (AGENTS R10) and this layer only names the title. The three move commands are thin on purpose —
//! the state machine and the engine calls are the service's, and this layer names an id and maps
//! the answer.

use peregrine_app::DeletionPlan;
use peregrine_app::DownloadJobRow;
use peregrine_app::PreferredQuality;
use peregrine_app::TrashError;
use peregrine_app::acquisition::AcquisitionError;
use peregrine_app::jobs::acquire::EpisodePick;
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

/// Which of the two a download's title is, as the title route spells it.
///
/// The route names the kind because the page names itself from the address before its read answers
/// (`PG-E7-02`), so a screen that opens a title needs it. It is the domain's `TitleKind` as the wire
/// spells it: `movie` and `show`, not the library root's `movies` and `shows`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum DownloadTitleKind {
    /// A film.
    Movie,
    /// A series.
    Show,
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
    /// Which of the two the title is, when the library can answer it.
    ///
    /// The Downloads screen opens the title's own route from a row, and that route carries the kind
    /// because the page names itself before its read answers. `null` is a title neither the library
    /// nor a provider can resolve, and such a row opens nothing rather than a route that guesses.
    pub title_kind: Option<DownloadTitleKind>,
    /// The cached poster's absolute path, when the cache holds one.
    ///
    /// A path and never bytes (AGENTS R11). It is carried so the Discover Recent Downloads rail
    /// can draw the same tile Browse draws; null falls back to the placeholder.
    pub poster: Option<String>,
    /// The episode's season, when the job was started for one episode.
    #[specta(type = Option<Number>)]
    pub season_number: Option<u32>,
    /// The episode's number within that season.
    #[specta(type = Option<Number>)]
    pub episode_number: Option<u32>,
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
    /// The row, plus the title name and kind a read supplied when it could.
    fn of(
        row: &DownloadJobRow,
        title: Option<String>,
        title_kind: Option<DownloadTitleKind>,
        poster: Option<String>,
    ) -> Self {
        Self {
            id: row.id,
            title_id: row.title_id,
            title,
            title_kind,
            poster,
            season_number: None,
            episode_number: None,
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

/// One file a deletion would remove, as the confirmation screen lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DeletionEntryDto {
    /// The absolute path the plan enumerated.
    pub path: String,
    /// The file size in bytes, as the plan measured it.
    #[specta(type = Number)]
    pub bytes: u64,
}

/// Exactly what one deletion would remove, before the user confirms it (AGENTS R22).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DeletionPlanDto {
    /// The download job the plan belongs to.
    #[specta(type = Number)]
    pub job_id: i64,
    /// The files, in the repository order.
    pub entries: Vec<DeletionEntryDto>,
    /// The sum of every entry size.
    #[specta(type = Number)]
    pub total_bytes: u64,
}

impl DeletionPlanDto {
    /// The service plan, as a screen reads it.
    fn of(plan: &DeletionPlan) -> Self {
        Self {
            job_id: plan.job_id,
            entries: plan
                .entries
                .iter()
                .map(|entry| DeletionEntryDto {
                    path: entry.path.as_str().to_owned(),
                    bytes: entry.bytes,
                })
                .collect(),
            total_bytes: plan.total_bytes,
        }
    }
}

/// The title's display name and kind, when the library can answer them; a missing title is not a
/// failed list.
///
/// One `library_title` call answers both, and its `LEFT JOIN` is what lets a title the library holds
/// nothing of — a download that has not landed yet — still resolve: without it a queued row would
/// have no name and no route to open.
async fn title_ref(
    runtime: &peregrine_app::runtime::Runtime,
    title_id: i64,
) -> (Option<String>, Option<DownloadTitleKind>, Option<String>) {
    match runtime.library_title(title_id).await.ok().flatten() {
        Some(detail) => {
            let title = detail.entry.row.title;
            let kind = match title.kind {
                peregrine_app::TitleKind::Movie => DownloadTitleKind::Movie,
                peregrine_app::TitleKind::Show => DownloadTitleKind::Show,
            };
            (
                Some(title.name),
                Some(kind),
                crate::library::artwork_text(detail.entry.poster.as_deref()),
            )
        }
        None => (None, None, None),
    }
}

/// Maps an acquisition refusal onto the wire error.
///
/// The sentence is the service's own, so a screen says what happened rather than a code's short
/// word; only the code is chosen here.
fn acquisition_error(error: &AcquisitionError) -> AppError {
    match error {
        AcquisitionError::TitleUnavailable | AcquisitionError::NotFound => {
            AppError::new(ErrorCode::NotFound).with_detail(error.to_string())
        }
        AcquisitionError::Duplicate | AcquisitionError::NotFinished => {
            AppError::new(ErrorCode::Conflict).with_detail(error.to_string())
        }
        AcquisitionError::OptInStillRequired | AcquisitionError::NoOverride => {
            AppError::new(ErrorCode::PermissionDenied).with_detail(error.to_string())
        }
        AcquisitionError::DiskFull { .. } => {
            AppError::new(ErrorCode::InvalidInput).with_detail(error.to_string())
        }
        AcquisitionError::UnsupportedSource | AcquisitionError::Trash(TrashError::Unavailable) => {
            AppError::new(ErrorCode::Unsupported).with_detail(error.to_string())
        }
        AcquisitionError::SavePath { .. }
        | AcquisitionError::SpaceUnknown { .. }
        | AcquisitionError::Engine(_)
        | AcquisitionError::AddRefused(_)
        | AcquisitionError::Trash(TrashError::Refused)
        | AcquisitionError::TransitionRefused { .. }
        | AcquisitionError::Storage(_) => {
            AppError::new(ErrorCode::Internal).with_detail(error.to_string())
        }
    }
}

/// Reads an episode reference, refusing one half of it rather than guessing the other.
///
/// A season without an episode would acquire the whole season and an episode without one has no
/// season to be an episode of, so neither is a request the job can honour.
pub(crate) fn episode_pick(
    season: Option<u32>,
    episode: Option<u32>,
) -> Result<Option<EpisodePick>, AppError> {
    match (season, episode) {
        (Some(season), Some(episode)) => Ok(Some(EpisodePick { season, episode })),
        (None, None) => Ok(None),
        _ => Err(AppError::new(ErrorCode::InvalidInput)
            .with_detail("A season and an episode must be given together, or neither.")),
    }
}

/// Starts an automatic acquisition for one title, or for one episode of it, and answers the job
/// doing it.
///
/// `season` and `episode` are given together to acquire a single episode; both omitted acquires
/// the title as a whole. Giving one without the other is refused rather than guessed.
///
/// # Errors
///
/// `ErrorCode::InvalidInput` when only one of the two is given, `ErrorCode::Internal` when the
/// runtime is not running or the journal refuses the row, and `ErrorCode::Unsupported` when the
/// torrent session did not start.
#[tauri::command]
#[specta::specta]
pub async fn acquire(
    state: State<'_, RuntimeState>,
    title: TitleRef,
    min_seeders: Option<u32>,
    season: Option<u32>,
    episode: Option<u32>,
) -> Result<JobStarted, AppError> {
    let episode = episode_pick(season, episode)?;
    let started = state
        .runtime()?
        .start_acquire(
            peregrine_app::runtime::installation_scope(),
            title.id,
            episode,
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
        let (title, title_kind, poster) = title_ref(runtime, row.title_id).await;
        let mut dto = DownloadJobDto::of(row, title, title_kind, poster);
        let episode = match row.episode_id {
            Some(id) => runtime.episode_numbers(id).await.ok().flatten(),
            None => None,
        };
        if let Some((season, episode)) = episode {
            dto.season_number = Some(season);
            dto.episode_number = Some(episode);
        }
        downloads.push(dto);
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
        .map_err(|error| acquisition_error(&error))?;
    let row = row_after(runtime, download.id).await?;
    let (title, title_kind, poster) = title_ref(runtime, row.title_id).await;
    Ok(guard_response(
        "downloads::pause",
        DownloadJobDto::of(&row, title, title_kind, poster),
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
        .map_err(|error| acquisition_error(&error))?;
    let row = row_after(runtime, download.id).await?;
    let (title, title_kind, poster) = title_ref(runtime, row.title_id).await;
    Ok(guard_response(
        "downloads::resume",
        DownloadJobDto::of(&row, title, title_kind, poster),
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
        .map_err(|error| acquisition_error(&error))?;
    let row = row_after(runtime, download.id).await?;
    let (title, title_kind, poster) = title_ref(runtime, row.title_id).await;
    Ok(guard_response(
        "downloads::remove",
        DownloadJobDto::of(&row, title, title_kind, poster),
    ))
}

/// Clears one finished download from the queue.
///
/// The bytes already written stay on disk; this removes the queue entry only. A row that has not
/// finished is refused rather than cleared, and an entry that is already gone answers false.
#[tauri::command]
#[specta::specta]
pub async fn dismiss_download(
    state: State<'_, RuntimeState>,
    download: DownloadRef,
) -> Result<bool, AppError> {
    let runtime = state.runtime()?;
    let removed = runtime
        .acquisition()?
        .dismiss(download.id)
        .await
        .map_err(|error| acquisition_error(&error))?;
    Ok(guard_response("downloads::dismiss", removed))
}

/// Clears every finished download from the queue, answering how many went.
#[tauri::command]
#[specta::specta]
pub async fn clear_finished_downloads(state: State<'_, RuntimeState>) -> Result<u32, AppError> {
    let runtime = state.runtime()?;
    let removed = runtime
        .acquisition()?
        .clear_finished()
        .await
        .map_err(|error| acquisition_error(&error))?;
    Ok(guard_response(
        "downloads::clear_finished",
        u32::try_from(removed).unwrap_or(u32::MAX),
    ))
}

/// Enumerates what deleting one download would remove, without removing anything.
///
/// The confirmation screen shows this list and its byte total, so what the user agrees to is what
/// actually goes (AGENTS R22).
#[tauri::command]
#[specta::specta]
pub async fn plan_download_deletion(
    state: State<'_, RuntimeState>,
    download: DownloadRef,
) -> Result<DeletionPlanDto, AppError> {
    let runtime = state.runtime()?;
    let plan = runtime
        .acquisition()?
        .deletion_plan(download.id)
        .await
        .map_err(|error| acquisition_error(&error))?;
    Ok(guard_response(
        "downloads::plan_deletion",
        DeletionPlanDto::of(&plan),
    ))
}

/// Moves one download files to the operating system trash and clears its queue entry.
///
/// The list is resolved again inside the service, so a path that changed since the plan fails the
/// whole act rather than deleting something the user did not confirm.
#[tauri::command]
#[specta::specta]
pub async fn delete_download(
    state: State<'_, RuntimeState>,
    download: DownloadRef,
) -> Result<DeletionPlanDto, AppError> {
    let runtime = state.runtime()?;
    let plan = runtime
        .acquisition()?
        .delete_files(download.id)
        .await
        .map_err(|error| acquisition_error(&error))?;
    Ok(guard_response(
        "downloads::delete",
        DeletionPlanDto::of(&plan),
    ))
}

/// The quality downloads prefer, as its wire spelling, or null when the stored list names none.
#[tauri::command]
#[specta::specta]
pub async fn preferred_quality(state: State<'_, RuntimeState>) -> Result<Option<String>, AppError> {
    let runtime = state.runtime()?;
    runtime
        .preferred_quality()
        .await
        .map(|quality| quality.map(|quality| quality.as_str().to_owned()))
        .map_err(AppError::from)
}

/// Records the quality downloads should prefer.
///
/// The choice is the default profile's resolution list, so this is the one control that decides
/// whether a 4K release is a candidate at all.
#[tauri::command]
#[specta::specta]
pub async fn set_preferred_quality(
    state: State<'_, RuntimeState>,
    quality: String,
) -> Result<(), AppError> {
    let runtime = state.runtime()?;
    let parsed = PreferredQuality::parse(&quality).ok_or_else(|| {
        AppError::new(ErrorCode::InvalidInput)
            .with_detail(format!("unknown preferred quality: {quality}"))
    })?;
    runtime
        .set_preferred_quality(parsed)
        .await
        .map_err(AppError::from)
}

/// A release offer's id, as a listing returned it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct OfferRef {
    /// The row a choice names.
    #[specta(type = Number)]
    pub id: i64,
}

/// One release a search found at a quality the preference excluded.
///
/// The release's source is deliberately absent: it stays in the core and is looked up by id when
/// a choice names one, so no magnet, URL, or path crosses IPC (R11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseOfferDto {
    /// The row a choice names.
    #[specta(type = Number)]
    pub id: i64,
    /// The release name the indexer advertised.
    pub release_title: String,
    /// Which indexer it came from.
    pub indexer: Option<String>,
    /// Why the preferred quality excluded it, in the domain's own words.
    pub reason: String,
    /// The size the indexer stated.
    #[specta(type = Option<Number>)]
    pub size_bytes: Option<u64>,
    /// The seeders the indexer stated.
    pub seeders: Option<u32>,
}

/// The offers a search left for one title.
#[tauri::command]
#[specta::specta]
pub async fn list_release_offers(
    state: State<'_, RuntimeState>,
    title: TitleRef,
) -> Result<Vec<ReleaseOfferDto>, AppError> {
    let runtime = state.runtime()?;
    let rows = runtime.release_offers(title.id).await?;
    Ok(guard_response(
        "downloads::list_offers",
        rows.into_iter()
            .map(|row| ReleaseOfferDto {
                id: row.id,
                release_title: row.release_title,
                indexer: row.indexer,
                reason: row.reasons_json,
                size_bytes: row.size_bytes.and_then(|value| u64::try_from(value).ok()),
                seeders: row.seeders.and_then(|value| u32::try_from(value).ok()),
            })
            .collect(),
    ))
}

/// Queues the release the user picked from the offers, without a second search.
#[tauri::command]
#[specta::specta]
pub async fn choose_release_offer(
    state: State<'_, RuntimeState>,
    offer: OfferRef,
    min_seeders: Option<u32>,
) -> Result<JobStarted, AppError> {
    let started = state
        .runtime()?
        .acquire_release(
            peregrine_app::runtime::installation_scope(),
            offer.id,
            min_seeders.unwrap_or(1),
        )
        .await?;
    Ok(guard_response(
        "downloads::choose_offer",
        JobStarted::from(started),
    ))
}

/// Dismisses one offer so a later search does not offer it again.
#[tauri::command]
#[specta::specta]
pub async fn dismiss_release_offer(
    state: State<'_, RuntimeState>,
    offer: OfferRef,
) -> Result<bool, AppError> {
    let runtime = state.runtime()?;
    let changed = runtime.dismiss_release_offer(offer.id).await?;
    Ok(guard_response("downloads::dismiss_offer", changed))
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
