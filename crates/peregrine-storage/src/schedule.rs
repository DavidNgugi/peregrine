//! A download a person scheduled for a date that has not arrived.

use sqlx::SqlitePool;

use crate::StorageError;
use crate::library::hidden_by_scope;
use jiff::Timestamp;
use peregrine_domain::AccountScope;

/// A schedule to write, whether or not one already exists for the same title or episode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSchedule {
    /// The account that asked.
    pub account_id: i64,
    /// The title the download fills.
    pub title_id: i64,
    /// The one episode, or None for the whole title.
    pub episode_id: Option<i64>,
    /// The first date it may be fetched, as YYYY-MM-DD.
    pub release_on: String,
    /// When the request was made.
    pub created_at: Timestamp,
}

/// One stored schedule, as the scheduler and the settings list read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadScheduleRow {
    /// The row id.
    pub id: i64,
    /// The title the download fills.
    pub title_id: i64,
    /// The one episode, or None for the whole title.
    pub episode_id: Option<i64>,
    /// The first date it may be fetched, as YYYY-MM-DD.
    pub release_on: String,
    /// scheduled, started, or failed.
    pub state: String,
    /// Why the last attempt failed, when it did.
    pub error: Option<String>,
    /// Unix milliseconds.
    pub created_at: i64,
}

/// Writes one schedule, replacing the date of an existing one for the same title or episode.
pub(crate) async fn upsert(
    connection: &mut sqlx::SqliteConnection,
    schedule: &NewSchedule,
) -> Result<(), StorageError> {
    let at = schedule.created_at.as_millisecond();
    sqlx::query(
        "INSERT INTO download_schedule (account_id, title_id, episode_id, release_on, state, error, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 'scheduled', NULL, ?5, ?5) ON CONFLICT (account_id, entity_key) DO UPDATE SET release_on = excluded.release_on, state = 'scheduled', error = NULL, updated_at = excluded.updated_at",
    )
    .bind(schedule.account_id)
    .bind(schedule.title_id)
    .bind(schedule.episode_id)
    .bind(&schedule.release_on)
    .bind(at)
    .execute(connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(())
}

/// Records what the scheduler did with one row.
pub(crate) async fn set_state(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
    state: &str,
    error: Option<&str>,
    at: Timestamp,
) -> Result<(), StorageError> {
    sqlx::query(
        "UPDATE download_schedule SET state = ?2, error = ?3, updated_at = ?4 WHERE id = ?1",
    )
    .bind(id)
    .bind(state)
    .bind(error)
    .bind(at.as_millisecond())
    .execute(connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(())
}

/// Removes one schedule, which is what cancelling it means.
pub(crate) async fn delete(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
) -> Result<(), StorageError> {
    sqlx::query("DELETE FROM download_schedule WHERE id = ?1")
        .bind(id)
        .execute(connection)
        .await
        .map_err(StorageError::from_sqlx)?;
    Ok(())
}

const LIST_SQL: &str = "SELECT s.id, s.title_id, s.episode_id, s.release_on, s.state, s.error, s.created_at FROM download_schedule s JOIN title t ON t.id = s.title_id WHERE s.account_id = ?1 AND (?2 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL)) ORDER BY s.release_on ASC, s.id ASC";
const DUE_SQL: &str = "SELECT s.id, s.title_id, s.episode_id, s.release_on, s.state, s.error, s.created_at FROM download_schedule s JOIN title t ON t.id = s.title_id WHERE s.account_id = ?1 AND s.state = 'scheduled' AND s.release_on <= ?2 AND (?3 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL)) ORDER BY s.release_on ASC, s.id ASC";

fn row(value: ScheduleQueryRow) -> DownloadScheduleRow {
    DownloadScheduleRow {
        id: value.id,
        title_id: value.title_id,
        episode_id: value.episode_id,
        release_on: value.release_on,
        state: value.state,
        error: value.error,
        created_at: value.created_at,
    }
}

/// Every schedule the account holds, soonest first.
pub(crate) async fn list(
    pool: &SqlitePool,
    scope: AccountScope,
) -> Result<Vec<DownloadScheduleRow>, StorageError> {
    let rows = sqlx::query_as::<_, ScheduleQueryRow>(LIST_SQL)
        .bind(scope.account().get())
        .bind(hidden_by_scope(scope))
        .fetch_all(pool)
        .await
        .map_err(StorageError::from_sqlx)?;
    Ok(rows.into_iter().map(row).collect())
}

/// The schedules whose date has arrived, which the scheduler claims.
pub(crate) async fn due(
    pool: &SqlitePool,
    scope: AccountScope,
    today: &str,
) -> Result<Vec<DownloadScheduleRow>, StorageError> {
    let rows = sqlx::query_as::<_, ScheduleQueryRow>(DUE_SQL)
        .bind(scope.account().get())
        .bind(today)
        .bind(hidden_by_scope(scope))
        .fetch_all(pool)
        .await
        .map_err(StorageError::from_sqlx)?;
    Ok(rows.into_iter().map(row).collect())
}

#[derive(sqlx::FromRow)]
struct ScheduleQueryRow {
    id: i64,
    title_id: i64,
    episode_id: Option<i64>,
    release_on: String,
    state: String,
    error: Option<String>,
    created_at: i64,
}
