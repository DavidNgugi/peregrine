//! The fixture database every repository test opens.
//!
//! `catalogue.rs` and `jobs.rs` each grew their own three-line `database()` helper; the
//! library and watch-state suites need more than a path — an account, a title, and an episode
//! before a row can reference one — so the setup lives here once rather than four times.
//!
//! Everything here is a *test* fixture: it writes rows with `sqlx` directly, which is what
//! lets a repository test set up a state the repository itself cannot create (a row written by
//! another version, a title with no certification) and leaves the repositories' own writes to
//! be the thing under test. No value comes from the wall clock (AGENTS R14).
//!
//! Some fixture is used by one suite and not another, which is why the module allows dead code
//! as a whole: a `#[cfg(test)]` helper that only `watch` calls is not dead, it is shared.

#![allow(dead_code)]

use std::path::Path;

use sqlx::Connection as _;

use crate::database::Database;

/// The instant every fixture writes.
pub(crate) const NOW: i64 = 1_700_000_000_000;

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
