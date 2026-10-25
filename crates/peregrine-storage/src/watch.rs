//! Watch state: how far each account has watched, and what each account intends to watch.
//!
//! `DATA_MODEL` §2.6 — `watch_progress` and `watchlist_entry`. Both are per-account by
//! construction: the row's own `account_id` is the account the read serves, so the scope is
//! not only a visibility filter here, it is the key. The title each row points at still takes
//! the scope's visibility rule, because an account that may not see a title may not see its
//! progress either, and "we have no progress for this" must be the answer for a hidden title
//! exactly as it is for one nobody has watched (R34, I26).
//!
//! ## Cursors
//!
//! These are the two lists in the crate whose ordering column is not unique, and they are why
//! [`crate::page::Cursor`] carries a tie-break at all:
//!
//! - `watch_progress` is read newest-first for the Continue Watching rail, ordered by
//!   `(updated_at DESC, id DESC)`. Two rows written in the same millisecond — a batch import,
//!   or a test that injects one instant — tie on `updated_at`, and the row's id is what makes
//!   the boundary total. `watch_progress_continue_idx` serves the rail's partial predicate, and
//!   `watch_progress_account_idx` (migration 0009) serves the unrestricted list.
//! - `watchlist_entry` is read newest-first for the watchlist, ordered by
//!   `(added_at DESC, title_id DESC)`. Its primary key is `(account_id, title_id)`, so the
//!   title id is unique within the account and is the tie-break. `watchlist_added_idx` serves
//!   it.
//!
//! `WatchProgress` is identified by `(account_id, entity_key)` rather than by the row's
//! surrogate id — the domain says so, and the generated `entity_key` column plus
//! `UNIQUE (account_id, entity_key)` is the schema agreeing. The surrogate id therefore never
//! leaves this module: it is carried in [`WatchProgressRow`] only because a page boundary has
//! to name one row, and the upsert that writes a value keys on the natural pair.

use peregrine_domain::{
    AccountId, AccountScope, ProgressSource, StoredWatchProgress, WatchProgress, WatchlistEntry,
};

use crate::error::StorageError;
use crate::library::hidden_by_scope;
use crate::page::PageRequest;

/// One row of `watch_progress`, with the row id a page resumes below.
#[derive(Debug, Clone, PartialEq)]
pub struct WatchProgressRow {
    /// The surrogate row id, which is the tie-break a page resumes below.
    pub id: i64,
    /// The progress itself.
    pub progress: WatchProgress,
}

/// One episode's watch state, keyed by the numbers a season list draws.
#[derive(Debug, Clone, PartialEq)]
pub struct EpisodeProgressRow {
    /// The season the episode sits in.
    pub season_number: u32,
    /// Its number within that season.
    pub episode_number: u32,
    /// Whether the viewer finished it.
    pub watched: bool,
    /// How far through they are, as a fraction in `0.0..=1.0`.
    pub percent: f64,
    /// How far into the episode, in milliseconds, so a Resume control starts where they stopped.
    pub position_ms: i64,
}

/// One episode, named by the numbers a season list draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EpisodeRef {
    /// The season the episode sits in.
    pub season_number: u32,
    /// Its number within that season.
    pub episode_number: u32,
}

/// Where a title's watch state leaves the reader: what they last touched and what comes next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TitleWatchState {
    /// The episode with the most recently written progress row, watched or not.
    pub last_watched: Option<EpisodeRef>,
    /// The episode to offer next: the latest row when it is unfinished, else the first
    /// unwatched episode after it. `None` when every episode is watched.
    pub next_up: Option<EpisodeRef>,
}

/// One day's total watched milliseconds, for the history graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchDayRow {
    /// The local calendar day, as `YYYY-MM-DD`.
    pub day: String,
    /// Summed position across the rows last written that day.
    pub total_ms: i64,
}

/// Watched time per local day, optionally bounded by the row's last write.
pub(crate) async fn watch_by_day(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    from_ms: Option<i64>,
    to_ms: Option<i64>,
) -> Result<Vec<WatchDayRow>, StorageError> {
    let rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT date(p.updated_at / 1000, 'unixepoch', 'localtime') AS day,                 COALESCE(SUM(p.position_ms), 0) AS total_ms            FROM watch_progress p            JOIN title t ON t.id = p.title_id           WHERE p.account_id = ?1             AND (?2 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL))             AND (?3 IS NULL OR p.updated_at >= ?3)             AND (?4 IS NULL OR p.updated_at <= ?4)           GROUP BY day ORDER BY day ASC",
    )
    .bind(scope.account().get())
    .bind(hidden_by_scope(scope))
    .bind(from_ms)
    .bind(to_ms)
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(rows
        .into_iter()
        .map(|(day, total_ms)| WatchDayRow { day, total_ms })
        .collect())
}

/// One row of `watchlist_entry`.
///
/// There is no separate row id to carry: the table's identity is `(account_id, title_id)`, and
/// the title id the entry already holds is the tie-break a page resumes below.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchlistRow {
    /// The entry itself.
    pub entry: WatchlistEntry,
}

/// Writes one progress row, replacing the value for the same account and entity.
///
/// The conflict target is the natural key `(account_id, entity_key)`, which the schema's
/// `UNIQUE` constraint declares, so a player reporting a position twice updates one row rather
/// than accumulating them. `entity_key` is a generated column and is deliberately absent from
/// the column list: `SQLite` refuses to write one.
///
/// `percent` is written from the value the domain derived, not recomputed here (I6).
pub(crate) async fn upsert_progress(
    connection: &mut sqlx::SqliteConnection,
    progress: &WatchProgress,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO watch_progress (account_id, title_id, episode_id, position_ms, duration_ms, \
         percent, watched, watched_at, play_count, updated_at, source, device_label) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) \
         ON CONFLICT (account_id, entity_key) DO UPDATE SET \
             position_ms = excluded.position_ms, duration_ms = excluded.duration_ms, \
             percent = excluded.percent, watched = excluded.watched, \
             watched_at = excluded.watched_at, play_count = excluded.play_count, \
             updated_at = excluded.updated_at, source = excluded.source, \
             device_label = excluded.device_label",
    )
    .bind(progress.account_id().get())
    .bind(progress.title_id())
    .bind(progress.episode_id())
    .bind(progress.position_ms())
    .bind(progress.duration_ms())
    .bind(progress.percent())
    .bind(i64::from(progress.watched()))
    .bind(progress.watched_at().map(as_millis))
    .bind(progress.play_count())
    .bind(as_millis(progress.updated_at()))
    .bind(progress.source().as_str())
    .bind(progress.device_label())
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// Writes one watchlist entry, replacing the value for the same account and title.
///
/// The primary key is the natural key, so adding a title a second time updates the row it
/// already has rather than failing on a duplicate.
pub(crate) async fn upsert_watchlist(
    connection: &mut sqlx::SqliteConnection,
    entry: &WatchlistEntry,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO watchlist_entry (account_id, title_id, added_at, position, note, \
         auto_acquire) VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT (account_id, title_id) DO UPDATE SET \
             added_at = excluded.added_at, position = excluded.position, note = excluded.note, \
             auto_acquire = excluded.auto_acquire",
    )
    .bind(entry.account_id.get())
    .bind(entry.title_id)
    .bind(as_millis(entry.added_at))
    .bind(entry.position.map(i64::from))
    .bind(entry.note.as_deref())
    .bind(i64::from(entry.auto_acquire))
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// Removes one watchlist entry, reporting whether it was there.
pub(crate) async fn remove_watchlist(
    connection: &mut sqlx::SqliteConnection,
    account_id: AccountId,
    title_id: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM watchlist_entry WHERE account_id = ?1 AND title_id = ?2")
        .bind(account_id.get())
        .bind(title_id)
        .execute(&mut *connection)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Removes one account's progress, for one title or for everything, and reports how many rows went.
///
/// The title case removes every episode of that title, which is what "clear this title from my
/// history" means to a reader looking at one row: the row is the title's latest episode, not a
/// separate history entry per episode.
pub(crate) async fn clear_progress(
    connection: &mut sqlx::SqliteConnection,
    account_id: AccountId,
    title_id: Option<i64>,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM watch_progress WHERE account_id = ?1 AND (?2 IS NULL OR title_id = ?2)",
    )
    .bind(account_id.get())
    .bind(title_id)
    .execute(&mut *connection)
    .await?;
    Ok(result.rows_affected())
}

/// Reads one account's progress for one title or episode, when the scope may see the title.
pub(crate) async fn progress_for(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    title_id: i64,
    episode_id: Option<i64>,
) -> Result<Option<WatchProgress>, StorageError> {
    let row = sqlx::query_as::<_, ProgressRow>(
        "SELECT p.id, p.account_id, p.title_id, p.episode_id, p.position_ms, p.duration_ms, \
         p.percent, p.watched, p.watched_at, p.play_count, p.updated_at, p.source, \
         p.device_label FROM watch_progress p \
         JOIN title t ON t.id = p.title_id \
         WHERE p.account_id = ?1 AND p.title_id = ?2 AND p.episode_id IS ?3 \
           AND (?4 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL))",
    )
    .bind(scope.account().get())
    .bind(title_id)
    .bind(episode_id)
    .bind(hidden_by_scope(scope))
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    row.map(WatchProgressRow::try_from)
        .transpose()
        .map(|row| row.map(|row| row.progress))
}

/// Every episode of one title the account has progress for, in season and episode order.
///
/// A single title's rows rather than a page: a season list draws all of its episodes at once, and a
/// cursor would make the screen responsible for stitching pages of a set it already bounds by its
/// own limit.
pub(crate) async fn list_progress_for_title(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    title_id: i64,
) -> Result<Vec<EpisodeProgressRow>, StorageError> {
    let rows = sqlx::query_as::<_, EpisodeProgressQueryRow>(
        "SELECT e.season_number, e.episode_number, p.watched, p.percent, p.position_ms \
           FROM watch_progress p \
           JOIN episode e ON e.id = p.episode_id \
           JOIN title t ON t.id = p.title_id \
          WHERE p.account_id = ?1 AND p.title_id = ?2 AND p.episode_id IS NOT NULL \
            AND (?3 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL)) \
          ORDER BY e.season_number ASC, e.episode_number ASC",
    )
    .bind(scope.account().get())
    .bind(title_id)
    .bind(hidden_by_scope(scope))
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    rows.into_iter()
        .map(|row| {
            Ok(EpisodeProgressRow {
                season_number: u32::try_from(row.season_number).map_err(|_| {
                    StorageError::UnexpectedValue {
                        column: "season_number",
                        found: row.season_number.to_string(),
                    }
                })?,
                episode_number: u32::try_from(row.episode_number).map_err(|_| {
                    StorageError::UnexpectedValue {
                        column: "episode_number",
                        found: row.episode_number.to_string(),
                    }
                })?,
                watched: row.watched != 0,
                percent: row.percent,
                position_ms: row.position_ms,
            })
        })
        .collect()
}

/// One episode's season and number, when the id still names a row.
pub(crate) async fn episode_numbers(
    pool: &sqlx::SqlitePool,
    episode_id: i64,
) -> Result<Option<(u32, u32)>, StorageError> {
    let row = sqlx::query_as::<_, (i64, i64)>(
        "SELECT season_number, episode_number FROM episode WHERE id = ?1",
    )
    .bind(episode_id)
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    row.map(|(season, episode)| {
        Ok((
            u32::try_from(season).map_err(|_| StorageError::UnexpectedValue {
                column: "season_number",
                found: season.to_string(),
            })?,
            u32::try_from(episode).map_err(|_| StorageError::UnexpectedValue {
                column: "episode_number",
                found: episode.to_string(),
            })?,
        ))
    })
    .transpose()
}

/// The columns [`EpisodeProgressRow`] is built from.
#[derive(sqlx::FromRow)]
struct EpisodeProgressQueryRow {
    season_number: i64,
    episode_number: i64,
    watched: i64,
    percent: f64,
    position_ms: i64,
}

/// Reads a page of one account's progress, most recently updated first.
///
/// The rail's own query: `watched = 0 AND percent BETWEEN 0.02 AND 0.95` is the partial
/// predicate `watch_progress_continue_idx` covers, and the tuple cursor over
/// `(updated_at, id)` is what keeps two rows written in the same millisecond from making the
/// boundary skip one of them.
pub(crate) async fn list_continue(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    request: PageRequest,
) -> Result<Vec<WatchProgressRow>, StorageError> {
    let rows = sqlx::query_as::<_, ProgressRow>(
        "SELECT p.id, p.account_id, p.title_id, p.episode_id, p.position_ms, p.duration_ms, \
         p.percent, p.watched, p.watched_at, p.play_count, p.updated_at, p.source, \
         p.device_label FROM watch_progress p \
         JOIN title t ON t.id = p.title_id \
         WHERE p.account_id = ?1 AND p.watched = 0 AND p.percent BETWEEN 0.02 AND 0.95 \
           AND (?2 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL)) \
           AND (?3 IS NULL OR (p.updated_at, p.id) < (?3, ?4)) \
         ORDER BY p.updated_at DESC, p.id DESC LIMIT ?5",
    )
    .bind(scope.account().get())
    .bind(hidden_by_scope(scope))
    .bind(request.bindable_cursor())
    .bind(request.bindable_tie_break())
    .bind(request.bindable_limit())
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    rows.into_iter().map(TryInto::try_into).collect()
}

/// Where one title's watch state leaves the reader, across every season.
///
/// This is the one read that can choose the season a title page opens on: the latest progress
/// row names the episode the reader last touched, and the next unwatched episode after it is the
/// one a Resume or Download control should offer. Files are irrelevant here; deleting a payload
/// does not delete the fact that the episode was watched.
pub(crate) async fn title_watch_state(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    title_id: i64,
) -> Result<TitleWatchState, StorageError> {
    let rows = sqlx::query_as::<_, (i64, i64, i64, Option<i64>)>(
        "SELECT e.season_number, e.episode_number, COALESCE(p.watched, 0), p.updated_at \
         FROM episode e JOIN title t ON t.id = e.title_id \
         LEFT JOIN watch_progress p ON p.episode_id = e.id AND p.account_id = ?2 \
         WHERE e.title_id = ?1 AND (?3 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL)) \
         ORDER BY e.season_number ASC, e.episode_number ASC",
    )
    .bind(title_id)
    .bind(scope.account().get())
    .bind(hidden_by_scope(scope))
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;

    let mut episodes = Vec::with_capacity(rows.len());
    for (season, episode, watched, updated_at) in rows {
        let season_number = u32::try_from(season).map_err(|_| StorageError::UnexpectedValue {
            column: "season_number",
            found: season.to_string(),
        })?;
        let episode_number = u32::try_from(episode).map_err(|_| StorageError::UnexpectedValue {
            column: "episode_number",
            found: episode.to_string(),
        })?;
        episodes.push((
            EpisodeRef {
                season_number,
                episode_number,
            },
            watched != 0,
            updated_at,
        ));
    }

    let Some((latest, _)) = episodes
        .iter()
        .enumerate()
        .filter_map(|(index, (_, _, updated_at))| updated_at.map(|at| (index, at)))
        .max_by_key(|(_, at)| *at)
    else {
        return Ok(TitleWatchState::default());
    };

    let last_watched = Some(episodes[latest].0);
    let next_up = if episodes[latest].1 {
        episodes[latest + 1..]
            .iter()
            .find(|(_, watched, _)| !*watched)
            .map(|(episode, _, _)| *episode)
    } else {
        last_watched
    };
    Ok(TitleWatchState {
        last_watched,
        next_up,
    })
}

/// Reads a page of everything one account has watched, most recently updated first.
///
/// `watch_progress_account_idx` is the index this serves. A restricted scope's rows for
/// hidden titles are filtered in the same statement, so they neither appear nor consume the
/// page.
pub(crate) async fn list_progress(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    request: PageRequest,
) -> Result<Vec<WatchProgressRow>, StorageError> {
    let rows = sqlx::query_as::<_, ProgressRow>(
        "SELECT p.id, p.account_id, p.title_id, p.episode_id, p.position_ms, p.duration_ms, \
         p.percent, p.watched, p.watched_at, p.play_count, p.updated_at, p.source, \
         p.device_label FROM watch_progress p \
         JOIN title t ON t.id = p.title_id \
         WHERE p.account_id = ?1 \
           AND (?2 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL)) \
           AND (?3 IS NULL OR (p.updated_at, p.id) < (?3, ?4)) \
         ORDER BY p.updated_at DESC, p.id DESC LIMIT ?5",
    )
    .bind(scope.account().get())
    .bind(hidden_by_scope(scope))
    .bind(request.bindable_cursor())
    .bind(request.bindable_tie_break())
    .bind(request.bindable_limit())
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    rows.into_iter().map(TryInto::try_into).collect()
}

/// Reads one watchlist entry, when the scope may see the title.
pub(crate) async fn watchlist_entry(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    title_id: i64,
) -> Result<Option<WatchlistEntry>, StorageError> {
    let row = sqlx::query_as::<_, WatchlistRowInner>(
        "SELECT w.account_id, w.title_id, w.added_at, w.position, w.note, w.auto_acquire \
         FROM watchlist_entry w JOIN title t ON t.id = w.title_id \
         WHERE w.account_id = ?1 AND w.title_id = ?2 \
           AND (?3 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL))",
    )
    .bind(scope.account().get())
    .bind(title_id)
    .bind(hidden_by_scope(scope))
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    row.map(WatchlistRow::try_from)
        .transpose()
        .map(|row| row.map(|row| row.entry))
}

/// Reads a page of one account's watchlist, newest first.
///
/// `watchlist_added_idx` is the index this serves. The cursor is `(added_at, title_id)`: the
/// title id is unique within the account by the table's primary key, so the pair is total.
pub(crate) async fn list_watchlist(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    request: PageRequest,
) -> Result<Vec<WatchlistRow>, StorageError> {
    let rows = sqlx::query_as::<_, WatchlistRowInner>(
        "SELECT w.account_id, w.title_id, w.added_at, w.position, w.note, w.auto_acquire \
         FROM watchlist_entry w JOIN title t ON t.id = w.title_id \
         WHERE w.account_id = ?1 \
           AND (?2 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL)) \
           AND (?3 IS NULL OR (w.added_at, w.title_id) < (?3, ?4)) \
         ORDER BY w.added_at DESC, w.title_id DESC LIMIT ?5",
    )
    .bind(scope.account().get())
    .bind(hidden_by_scope(scope))
    .bind(request.bindable_cursor())
    .bind(request.bindable_tie_break())
    .bind(request.bindable_limit())
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    rows.into_iter().map(TryInto::try_into).collect()
}

/// A `TIMESTAMP` column as the `i64` milliseconds it is stored as.
fn as_millis(value: jiff::Timestamp) -> i64 {
    value.as_millisecond()
}

/// The inverse of [`as_millis`], for a row this build is reading.
fn millis(value: i64) -> jiff::Timestamp {
    jiff::Timestamp::from_millisecond(value).unwrap_or(jiff::Timestamp::UNIX_EPOCH)
}

#[derive(sqlx::FromRow)]
struct ProgressRow {
    id: i64,
    account_id: i64,
    title_id: i64,
    episode_id: Option<i64>,
    position_ms: i64,
    duration_ms: Option<i64>,
    percent: f64,
    watched: i64,
    watched_at: Option<i64>,
    play_count: i64,
    updated_at: i64,
    source: String,
    device_label: Option<String>,
}

impl TryFrom<ProgressRow> for WatchProgressRow {
    type Error = StorageError;

    fn try_from(row: ProgressRow) -> Result<Self, Self::Error> {
        let account_id =
            AccountId::new(row.account_id).map_err(|_| StorageError::UnexpectedValue {
                column: "watch_progress.account_id",
                found: row.account_id.to_string(),
            })?;
        let source =
            ProgressSource::parse(&row.source).ok_or_else(|| StorageError::UnexpectedValue {
                column: "watch_progress.source",
                found: row.source.clone(),
            })?;
        // The row's shape is the domain's; the value it becomes is validated by
        // `WatchProgress::restore`, which re-derives the percentage and refuses a watched
        // flag the derivation cannot support (I6). The stored `percent` column is carried
        // because the shape has it, and ignored because the derivation is the source of
        // truth.
        let progress = WatchProgress::restore(StoredWatchProgress {
            account_id,
            title_id: row.title_id,
            episode_id: row.episode_id,
            position_ms: row.position_ms,
            duration_ms: row.duration_ms,
            percent: row.percent,
            watched: row.watched != 0,
            watched_at: row.watched_at.map(millis),
            play_count: u32::try_from(row.play_count).map_err(|_| {
                StorageError::UnexpectedValue {
                    column: "watch_progress.play_count",
                    found: row.play_count.to_string(),
                }
            })?,
            updated_at: millis(row.updated_at),
            source,
            device_label: row.device_label,
        })
        .map_err(|error| StorageError::UnexpectedValue {
            column: "watch_progress",
            found: error.to_string(),
        })?;
        Ok(Self {
            id: row.id,
            progress,
        })
    }
}

#[derive(sqlx::FromRow)]
struct WatchlistRowInner {
    account_id: i64,
    title_id: i64,
    added_at: i64,
    position: Option<i64>,
    note: Option<String>,
    auto_acquire: i64,
}

impl TryFrom<WatchlistRowInner> for WatchlistRow {
    type Error = StorageError;

    fn try_from(row: WatchlistRowInner) -> Result<Self, Self::Error> {
        let account_id =
            AccountId::new(row.account_id).map_err(|_| StorageError::UnexpectedValue {
                column: "watchlist_entry.account_id",
                found: row.account_id.to_string(),
            })?;
        let position = row
            .position
            .map(|value| {
                u32::try_from(value).map_err(|_| StorageError::UnexpectedValue {
                    column: "watchlist_entry.position",
                    found: value.to_string(),
                })
            })
            .transpose()?;
        Ok(Self {
            entry: WatchlistEntry {
                account_id,
                title_id: row.title_id,
                added_at: millis(row.added_at),
                position,
                note: row.note,
                auto_acquire: row.auto_acquire != 0,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Database;
    use crate::database::open_writer_connection;
    use crate::page::Cursor;
    use crate::test_support::{NOW, account, database, path_of, title};
    use jiff::Timestamp;
    use peregrine_domain::{PlaybackPosition, ProgressSource, WatchOrigin};
    use proptest::prelude::*;
    use sqlx::Connection as _;
    use tempfile::TempDir;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millisecond(millis).expect("a millisecond inside the range")
    }

    fn any_account_scope() -> AccountScope {
        AccountScope::unrestricted(AccountId::new(1).expect("account one exists"))
    }

    fn restricted() -> AccountScope {
        AccountScope::restricted(AccountId::new(1).expect("account one exists"))
    }

    fn account_id() -> AccountId {
        AccountId::new(1).expect("account one exists")
    }

    /// One account and three titles: two certified, one adult, one unrated.
    async fn populated(directory: &TempDir) -> (Database, i64, i64, i64) {
        let path = path_of(directory);
        let database = database(directory).await;
        account(&path, 1, "Adult").await;
        title(&path, 1, "Alien", 0, Some("R")).await;
        title(&path, 2, "Adult Film", 1, Some("X")).await;
        title(&path, 3, "Unrated", 0, None).await;
        (database, 1, 2, 3)
    }

    fn progress(
        title_id: i64,
        episode_id: Option<i64>,
        position_ms: i64,
        duration_ms: Option<i64>,
        updated_at: i64,
    ) -> WatchProgress {
        WatchProgress::record(
            account_id(),
            title_id,
            episode_id,
            PlaybackPosition::new(position_ms, duration_ms).expect("a valid position"),
            ProgressSource::Local,
            at(updated_at),
            None,
        )
    }

    fn entry(title_id: i64, added_at: i64) -> WatchlistEntry {
        WatchlistEntry::new(account_id(), title_id, at(added_at))
    }

    // ---- round trips -------------------------------------------------------------

    #[tokio::test]
    async fn watch_progress_round_trips() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _, _) = populated(&directory).await;

        let written = progress(certified, None, 300_000, Some(600_000), NOW);
        database
            .record_watch_progress(written.clone())
            .await
            .unwrap();

        let stored = database
            .watch_progress(any_account_scope(), certified, None)
            .await
            .unwrap()
            .expect("the row");
        assert_eq!(stored, written);
        assert_eq!(stored.percent(), 0.5, "the derivation is the domain's");
        assert!(!stored.watched());
        assert_eq!(stored.account_id(), account_id());
        assert_eq!(stored.entity_key(), format!("t{certified}"));

        assert_eq!(
            database
                .watch_progress(any_account_scope(), certified + 99, None)
                .await
                .unwrap(),
            None
        );
    }

    /// The natural key: a second report for the same account and entity updates the row
    /// rather than adding one, which is what makes a coalesced player write idempotent.
    #[tokio::test]
    async fn a_second_report_updates_the_one_row() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _, _) = populated(&directory).await;

        database
            .record_watch_progress(progress(certified, None, 60_000, Some(600_000), NOW))
            .await
            .unwrap();
        database
            .record_watch_progress(progress(certified, None, 300_000, Some(600_000), NOW + 1))
            .await
            .unwrap();

        let page = database
            .list_watch_progress(any_account_scope(), PageRequest::first(10))
            .await
            .unwrap();
        assert_eq!(page.len(), 1, "one account, one title, one row");
        assert_eq!(page[0].progress.position_ms(), 300_000);
        assert_eq!(page[0].progress.updated_at(), at(NOW + 1));
    }

    /// An episode's progress is its own row: the generated `entity_key` distinguishes it from
    /// the title's.
    #[tokio::test]
    async fn an_episodes_progress_is_a_separate_row() {
        let directory = TempDir::new().unwrap();
        let path = path_of(&directory);
        let (database, certified, _, _) = populated(&directory).await;
        crate::test_support::season(&path, 1, certified, 1).await;
        crate::test_support::episode(&path, 1, certified, 1, 1, 1).await;

        database
            .record_watch_progress(progress(certified, None, 10, Some(100), NOW))
            .await
            .unwrap();
        database
            .record_watch_progress(progress(certified, Some(1), 20, Some(100), NOW + 1))
            .await
            .unwrap();

        assert_eq!(
            database
                .watch_progress(any_account_scope(), certified, None)
                .await
                .unwrap()
                .unwrap()
                .position_ms(),
            10
        );
        assert_eq!(
            database
                .watch_progress(any_account_scope(), certified, Some(1))
                .await
                .unwrap()
                .unwrap()
                .position_ms(),
            20
        );
        assert_eq!(
            database
                .list_watch_progress(any_account_scope(), PageRequest::first(10))
                .await
                .unwrap()
                .len(),
            2
        );
    }

    /// I6's storage floor, read back: a policy mark inside the floor is written, and one
    /// below it is refused by the domain before it can reach the database.
    #[tokio::test]
    async fn a_watched_mark_below_the_storage_floor_is_refused_by_the_domain() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _, _) = populated(&directory).await;

        let mut low = progress(certified, None, 60_000, Some(600_000), NOW);
        assert!(
            low.mark_watched(WatchOrigin::Policy, at(NOW)).is_err(),
            "10% is below the storage floor"
        );

        let mut high = progress(certified, None, 570_000, Some(600_000), NOW);
        high.mark_watched(WatchOrigin::Policy, at(NOW)).unwrap();
        database.record_watch_progress(high).await.unwrap();
        let stored = database
            .watch_progress(any_account_scope(), certified, None)
            .await
            .unwrap()
            .unwrap();
        assert!(stored.watched());
        assert_eq!(stored.watched_at(), Some(at(NOW)));

        // A manual mark below the floor is the user's statement of intent and survives.
        let mut manual = progress(certified, None, 60_000, Some(600_000), NOW);
        manual
            .mark_watched(WatchOrigin::Manual, at(NOW))
            .expect("a manual mark is exempt from the floor");
        database.record_watch_progress(manual).await.unwrap();
        assert!(
            database
                .watch_progress(any_account_scope(), certified, None)
                .await
                .unwrap()
                .unwrap()
                .watched()
        );
    }

    #[tokio::test]
    async fn title_watch_state_follows_the_latest_episode_across_seasons() {
        let directory = TempDir::new().unwrap();
        let path = path_of(&directory);
        let (database, title_id, _, _) = populated(&directory).await;
        crate::test_support::season(&path, 1, title_id, 1).await;
        crate::test_support::season(&path, 2, title_id, 2).await;
        for (episode_id, season, number) in [(1, 1, 1), (2, 1, 2), (3, 2, 1), (4, 2, 2)] {
            crate::test_support::episode(&path, episode_id, title_id, season, season, number).await;
        }

        assert_eq!(
            database
                .title_watch_state(any_account_scope(), title_id)
                .await
                .unwrap(),
            TitleWatchState::default()
        );

        let mut first = progress(title_id, Some(1), 0, Some(600_000), NOW);
        first
            .mark_watched(WatchOrigin::Manual, at(NOW))
            .expect("a manual mark is exempt from the floor");
        database.record_watch_progress(first).await.unwrap();
        assert_eq!(
            database
                .title_watch_state(any_account_scope(), title_id)
                .await
                .unwrap(),
            TitleWatchState {
                last_watched: Some(EpisodeRef {
                    season_number: 1,
                    episode_number: 1,
                }),
                next_up: Some(EpisodeRef {
                    season_number: 1,
                    episode_number: 2,
                }),
            },
            "the first unwatched episode after the latest mark"
        );

        let mut later = progress(title_id, Some(3), 0, Some(600_000), NOW + 10);
        later
            .mark_watched(WatchOrigin::Manual, at(NOW + 10))
            .expect("a manual mark is exempt from the floor");
        database.record_watch_progress(later).await.unwrap();
        assert_eq!(
            database
                .title_watch_state(any_account_scope(), title_id)
                .await
                .unwrap(),
            TitleWatchState {
                last_watched: Some(EpisodeRef {
                    season_number: 2,
                    episode_number: 1,
                }),
                next_up: Some(EpisodeRef {
                    season_number: 2,
                    episode_number: 2,
                }),
            },
            "a later season moves the next-up point with it"
        );

        database
            .record_watch_progress(progress(title_id, Some(4), 30_000, Some(600_000), NOW + 20))
            .await
            .unwrap();
        assert_eq!(
            database
                .title_watch_state(any_account_scope(), title_id)
                .await
                .unwrap(),
            TitleWatchState {
                last_watched: Some(EpisodeRef {
                    season_number: 2,
                    episode_number: 2,
                }),
                next_up: Some(EpisodeRef {
                    season_number: 2,
                    episode_number: 2,
                }),
            },
            "an unfinished latest episode is the resume point"
        );

        let mut finished = progress(title_id, Some(4), 600_000, Some(600_000), NOW + 30);
        finished
            .mark_watched(WatchOrigin::Manual, at(NOW + 30))
            .expect("a manual mark is exempt from the floor");
        database.record_watch_progress(finished).await.unwrap();
        let state = database
            .title_watch_state(any_account_scope(), title_id)
            .await
            .unwrap();
        assert_eq!(
            state.last_watched,
            Some(EpisodeRef {
                season_number: 2,
                episode_number: 2,
            })
        );
        assert_eq!(state.next_up, None, "nothing is left to offer");
    }

    #[tokio::test]
    async fn a_watchlist_entry_round_trips() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _, _) = populated(&directory).await;

        let written = WatchlistEntry {
            position: Some(3),
            note: Some("watch after Alien".to_owned()),
            auto_acquire: true,
            ..entry(certified, NOW)
        };
        database.add_to_watchlist(written.clone()).await.unwrap();

        assert_eq!(
            database
                .watchlist_entry(any_account_scope(), certified)
                .await
                .unwrap(),
            Some(written.clone())
        );
        assert_eq!(
            database
                .watchlist_entry(any_account_scope(), certified + 99)
                .await
                .unwrap(),
            None
        );

        // The primary key is the natural key: adding it again updates the one row.
        let updated = WatchlistEntry {
            position: None,
            note: None,
            auto_acquire: false,
            ..entry(certified, NOW + 5)
        };
        database.add_to_watchlist(updated.clone()).await.unwrap();
        assert_eq!(
            database
                .list_watchlist(any_account_scope(), PageRequest::first(10))
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            database
                .watchlist_entry(any_account_scope(), certified)
                .await
                .unwrap(),
            Some(updated)
        );

        assert!(
            database
                .remove_from_watchlist(account_id(), certified)
                .await
                .unwrap()
        );
        assert!(
            !database
                .remove_from_watchlist(account_id(), certified)
                .await
                .unwrap(),
            "removing it twice reports the second as a no-op"
        );
        assert_eq!(
            database
                .watchlist_entry(any_account_scope(), certified)
                .await
                .unwrap(),
            None
        );
    }

    // ---- scope (R34, I25, I26) ---------------------------------------------------

    /// Progress for a title the scope may not see is indistinguishable from progress that does
    /// not exist, by id, by title, and in the list.
    #[tokio::test]
    async fn a_restricted_scope_cannot_see_progress_for_hidden_titles() {
        let directory = TempDir::new().unwrap();
        let (database, certified, adult, unrated) = populated(&directory).await;

        database
            .record_watch_progress(progress(certified, None, 300_000, Some(600_000), NOW))
            .await
            .unwrap();
        database
            .record_watch_progress(progress(adult, None, 300_000, Some(600_000), NOW + 1))
            .await
            .unwrap();
        database
            .record_watch_progress(progress(unrated, None, 300_000, Some(600_000), NOW + 2))
            .await
            .unwrap();

        let scope = restricted();
        assert!(
            database
                .watch_progress(scope, certified, None)
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(
            database.watch_progress(scope, adult, None).await.unwrap(),
            None,
            "an adult title's progress is not readable"
        );
        assert_eq!(
            database.watch_progress(scope, unrated, None).await.unwrap(),
            None,
            "I27: an uncertified title fails closed"
        );
        assert_eq!(
            database
                .watch_progress(scope, i64::MAX, None)
                .await
                .unwrap(),
            None,
            "and an absent title answers the same"
        );

        let visible = database
            .list_watch_progress(scope, PageRequest::first(10))
            .await
            .unwrap();
        assert_eq!(visible.len(), 1, "hidden rows do not consume the page");
        assert_eq!(visible[0].progress.title_id(), certified);

        assert_eq!(
            database
                .list_watch_progress(any_account_scope(), PageRequest::first(10))
                .await
                .unwrap()
                .len(),
            3,
            "the rows exist; the scope is what hides them"
        );
    }

    /// The same for the rail, which is on the first screen and must not be a place a hidden
    /// title leaks.
    #[tokio::test]
    async fn a_restricted_scope_cannot_see_continue_watching_for_hidden_titles() {
        let directory = TempDir::new().unwrap();
        let (database, certified, adult, _) = populated(&directory).await;

        database
            .record_watch_progress(progress(certified, None, 300_000, Some(600_000), NOW))
            .await
            .unwrap();
        database
            .record_watch_progress(progress(adult, None, 300_000, Some(600_000), NOW + 1))
            .await
            .unwrap();

        let scope = restricted();
        let rail = database
            .list_continue_watching(scope, PageRequest::first(10))
            .await
            .unwrap();
        assert_eq!(rail.len(), 1);
        assert_eq!(rail[0].progress.title_id(), certified);
        assert_eq!(
            database
                .list_continue_watching(any_account_scope(), PageRequest::first(10))
                .await
                .unwrap()
                .len(),
            2
        );
    }

    /// A watched row is not on the rail, and a barely-started one is not either.
    #[tokio::test]
    async fn the_rail_holds_only_what_is_part_way_through() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _, _) = populated(&directory).await;
        crate::test_support::title(&path_of(&directory), 4, "Aliens", 0, Some("R")).await;

        // 1%: below the rail's lower bound.
        database
            .record_watch_progress(progress(certified, None, 6_000, Some(600_000), NOW))
            .await
            .unwrap();
        // 50%: on the rail.
        database
            .record_watch_progress(progress(4, None, 300_000, Some(600_000), NOW + 1))
            .await
            .unwrap();

        let rail = database
            .list_continue_watching(any_account_scope(), PageRequest::first(10))
            .await
            .unwrap();
        assert_eq!(rail.len(), 1);
        assert_eq!(rail[0].progress.title_id(), 4);

        // Watched: off the rail.
        let mut watched = progress(certified, None, 300_000, Some(600_000), NOW + 2);
        watched
            .mark_watched(WatchOrigin::Manual, at(NOW + 2))
            .unwrap();
        database.record_watch_progress(watched).await.unwrap();
        let rail = database
            .list_continue_watching(any_account_scope(), PageRequest::first(10))
            .await
            .unwrap();
        assert_eq!(rail.len(), 1);
        assert_eq!(rail[0].progress.title_id(), 4);
    }

    /// The watchlist is scoped the same way, and removing an entry for a title the scope may
    /// not see is refused the same way reading it is.
    #[tokio::test]
    async fn a_restricted_scope_cannot_see_a_hidden_watchlist_entry() {
        let directory = TempDir::new().unwrap();
        let (database, certified, adult, _) = populated(&directory).await;

        database
            .add_to_watchlist(entry(certified, NOW))
            .await
            .unwrap();
        database
            .add_to_watchlist(entry(adult, NOW + 1))
            .await
            .unwrap();

        let scope = restricted();
        assert_eq!(
            database.watchlist_entry(scope, adult).await.unwrap(),
            None,
            "an adult entry reads as nothing"
        );
        assert!(
            database
                .watchlist_entry(scope, certified)
                .await
                .unwrap()
                .is_some()
        );
        let visible = database
            .list_watchlist(scope, PageRequest::first(10))
            .await
            .unwrap();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].entry.title_id, certified);
    }

    /// A different account's progress is not this account's, whatever the scope's visibility.
    #[tokio::test]
    async fn another_accounts_progress_is_not_returned() {
        let directory = TempDir::new().unwrap();
        let path = path_of(&directory);
        let (database, certified, _, _) = populated(&directory).await;
        account(&path, 2, "Child").await;

        let other = WatchProgress::record(
            AccountId::new(2).unwrap(),
            certified,
            None,
            PlaybackPosition::new(300_000, Some(600_000)).unwrap(),
            ProgressSource::Local,
            at(NOW),
            None,
        );
        database.record_watch_progress(other).await.unwrap();

        assert_eq!(
            database
                .watch_progress(any_account_scope(), certified, None)
                .await
                .unwrap(),
            None,
            "the row is account two's, and this read is account one's"
        );
        assert!(
            database
                .watch_progress(
                    AccountScope::unrestricted(AccountId::new(2).unwrap()),
                    certified,
                    None
                )
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(
            database
                .list_watchlist(any_account_scope(), PageRequest::first(10))
                .await
                .unwrap(),
            Vec::new(),
        );
    }

    // ---- pagination --------------------------------------------------------------

    /// The tuple cursor is the point of this list: rows written in the same millisecond tie on
    /// `updated_at`, and a page boundary over a single-valued cursor would skip one of them.
    #[tokio::test]
    async fn rows_written_in_one_millisecond_are_not_skipped_between_pages() {
        let directory = TempDir::new().unwrap();
        let path = path_of(&directory);
        let (database, _, _, _) = populated(&directory).await;
        for id in 4..=9 {
            title(&path, id, &format!("Title {id}"), 0, Some("R")).await;
        }

        // One instant for every row: the tie the tie-break exists for.
        for id in 1..=9 {
            database
                .record_watch_progress(progress(id, None, 300_000, Some(600_000), NOW))
                .await
                .unwrap();
        }

        let mut seen = Vec::new();
        let mut request = PageRequest::first(2);
        loop {
            let page = database
                .list_watch_progress(any_account_scope(), request)
                .await
                .unwrap();
            let Some(last) = page.last() else { break };
            seen.extend(page.iter().map(|row| row.id));
            request = PageRequest::after(
                Cursor::at(last.progress.updated_at().as_millisecond(), last.id),
                2,
            );
        }

        assert_eq!(seen.len(), 9, "every row is reached exactly once");
        let mut sorted = seen.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 9, "and none is repeated: {seen:?}");
    }

    #[tokio::test]
    async fn a_row_written_between_pages_neither_appears_nor_pushes_one_out() {
        let directory = TempDir::new().unwrap();
        let (database, _, _, _) = populated(&directory).await;
        crate::test_support::title(&path_of(&directory), 4, "Aliens", 0, Some("R")).await;
        crate::test_support::title(&path_of(&directory), 5, "Alien 3", 0, Some("R")).await;

        for (title_id, at) in [(1, NOW), (4, NOW + 1), (5, NOW + 2)] {
            database
                .record_watch_progress(progress(title_id, None, 300_000, Some(600_000), at))
                .await
                .unwrap();
        }

        let first = database
            .list_watch_progress(any_account_scope(), PageRequest::first(1))
            .await
            .unwrap();
        assert_eq!(first[0].progress.title_id(), 5);
        let cursor = Cursor::at(first[0].progress.updated_at().as_millisecond(), first[0].id);

        // A row that sorts above the cursor and one below it, both written after the first page.
        database
            .record_watch_progress(progress(2, None, 300_000, Some(600_000), NOW + 9))
            .await
            .unwrap();
        database
            .record_watch_progress(progress(3, None, 300_000, Some(600_000), NOW - 9))
            .await
            .unwrap();

        let second = database
            .list_watch_progress(any_account_scope(), PageRequest::after(cursor, 1))
            .await
            .unwrap();
        assert_eq!(
            second[0].progress.title_id(),
            4,
            "the page continues below the cursor, not from the top"
        );
        let second_cursor = Cursor::at(
            second[0].progress.updated_at().as_millisecond(),
            second[0].id,
        );
        let third = database
            .list_watch_progress(any_account_scope(), PageRequest::after(second_cursor, 5))
            .await
            .unwrap();
        let ids: Vec<i64> = third.iter().map(|row| row.progress.title_id()).collect();
        assert_eq!(
            ids,
            vec![1, 3],
            "the third page reads what is below the cursor — the row written below it is \
             there, the row written above it is not, and the one already read is not repeated"
        );
    }

    /// The watchlist's cursor is `(added_at, title_id)`, which the primary key makes total.
    #[tokio::test]
    async fn watchlist_entries_with_one_instant_are_not_skipped() {
        let directory = TempDir::new().unwrap();
        let path = path_of(&directory);
        let (database, _, _, _) = populated(&directory).await;
        for id in 4..=8 {
            title(&path, id, &format!("Title {id}"), 0, Some("R")).await;
        }
        for id in 1..=8 {
            database.add_to_watchlist(entry(id, NOW)).await.unwrap();
        }

        let mut seen = Vec::new();
        let mut request = PageRequest::first(3);
        loop {
            let page = database
                .list_watchlist(any_account_scope(), request)
                .await
                .unwrap();
            let Some(last) = page.last() else { break };
            seen.extend(page.iter().map(|row| row.entry.title_id));
            request = PageRequest::after(
                Cursor::at(last.entry.added_at.as_millisecond(), last.entry.title_id),
                3,
            );
        }
        seen.sort_unstable();
        assert_eq!(seen, (1..=8).collect::<Vec<i64>>());
    }

    #[tokio::test]
    async fn a_watchlist_addition_between_pages_neither_appears_nor_pushes_one_out() {
        let directory = TempDir::new().unwrap();
        let path = path_of(&directory);
        let (database, _, _, _) = populated(&directory).await;
        for id in 4..=6 {
            title(&path, id, &format!("Title {id}"), 0, Some("R")).await;
        }
        for (title_id, at) in [(1, NOW), (4, NOW + 1), (5, NOW + 2)] {
            database
                .add_to_watchlist(entry(title_id, at))
                .await
                .unwrap();
        }

        let first = database
            .list_watchlist(any_account_scope(), PageRequest::first(1))
            .await
            .unwrap();
        assert_eq!(first[0].entry.title_id, 5);
        let cursor = Cursor::at(first[0].entry.added_at.as_millisecond(), 5);

        database.add_to_watchlist(entry(3, NOW + 9)).await.unwrap();
        database.add_to_watchlist(entry(6, NOW - 9)).await.unwrap();

        let second = database
            .list_watchlist(any_account_scope(), PageRequest::after(cursor, 3))
            .await
            .unwrap();
        assert_eq!(
            second
                .iter()
                .map(|row| row.entry.title_id)
                .collect::<Vec<_>>(),
            vec![4, 1, 6],
            "the page below the cursor is unchanged by the two additions"
        );
    }

    /// The one-row page walk: the cursor is total, so it reaches every row once.
    #[tokio::test]
    async fn a_one_row_page_walks_every_progress_row_exactly_once() {
        let directory = TempDir::new().unwrap();
        let path = path_of(&directory);
        let (database, _, _, _) = populated(&directory).await;
        for id in 4..=7 {
            title(&path, id, &format!("Title {id}"), 0, Some("R")).await;
        }
        for (index, id) in (1..=7).enumerate() {
            database
                .record_watch_progress(progress(
                    id,
                    None,
                    300_000,
                    Some(600_000),
                    NOW + i64::try_from(index).unwrap(),
                ))
                .await
                .unwrap();
        }

        let mut seen = Vec::new();
        let mut request = PageRequest::first(1);
        loop {
            let page = database
                .list_watch_progress(any_account_scope(), request)
                .await
                .unwrap();
            let Some(last) = page.last() else { break };
            seen.push(last.id);
            request = PageRequest::after(
                Cursor::at(last.progress.updated_at().as_millisecond(), last.id),
                1,
            );
        }
        assert_eq!(seen.len(), 7);
        let mut sorted = seen.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 7, "no row is repeated: {seen:?}");
    }

    // ---- empty tables -------------------------------------------------------------

    #[tokio::test]
    async fn an_empty_watch_state_returns_nothing() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _, _) = populated(&directory).await;

        for scope in [any_account_scope(), restricted()] {
            assert_eq!(
                database
                    .list_watch_progress(scope, PageRequest::first(10))
                    .await
                    .unwrap(),
                Vec::new(),
            );
            assert_eq!(
                database
                    .list_continue_watching(scope, PageRequest::first(10))
                    .await
                    .unwrap(),
                Vec::new(),
            );
            assert_eq!(
                database
                    .list_watchlist(scope, PageRequest::first(10))
                    .await
                    .unwrap(),
                Vec::new(),
            );
            assert_eq!(
                database
                    .watch_progress(scope, certified, None)
                    .await
                    .unwrap(),
                None
            );
            assert_eq!(
                database.watchlist_entry(scope, certified).await.unwrap(),
                None
            );
        }
    }

    // ---- rows this build cannot read ---------------------------------------------

    /// A phase that is not one of the stored spellings is reported, and so is a position the
    /// domain cannot hold.
    #[tokio::test]
    async fn a_row_this_build_cannot_read_is_reported() {
        let directory = TempDir::new().unwrap();
        let path = path_of(&directory);
        let (database, certified, _, _) = populated(&directory).await;
        database
            .record_watch_progress(progress(certified, None, 300_000, Some(600_000), NOW))
            .await
            .unwrap();

        let mut connection = open_writer_connection(&path).await.unwrap();
        sqlx::query("PRAGMA ignore_check_constraints = ON")
            .execute(&mut connection)
            .await
            .unwrap();
        sqlx::query("UPDATE watch_progress SET source = 'plex' WHERE title_id = ?1")
            .bind(certified)
            .execute(&mut connection)
            .await
            .unwrap();
        connection.close().await.unwrap();

        let error = database
            .watch_progress(any_account_scope(), certified, None)
            .await
            .expect_err("a source this build does not know is not read as one it does");
        assert!(
            matches!(
                error,
                StorageError::UnexpectedValue {
                    column: "watch_progress.source",
                    ..
                }
            ),
            "{error:?}"
        );
        assert!(error.to_string().contains("plex"), "{error}");
    }

    /// A watched row below the storage floor cannot have been written by this build, so
    /// reading it is a refusal rather than a value (I6).
    #[tokio::test]
    async fn a_watched_row_below_the_floor_is_refused_on_read() {
        let directory = TempDir::new().unwrap();
        let path = path_of(&directory);
        let (database, certified, _, _) = populated(&directory).await;
        database
            .record_watch_progress(progress(certified, None, 60_000, Some(600_000), NOW))
            .await
            .unwrap();

        let mut connection = open_writer_connection(&path).await.unwrap();
        sqlx::query("UPDATE watch_progress SET watched = 1, percent = 0.1 WHERE title_id = ?1")
            .bind(certified)
            .execute(&mut connection)
            .await
            .unwrap();
        connection.close().await.unwrap();

        let error = database
            .watch_progress(any_account_scope(), certified, None)
            .await
            .expect_err("a measured watched row below the floor is unreadable");
        assert!(
            matches!(
                error,
                StorageError::UnexpectedValue {
                    column: "watch_progress",
                    ..
                }
            ),
            "{error:?}"
        );
    }

    // ---- property tests ----------------------------------------------------------

    fn any_millis() -> impl Strategy<Value = i64> {
        1_600_000_000_000_i64..1_800_000_000_000
    }

    fn any_optional<T: std::fmt::Debug + Clone + 'static>(
        value: impl Strategy<Value = T>,
    ) -> impl Strategy<Value = Option<T>> {
        proptest::option::of(value)
    }

    // Every column of `watch_progress` survives the round trip, for any position, source, and
    // device label the schema accepts. The watch threshold is checked by the domain before the
    // write, so the property generates positions as a player reports them.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        #[test]
        fn any_watch_progress_round_trips(
            position_ms in 0_i64..3_600_000,
            duration_ms in proptest::option::of(1_i64..7_200_000),
            source in prop::sample::select(ProgressSource::ALL.to_vec()),
            device_label in any_optional("[A-Za-z0-9 -]{1,16}"),
            episode_id in proptest::option::of(1_i64..4),
            updated_at in any_millis(),
        ) {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a test runtime");
            runtime.block_on(async {
                let directory = TempDir::new().expect("a temporary directory");
                let path = path_of(&directory);
                let (database, certified, _, _) = populated(&directory).await;
                crate::test_support::season(&path, 1, certified, 1).await;
                for id in 1..=3 {
                    crate::test_support::episode(&path, id, certified, 1, 1, id).await;
                }

                let expected = WatchProgress::record(
                    account_id(),
                    certified,
                    episode_id,
                    PlaybackPosition::new(position_ms, duration_ms)
                        .expect("the strategy is a valid position"),
                    source,
                    at(updated_at),
                    device_label,
                );
                database
                    .record_watch_progress(expected.clone())
                    .await
                    .expect("writes");
                let stored = database
                    .watch_progress(any_account_scope(), certified, episode_id)
                    .await
                    .expect("reads")
                    .expect("the row");
                prop_assert_eq!(stored, expected);
                Ok(())
            })?;
        }
    }

    // Every column of `watchlist_entry` survives the round trip.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        #[test]
        fn any_watchlist_entry_round_trips(
            position in any_optional(0_u32..10_000),
            note in any_optional("[A-Za-z0-9 ,.!]{1,32}"),
            auto_acquire in any::<bool>(),
            added_at in any_millis(),
        ) {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a test runtime");
            runtime.block_on(async {
                let directory = TempDir::new().expect("a temporary directory");
                let (database, certified, _, _) = populated(&directory).await;

                let expected = WatchlistEntry {
                    position,
                    note,
                    auto_acquire,
                    ..WatchlistEntry::new(account_id(), certified, at(added_at))
                };
                database
                    .add_to_watchlist(expected.clone())
                    .await
                    .expect("writes");
                let stored = database
                    .watchlist_entry(any_account_scope(), certified)
                    .await
                    .expect("reads")
                    .expect("the row");
                prop_assert_eq!(stored, expected);
                Ok(())
            })?;
        }
    }
}
