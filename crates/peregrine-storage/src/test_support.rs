//! The fixture database every repository test opens.
//!
//! `catalogue.rs` and `jobs.rs` each grew their own three-line `database()` helper; the
//! library and watch-state suites need more than a path — an account, a title, and an episode
//! before a row can reference one — so the setup lives here once rather than four times.
//!
//! It also holds the *guard* fixtures two deletion-shaped suites share: rows in the tables
//! that are user data at any age, and a rendering of a table by `SQLite`'s own `quote()`. The
//! retention suite wrote both (PG-E3-15) and the maintenance suite needs the same two things
//! to answer the same question — did this run touch anything it was not allowed to — so they
//! moved here rather than being written a second time (PG-E3-16).
//!
//! Everything here is a *test* fixture: it writes rows with `sqlx` directly, which is what
//! lets a repository test set up a state the repository itself cannot create (a row written by
//! another version, a title with no certification) and leaves the repositories' own writes to
//! be the thing under test. No value comes from the wall clock (AGENTS R14).
//!
//! Some fixture is used by one suite and not another, which is why the module allows dead code
//! as a whole: a `#[cfg(test)]` helper that only `watch` calls is not dead, it is shared.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::Path;

use sqlx::Connection as _;
use sqlx::Row as _;

use crate::database::Database;

/// The instant every fixture writes.
pub(crate) const NOW: i64 = 1_700_000_000_000;

/// One day, as the age fixtures count.
const DAY_MILLIS: i64 = 24 * 60 * 60 * 1_000;

/// An instant `days` before [`NOW`]; negative values are in the future.
pub(crate) const fn days_before(days: i64) -> i64 {
    NOW - days * DAY_MILLIS
}

/// A database in a fresh temporary directory.
pub(crate) async fn database(directory: &tempfile::TempDir) -> Database {
    Database::open(directory.path().join("peregrine.sqlite"))
        .await
        .unwrap()
}

/// The database's path, for a fixture that opens its own writer connection.
pub(crate) fn path_of(directory: &tempfile::TempDir) -> std::path::PathBuf {
    directory.path().join("peregrine.sqlite")
}

/// Runs a statement on its own writer connection and returns the rowid it inserted.
///
/// A test uses this for the rows the repositories are not allowed to create: a title with no
/// `content_rating`, an `unmatched_file` whose suggestion is a hidden title, a row written
/// with `ignore_check_constraints` on. The SQL is assembled by the fixture, so it is wrapped
/// in `AssertSqlSafe`; nothing here ever reaches the product.
pub(crate) async fn execute(path: &Path, sql: &str) -> i64 {
    let mut connection = crate::database::open_writer_connection(path).await.unwrap();
    let result = sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
        .execute(&mut connection)
        .await
        .unwrap();
    let id = result.last_insert_rowid();
    connection.close().await.unwrap();
    id
}

/// Runs `statements` on their own writer connection, in order.
///
/// A suite that seeds a dozen tables — retention seeds every eligible table and the user-data
/// tables its guard compares — should not open a connection per row, and the connection
/// discipline is the same as [`execute`]'s: one writer, closed before the assertions run.
pub(crate) async fn seed(path: &Path, statements: &[String]) {
    let mut connection = crate::database::open_writer_connection(path).await.unwrap();
    for statement in statements {
        sqlx::query(sqlx::AssertSqlSafe(statement.clone()))
            .execute(&mut connection)
            .await
            .unwrap();
    }
    connection.close().await.unwrap();
}

/// An account with `id`, created once however many tests ask for it.
pub(crate) async fn account(path: &Path, id: i64, name: &str) {
    let _ = execute(
        path,
        &format!(
            "INSERT OR IGNORE INTO account (id, name, kind, is_default, created_at) \
             VALUES ({id}, '{name}', 'adult', 0, {NOW})"
        ),
    )
    .await;
}

/// A title with `id`, set to `adult` and certified with `rating` when one is given.
pub(crate) async fn title(path: &Path, id: i64, name: &str, adult: i64, rating: Option<&str>) {
    let rating = match rating {
        Some(value) => format!("'{value}'"),
        None => "NULL".to_owned(),
    };
    let _ = execute(
        path,
        &format!(
            "INSERT INTO title (id, kind, subtype, name, sort_name, content_rating, adult, \
             metadata_state, created_at, updated_at) \
             VALUES ({id}, 'movie', 'standard', '{name}', '{name}', {rating}, {adult}, \
             'complete', {NOW}, {NOW})"
        ),
    )
    .await;
}

/// A season of `title_id` with `id` and `number`, which an episode needs.
pub(crate) async fn season(path: &Path, id: i64, title_id: i64, number: i64) {
    let _ = execute(
        path,
        &format!(
            "INSERT INTO season (id, title_id, season_number, updated_at) \
             VALUES ({id}, {title_id}, {number}, {NOW})"
        ),
    )
    .await;
}

/// An episode of `title_id` and `season_id` with `id`.
pub(crate) async fn episode(
    path: &Path,
    id: i64,
    title_id: i64,
    season_id: i64,
    season_number: i64,
    episode_number: i64,
) {
    let _ = execute(
        path,
        &format!(
            "INSERT INTO episode (id, title_id, season_id, season_number, episode_number, \
             updated_at) VALUES ({id}, {title_id}, {season_id}, {season_number}, \
             {episode_number}, {NOW})"
        ),
    )
    .await;
}

/// Rows in the tables a deletion-shaped run must not touch, every one of them old enough that
/// a sweep by age alone would remove it.
///
/// This is the list that makes "user data is never eligible" checkable: a run that reaches any
/// of these tables fails the identity comparison, whatever its policy says it does.
pub(crate) fn user_data_rows() -> Vec<String> {
    let mut rows = catalogue_rows();
    rows.extend(library_rows());
    rows
}

/// The catalogue half of [`user_data_rows`]: identity, titles, and their children.
fn catalogue_rows() -> Vec<String> {
    let old = days_before(400);
    vec![
        format!(
            "INSERT OR REPLACE INTO account (id, name, kind, is_default, created_at) VALUES (1, \
             'Adult', 'adult', 1, {old})"
        ),
        format!(
            "INSERT INTO title (id, kind, name, sort_name, content_rating, metadata_state, \
             created_at, updated_at) VALUES (1, 'show', 'A Show', 'A Show', 'TV-14', 'complete', \
             {old}, {old})"
        ),
        "INSERT INTO title_alias (title_id, alias) VALUES (1, 'An Alias')".to_owned(),
        format!(
            "INSERT INTO external_id (title_id, provider, value, fetched_at) VALUES (1, 'tmdb', \
             '1', {old})"
        ),
        "INSERT INTO genre (id, name) VALUES (1, 'Drama')".to_owned(),
        "INSERT INTO title_genre (title_id, genre_id) VALUES (1, 1)".to_owned(),
        "INSERT INTO keyword (id, name) VALUES (1, 'slow burn')".to_owned(),
        "INSERT INTO title_keyword (title_id, keyword_id) VALUES (1, 1)".to_owned(),
        "INSERT INTO company (id, name) VALUES (1, 'A Studio')".to_owned(),
        "INSERT INTO title_company (title_id, company_id, role) VALUES (1, 1, 'studio')".to_owned(),
        format!(
            "INSERT INTO person (id, name, sort_name, updated_at) VALUES (1, 'A Person', \
             'Person, A', {old})"
        ),
        "INSERT INTO credit (title_id, person_id, kind, order_index) VALUES (1, 1, 'cast', 0)"
            .to_owned(),
        format!(
            "INSERT INTO season (id, title_id, season_number, updated_at) VALUES (1, 1, 1, {old})"
        ),
        format!(
            "INSERT INTO episode (id, title_id, season_id, season_number, episode_number, \
             updated_at) VALUES (1, 1, 1, 1, 1, {old})"
        ),
        "INSERT INTO episode_ordering (episode_id, scheme, season_number, episode_number) \
         VALUES (1, 'aired', 1, 1)"
            .to_owned(),
    ]
}

/// The library, watch-state, acquisition, cache, and platform half of [`user_data_rows`].
fn library_rows() -> Vec<String> {
    let old = days_before(400);
    vec![
        format!(
            "INSERT INTO library_root (id, path, kind, created_at) VALUES (1, '/media', 'shows', \
             {old})"
        ),
        format!(
            "INSERT INTO media_item (id, title_id, state, origin, created_at, updated_at) VALUES \
             (1, 1, 'available', 'scan', {old}, {old})"
        ),
        format!(
            "INSERT INTO media_file (id, media_item_id, path, root_id, size_bytes, mtime_ms, \
             added_at, updated_at) VALUES (1, 1, '/media/s01e01.mkv', 1, 4096, {old}, {old}, \
             {old})"
        ),
        format!(
            "INSERT INTO subtitle_track (id, media_file_id, origin, format, language, path, \
             downloaded_at, last_used_at) VALUES (1, 1, 'external', 'srt', 'en', \
             '/media/s01e01.en.srt', {old}, {old})"
        ),
        format!(
            "INSERT INTO subtitle_download (id, media_file_id, provider, provider_subtitle_id, \
             language, score, created_at) VALUES (1, 1, 'opensubtitles', 'x', 'en', 1.0, {old})"
        ),
        format!(
            "INSERT INTO watch_progress (account_id, title_id, episode_id, position_ms, percent, \
             updated_at, watched_at) VALUES (1, 1, 1, 1000, 0.5, {old}, {old})"
        ),
        format!(
            "INSERT INTO watchlist_entry (account_id, title_id, added_at) VALUES (1, 1, {old})"
        ),
        format!(
            "INSERT INTO smart_collection (id, name, query_json, sort, created_at) VALUES (1, \
             'Unwatched', '{{}}', 'name', {old})"
        ),
        format!(
            "INSERT INTO quality_profile (id, name, rules_json, cutoff_rank, updated_at) VALUES \
             (1, 'Default', '{{}}', 1, {old})"
        ),
        format!(
            "INSERT INTO indexer (id, kind, name, last_check_at) VALUES (1, 'builtin', 'Built \
             in', {old})"
        ),
        format!(
            "INSERT INTO download_job (id, title_id, state, release_title, save_path, added_at, \
             completed_at) VALUES (1, 1, 'completed', 'A.Release', '/downloads', {old}, {old})"
        ),
        "INSERT INTO download_file (id, download_job_id, file_index, path, size_bytes) \
         VALUES (1, 1, 0, '/downloads/a.mkv', 4096)"
            .to_owned(),
        format!(
            "INSERT INTO image_cache (key, provider, path, variant, disk_path, bytes, created_at, \
             last_used_at) VALUES ('image.old', 'tmdb', '/p', 'poster', '/cache/p.webp', 4096, \
             {old}, {old})"
        ),
        "INSERT INTO image_ref (owner_kind, owner_id, role, provider, path, language) VALUES \
         ('title', 1, 'poster', 'tmdb', '/p', '')"
            .to_owned(),
        format!(
            "INSERT INTO dataset_state (name, rows, size_bytes, last_checked_at, last_updated_at) \
             VALUES ('imdb.ratings', 100, 60000000, {old}, {old})"
        ),
        format!(
            "INSERT INTO unmatched_file (id, path, size_bytes, mtime_ms, first_seen_at) VALUES \
             (1, '/media/orphan.mkv', 4096, {old}, {old})"
        ),
        format!(
            "INSERT INTO device (id, protocol, device_uid, name, last_seen_at) VALUES (1, \
             'chromecast', 'uid', 'Living Room', {old})"
        ),
        format!(
            "INSERT INTO cast_session (id, device_id, state, delivery, position_ms, started_at, \
             ended_at) VALUES (1, 1, 'ended', 'direct', 0, {old}, {old})"
        ),
        format!(
            "INSERT INTO settings (key, value_json, updated_at) VALUES ('ui.theme', '{{}}', {old})"
        ),
        format!(
            "INSERT INTO secret_ref (key, keyring_service, keyring_account, created_at) VALUES \
             ('tmdb.api_key', 'peregrine', 'tmdb', {old})"
        ),
    ]
}

/// Every table of the schema that can be read back row by row, rendered by `SQLite` itself.
///
/// `skip` names the tables the caller expects to have changed; they are left out so a caller
/// comparing two snapshots is comparing what must not have moved.
pub(crate) async fn snapshot(path: &Path, skip: &[&str]) -> BTreeMap<String, String> {
    let mut connection = crate::database::open_writer_connection(path).await.unwrap();
    let names = tables(&mut connection).await;
    let mut snapshot = BTreeMap::new();
    for table in names {
        if skip.contains(&table.as_str()) {
            continue;
        }
        snapshot.insert(table.clone(), dump_with(&mut connection, &table).await);
    }
    connection.close().await.unwrap();
    snapshot
}

/// The tables a snapshot covers.
///
/// The migration runner's bookkeeping is not user data, and the virtual search index cannot be
/// read column by column, but its shadow tables are ordinary tables and are included, so damage
/// to the index would still show. `SQLite`'s own tables are included too: `sqlite_stat1` is what
/// `ANALYZE` writes, and a maintenance run that claimed to leave the database alone while
/// rewriting that table would otherwise go unnoticed.
async fn tables(connection: &mut sqlx::sqlite::SqliteConnection) -> Vec<String> {
    let rows = sqlx::query(
        "SELECT name, sql FROM sqlite_master WHERE type = 'table' \
         AND name <> '_sqlx_migrations' ORDER BY name",
    )
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    rows.iter()
        .filter(|row| {
            !row.get::<Option<String>, _>("sql")
                .unwrap_or_default()
                .starts_with("CREATE VIRTUAL TABLE")
        })
        .map(|row| row.get::<String, _>("name"))
        .collect()
}

/// Every row of `table` as text, rendered and sorted by `SQLite`.
///
/// `quote()` gives each value its SQL literal — `NULL`, a number, a quoted string, or `X'..'`
/// for a blob — so the comparison is over the stored bytes rather than over a lossy display.
/// Sorting the rendered lines makes it a comparison of the rows themselves: the order SQLite
/// happens to walk a table in is not user data, and one of these tables is `WITHOUT ROWID`.
pub(crate) async fn dump(path: &Path, table: &str) -> String {
    let mut connection = crate::database::open_writer_connection(path).await.unwrap();
    let rendered = dump_with(&mut connection, table).await;
    connection.close().await.unwrap();
    rendered
}

async fn dump_with(connection: &mut sqlx::sqlite::SqliteConnection, table: &str) -> String {
    let columns = columns_of(connection, table).await;
    let rendered = columns
        .iter()
        .map(|column| format!("quote(\"{column}\")"))
        .collect::<Vec<_>>()
        .join(" || '|' || ");
    let sql = format!(
        "SELECT COALESCE(group_concat(line, char(10)), '') FROM \
         (SELECT {rendered} AS line FROM \"{table}\" ORDER BY line)"
    );
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .fetch_one(&mut *connection)
        .await
        .unwrap()
}

/// The columns of `table`, in declaration order.
pub(crate) async fn columns(path: &Path, table: &str) -> Vec<String> {
    let mut connection = crate::database::open_writer_connection(path).await.unwrap();
    let columns = columns_of(&mut connection, table).await;
    connection.close().await.unwrap();
    columns
}

async fn columns_of(connection: &mut sqlx::sqlite::SqliteConnection, table: &str) -> Vec<String> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "PRAGMA table_info(\"{table}\")"
    )))
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    rows.iter()
        .map(|row| row.get::<String, _>("name"))
        .collect()
}

/// The single number a query returns.
pub(crate) async fn count(path: &Path, sql: &str) -> i64 {
    let mut connection = crate::database::open_writer_connection(path).await.unwrap();
    let count = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .fetch_one(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    count
}
