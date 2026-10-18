//! The watch namespace: what the account is part-way through, and the position a player reports.
//!
//! Two thin commands. The Continue Watching read exists so a rail draws without a call per tile, and
//! the progress write is the player's tick; neither computes a rule, because the percentage and the
//! finished decision are the domain's (I6).

use peregrine_app::WatchProgressRow;
use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;
use tauri::State;

use crate::downloads::DownloadTitleKind;
use crate::error::AppError;
use crate::guard::guard_response;
use crate::runtime::RuntimeState;

/// One in-progress title, as the Continue Watching rail draws it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ContinueWatchingDto {
    /// The title this row is about.
    #[specta(type = Number)]
    pub title_id: i64,
    /// The title's display name, when the library can answer it.
    pub title: Option<String>,
    /// Where the cached poster is, when the cache holds one (AGENTS R11 keeps it a path).
    pub poster: Option<String>,
    /// Which of the two the title is, so the rail can honour the Discover filter.
    pub kind: Option<DownloadTitleKind>,
    /// The episode, for a series; `null` for a film or a whole-title row.
    #[specta(type = Option<Number>)]
    pub episode_id: Option<i64>,
    /// The episode's season, when the row names one and the episode still exists.
    #[specta(type = Option<Number>)]
    pub season_number: Option<u32>,
    /// The episode's number within that season.
    #[specta(type = Option<Number>)]
    pub episode_number: Option<u32>,
    /// Where the account left off, in milliseconds.
    #[specta(type = Number)]
    pub position_ms: i64,
    /// The runtime at that position, when the player knew it.
    #[specta(type = Option<Number>)]
    pub duration_ms: Option<i64>,
    /// How far through, as a fraction in `0.0..=1.0`.
    pub percent: f64,
    /// When the row was last written, in Unix milliseconds.
    #[specta(type = Number)]
    pub updated_at: i64,
}

impl ContinueWatchingDto {
    /// The row, plus what this read could resolve about its title.
    fn of(
        row: WatchProgressRow,
        title: Option<String>,
        poster: Option<String>,
        kind: Option<DownloadTitleKind>,
        episode: Option<(u32, u32)>,
    ) -> Self {
        let progress = row.progress;
        Self {
            title_id: progress.title_id(),
            title,
            poster,
            kind,
            episode_id: progress.episode_id(),
            season_number: episode.map(|(season, _)| season),
            episode_number: episode.map(|(_, number)| number),
            position_ms: progress.position_ms(),
            duration_ms: progress.duration_ms(),
            percent: progress.percent(),
            updated_at: progress.updated_at().as_millisecond(),
        }
    }
}

/// One position a player reports, as one argument rather than four.
///
/// Specta refuses a bare `i64` argument, and a struct keeps the four numbers together so a caller
/// cannot pass a duration where a position belongs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProgressReport {
    /// The title the player is showing.
    #[specta(type = Number)]
    pub title_id: i64,
    /// The episode, for a series.
    #[specta(type = Option<Number>)]
    pub episode_id: Option<i64>,
    /// The current position, in milliseconds.
    #[specta(type = Number)]
    pub position_ms: i64,
    /// The media's runtime, in milliseconds, when the player knows it.
    #[specta(type = Option<Number>)]
    pub duration_ms: Option<i64>,
}

/// The title's name and cached poster, when the library can answer them.
///
/// One read per row of a page a rail shows; batching is a follow-up if the list ever grows past a
/// screen, which is the same trade the downloads list makes.
async fn title_art(
    runtime: &peregrine_app::runtime::Runtime,
    title_id: i64,
) -> (Option<String>, Option<String>, Option<DownloadTitleKind>) {
    match runtime.library_title(title_id).await {
        Ok(Some(detail)) => {
            let kind = match detail.entry.row.title.kind {
                peregrine_app::TitleKind::Movie => DownloadTitleKind::Movie,
                peregrine_app::TitleKind::Show => DownloadTitleKind::Show,
            };
            (
                Some(detail.entry.row.title.name),
                crate::library::artwork_text(detail.entry.poster.as_deref()),
                Some(kind),
            )
        }
        _ => (None, None, None),
    }
}

/// What the account is part-way through, most recently updated first.
#[tauri::command]
#[specta::specta]
pub async fn continue_watching(
    state: State<'_, RuntimeState>,
) -> Result<Vec<ContinueWatchingDto>, AppError> {
    let runtime = state.runtime()?;
    let rows = runtime.continue_watching().await?;
    let mut watching = Vec::with_capacity(rows.len());
    for row in rows {
        let (title, poster, kind) = title_art(runtime, row.progress.title_id()).await;
        let episode = match row.progress.episode_id() {
            Some(id) => runtime.episode_numbers(id).await.unwrap_or(None),
            None => None,
        };
        watching.push(ContinueWatchingDto::of(row, title, poster, kind, episode));
    }
    Ok(guard_response("watch::continue_watching", watching))
}

/// One row of the account's viewing history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct WatchHistoryDto {
    /// The title the row is about.
    #[specta(type = Number)]
    pub title_id: i64,
    /// The title's display name, when the library can answer it.
    pub title: Option<String>,
    /// Where the cached poster is, when the cache holds one (R11 keeps it a path).
    pub poster: Option<String>,
    /// Which of the two the title is, so a row can open the title route it belongs to.
    pub title_kind: Option<DownloadTitleKind>,
    /// The episode's season, when the row names an episode.
    #[specta(type = Option<Number>)]
    pub season_number: Option<u32>,
    /// The episode's number within that season.
    #[specta(type = Option<Number>)]
    pub episode_number: Option<u32>,
    /// Whether the account finished it.
    pub watched: bool,
    /// How far through, as a fraction in `0.0..=1.0`.
    pub percent: f64,
    /// When the row was last written, in Unix milliseconds.
    #[specta(type = Number)]
    pub updated_at: i64,
}

/// The account's viewing history, newest first.
#[tauri::command]
#[specta::specta]
pub async fn viewing_history(
    state: State<'_, RuntimeState>,
    limit: Option<u32>,
) -> Result<Vec<WatchHistoryDto>, AppError> {
    let runtime = state.runtime()?;
    let rows = runtime.watch_history(limit.unwrap_or(200)).await?;
    let history = history_of(runtime, rows).await;
    Ok(guard_response("watch::viewing_history", history))
}

/// One day's watched time, for the history graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct WatchDayDto {
    /// The local calendar day, `YYYY-MM-DD`.
    pub day: String,
    /// Watched milliseconds last written that day.
    pub total_ms: f64,
}

/// Watched time per local day, optionally bounded by the row's last write.
///
/// # Errors
///
/// [`AppError`] when the runtime is not running or the read fails.
#[tauri::command]
#[specta::specta]
pub async fn watch_metrics(
    state: State<'_, RuntimeState>,
    from: Option<f64>,
    to: Option<f64>,
) -> Result<Vec<WatchDayDto>, AppError> {
    let rows = state
        .runtime()?
        .watch_metrics(from.map(|ms| ms as i64), to.map(|ms| ms as i64))
        .await?;
    let days = rows
        .into_iter()
        .map(|row| WatchDayDto {
            day: row.day,
            total_ms: row.total_ms as f64,
        })
        .collect();
    Ok(guard_response("watch::watch_metrics", days))
}
/// Removes the account's progress for one title, or every title when none is named.
///
/// # Errors
///
/// [`AppError`] when the runtime is not running or the delete fails.
#[tauri::command]
#[specta::specta]
pub async fn clear_watch_history(
    state: State<'_, RuntimeState>,
    title: Option<crate::library::TitleRef>,
) -> Result<u32, AppError> {
    let removed = state.runtime()?.clear_history(title.map(|t| t.id)).await?;
    Ok(guard_response("watch::clear_watch_history", removed))
}

/// Resolves each progress row's title, episode, and state into the history list.
async fn history_of(
    runtime: &peregrine_app::runtime::Runtime,
    rows: Vec<WatchProgressRow>,
) -> Vec<WatchHistoryDto> {
    let mut history = Vec::with_capacity(rows.len());
    for row in rows {
        let progress = row.progress;
        let (title, poster, title_kind) = title_art(runtime, progress.title_id()).await;
        let episode = match progress.episode_id() {
            Some(id) => runtime.episode_numbers(id).await.unwrap_or(None),
            None => None,
        };
        history.push(WatchHistoryDto {
            title_id: progress.title_id(),
            title,
            poster,
            title_kind,
            season_number: episode.map(|(season, _)| season),
            episode_number: episode.map(|(_, number)| number),
            watched: progress.watched(),
            percent: progress.percent(),
            updated_at: progress.updated_at().as_millisecond(),
        });
    }
    history
}

/// One saved title, as the Discover rail and the title page draw it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct WatchlistEntryDto {
    /// The title on the list.
    #[specta(type = Number)]
    pub title_id: i64,
    /// The title's display name, when the library can answer it.
    pub title: Option<String>,
    /// Where the cached poster is, when the cache holds one.
    pub poster: Option<String>,
    /// Which of the two the title is, so a card links to the right route.
    pub kind: Option<DownloadTitleKind>,
    /// When it was added, in Unix milliseconds.
    #[specta(type = Number)]
    pub added_at: i64,
}

/// The account's watchlist, newest first.
#[tauri::command]
#[specta::specta]
pub async fn watchlist(state: State<'_, RuntimeState>) -> Result<Vec<WatchlistEntryDto>, AppError> {
    let runtime = state.runtime()?;
    let rows = runtime.watchlist().await?;
    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        let (title, poster, kind) = title_art(runtime, row.entry.title_id).await;
        entries.push(WatchlistEntryDto {
            title_id: row.entry.title_id,
            title,
            poster,
            kind,
            added_at: row.entry.added_at.as_millisecond(),
        });
    }
    Ok(guard_response("watch::watchlist", entries))
}

/// Adds one title to the watchlist.
#[tauri::command]
#[specta::specta]
pub async fn add_to_watchlist(
    state: State<'_, RuntimeState>,
    title: crate::library::TitleRef,
) -> Result<(), AppError> {
    state.runtime()?.add_to_watchlist(title.id).await?;
    Ok(())
}

/// Removes one title from the watchlist.
#[tauri::command]
#[specta::specta]
pub async fn remove_from_watchlist(
    state: State<'_, RuntimeState>,
    title: crate::library::TitleRef,
) -> Result<bool, AppError> {
    Ok(state.runtime()?.remove_from_watchlist(title.id).await?)
}

/// Records where the player is in a title or episode.
#[tauri::command]
#[specta::specta]
pub async fn record_progress(
    state: State<'_, RuntimeState>,
    report: ProgressReport,
) -> Result<(), AppError> {
    state
        .runtime()?
        .record_progress(
            report.title_id,
            report.episode_id,
            report.position_ms,
            report.duration_ms,
        )
        .await?;
    let () = guard_response("watch::record_progress", ());
    Ok(())
}
