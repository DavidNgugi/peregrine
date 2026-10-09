//! The embedded migration set and the runner that applies it.
//!
//! Migrations are numbered, append-only files under `migrations/`, embedded in the
//! binary at compile time so a packaged application cannot run with a migration
//! directory that is missing or was replaced. `sqlx` records each migration's checksum
//! in `_sqlx_migrations`, so an edited migration that has already run fails the open
//! rather than leaving the schema half-described by two versions of the same file.

use sqlx::SqliteConnection;
use sqlx::migrate::{MigrateError, Migrator};
use tracing::info;

/// The embedded migrations. `build.rs` keeps cargo watching the directory.
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Applies every pending migration and logs the set that is now in force.
///
/// Runs on the writer connection before the writer task starts, so no reader exists
/// while the schema is changing and every later write shares the same connection.
///
/// Returns how many migrations the embedded set now holds; after a successful run every
/// one of them is applied, so that is also the count of migrations in force.
pub(crate) async fn apply(connection: &mut SqliteConnection) -> Result<usize, MigrateError> {
    MIGRATOR.run(&mut *connection).await?;

    let applied: Vec<String> = MIGRATOR
        .iter()
        .map(|migration| format!("{:04}_{}", migration.version, migration.description))
        .collect();
    info!(migrations = ?applied, "embedded migrations applied");
    Ok(applied.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{Database, open_writer_connection};
    use sqlx::Connection;
    use tempfile::TempDir;

    /// The five tables migration 0001 creates, per `DATA_MODEL` §2.1 and §2.8.
    const DOCUMENTED_TABLES: [&str; 5] = ["account", "event_log", "job", "secret_ref", "settings"];

    /// `(name, declared type, notnull, default, pk)`, exactly as `PRAGMA table_info` reports it.
    type ColumnShape = (&'static str, &'static str, i64, Option<&'static str>, i64);

    const ACCOUNT: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("name", "TEXT", 1, None, 0),
        ("kind", "TEXT", 1, Some("'adult'"), 0),
        ("avatar_kind", "TEXT", 1, Some("'initials'"), 0),
        ("avatar_ref", "TEXT", 0, None, 0),
        ("avatar_blurhash", "TEXT", 0, None, 0),
        ("accent_color", "TEXT", 0, None, 0),
        ("rating_ceiling", "TEXT", 0, None, 0),
        ("curated_only", "INTEGER", 1, Some("0"), 0),
        ("can_acquire", "INTEGER", 1, Some("1"), 0),
        ("can_delete", "INTEGER", 1, Some("0"), 0),
        ("can_manage", "INTEGER", 1, Some("0"), 0),
        ("can_cast", "INTEGER", 1, Some("1"), 0),
        ("can_use_agents", "INTEGER", 1, Some("0"), 0),
        ("can_see_queue", "INTEGER", 1, Some("1"), 0),
        ("lock_policy", "TEXT", 1, Some("'none'"), 0),
        ("pin_hash", "TEXT", 0, None, 0),
        ("idle_lock_minutes", "INTEGER", 0, None, 0),
        ("is_default", "INTEGER", 1, Some("0"), 0),
        ("created_at", "INTEGER", 1, None, 0),
        ("last_used_at", "INTEGER", 0, None, 0),
    ];

    const SETTINGS: &[ColumnShape] = &[
        // A non-INTEGER primary key is nullable in SQLite, so the driver reports 0 here.
        ("key", "TEXT", 0, None, 1),
        ("value_json", "TEXT", 1, None, 0),
        ("updated_at", "INTEGER", 1, None, 0),
    ];

    const SECRET_REF: &[ColumnShape] = &[
        ("key", "TEXT", 0, None, 1),
        ("keyring_service", "TEXT", 1, None, 0),
        ("keyring_account", "TEXT", 1, None, 0),
        ("created_at", "INTEGER", 1, None, 0),
    ];

    const JOB: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("kind", "TEXT", 1, None, 0),
        ("state", "TEXT", 1, None, 0),
        ("priority", "INTEGER", 1, Some("100"), 0),
        ("dedupe_key", "TEXT", 0, None, 0),
        ("payload_json", "TEXT", 1, None, 0),
        ("progress_permille", "INTEGER", 1, Some("0"), 0),
        ("attempts", "INTEGER", 1, Some("0"), 0),
        ("max_attempts", "INTEGER", 1, Some("3"), 0),
        ("next_attempt_at", "INTEGER", 0, None, 0),
        ("last_error_code", "TEXT", 0, None, 0),
        ("last_error", "TEXT", 0, None, 0),
        ("created_at", "INTEGER", 1, None, 0),
        ("started_at", "INTEGER", 0, None, 0),
        ("finished_at", "INTEGER", 0, None, 0),
        ("cancel_requested", "INTEGER", 1, Some("0"), 0),
    ];

    const EVENT_LOG: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("at", "INTEGER", 1, None, 0),
        ("kind", "TEXT", 1, None, 0),
        ("subject_kind", "TEXT", 0, None, 0),
        ("subject_id", "INTEGER", 0, None, 0),
        ("detail_json", "TEXT", 0, None, 0),
    ];

    /// A connection with migration 0001 applied, plus the resources it needs to live.
    async fn migrated(directory: &TempDir) -> SqliteConnection {
        let mut connection = open_writer_connection(&directory.path().join("peregrine.sqlite"))
            .await
            .unwrap();
        apply(&mut connection).await.unwrap();
        connection
    }

    fn rendered(shape: &[ColumnShape]) -> Vec<(String, String, i64, Option<String>, i64)> {
        shape
            .iter()
            .map(
                |(name, declared_type, not_null, default_value, primary_key)| {
                    (
                        (*name).to_owned(),
                        (*declared_type).to_owned(),
                        *not_null,
                        default_value.map(str::to_owned),
                        *primary_key,
                    )
                },
            )
            .collect()
    }

    async fn columns(
        connection: &mut SqliteConnection,
        table: &str,
    ) -> Vec<(String, String, i64, Option<String>, i64)> {
        sqlx::query_as(
            "SELECT name, type, \"notnull\", dflt_value, pk \
             FROM pragma_table_info(?1) ORDER BY cid",
        )
        .bind(table)
        .fetch_all(&mut *connection)
        .await
        .unwrap()
    }

    async fn names(connection: &mut SqliteConnection, sql: &'static str) -> Vec<String> {
        sqlx::query_scalar(sql)
            .fetch_all(&mut *connection)
            .await
            .unwrap()
    }

    async fn versions(connection: &mut SqliteConnection) -> Vec<i64> {
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&mut *connection)
            .await
            .unwrap()
    }

    async fn insert_account(
        connection: &mut SqliteConnection,
        kind: &str,
        rating_ceiling: Option<&str>,
        curated_only: i64,
        can_acquire: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO account (name, kind, rating_ceiling, curated_only, can_acquire, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, 0)",
        )
        .bind("Test account")
        .bind(kind)
        .bind(rating_ceiling)
        .bind(curated_only)
        .bind(can_acquire)
        .execute(&mut *connection)
        .await
        .map(|_| ())
    }

    #[tokio::test]
    async fn migration_applies_to_an_empty_database() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");
        assert!(
            !path.exists(),
            "the fixture must start with no database file"
        );

        let mut connection = open_writer_connection(&path).await.unwrap();
        let applied = apply(&mut connection).await.unwrap();

        assert_eq!(
            applied, 1,
            "migration 0001 is the only migration in the set"
        );
        assert_eq!(
            names(
                &mut connection,
                "SELECT name FROM sqlite_master WHERE type = 'table' \
                 AND name NOT LIKE 'sqlite_%' AND name <> '_sqlx_migrations' ORDER BY name",
            )
            .await,
            DOCUMENTED_TABLES
        );
        assert_eq!(
            versions(&mut connection).await,
            vec![1],
            "the applied migration must be recorded"
        );
    }

    #[tokio::test]
    async fn every_documented_column_is_present_with_its_declared_shape() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated(&directory).await;

        for (table, shape) in [
            ("account", ACCOUNT),
            ("settings", SETTINGS),
            ("secret_ref", SECRET_REF),
            ("job", JOB),
            ("event_log", EVENT_LOG),
        ] {
            assert_eq!(
                columns(&mut connection, table).await,
                rendered(shape),
                "columns of `{table}`"
            );
        }
    }

    #[tokio::test]
    async fn the_documented_indices_exist_with_their_predicates() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated(&directory).await;

        assert_eq!(
            names(
                &mut connection,
                "SELECT name FROM sqlite_master WHERE type = 'index' AND sql IS NOT NULL \
                 ORDER BY name",
            )
            .await,
            vec!["event_log_at_idx", "job_dedupe_uq", "job_ready_idx"]
        );

        let dedupe: String = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = 'job_dedupe_uq'",
        )
        .fetch_one(&mut connection)
        .await
        .unwrap();
        assert!(
            dedupe.contains("UNIQUE INDEX job_dedupe_uq ON job (kind, dedupe_key)"),
            "the dedupe index must cover (kind, dedupe_key): {dedupe}"
        );
        assert!(
            dedupe.contains(
                "WHERE dedupe_key IS NOT NULL AND state IN ('queued','running','deferred')"
            ),
            "the dedupe index must be partial over non-terminal states (I12): {dedupe}"
        );
    }

    #[tokio::test]
    async fn a_child_account_without_a_ceiling_or_curation_is_rejected() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated(&directory).await;

        let error = insert_account(&mut connection, "child", None, 0, 0)
            .await
            .expect_err("an unrestricted child account is a configuration mistake");

        assert!(error.to_string().contains("CHECK"), "got {error}");
        insert_account(&mut connection, "child", Some("PG"), 0, 0)
            .await
            .expect("a ceiling is enough to make a child account safe");
        insert_account(&mut connection, "child", None, 1, 0)
            .await
            .expect("curation is enough to make a child account safe");
    }

    #[tokio::test]
    async fn a_child_account_that_can_acquire_is_rejected() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated(&directory).await;

        let error = insert_account(&mut connection, "child", Some("PG"), 0, 1)
            .await
            .expect_err("no path may let a child acquire media (I28)");

        assert!(error.to_string().contains("CHECK"), "got {error}");
    }

    #[tokio::test]
    async fn unknown_enumeration_values_are_rejected() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated(&directory).await;

        insert_account(&mut connection, "adult", None, 0, 1)
            .await
            .expect("an adult account is unrestricted");
        assert!(
            insert_account(&mut connection, "teen", None, 0, 1)
                .await
                .is_err(),
            "`kind` accepts only the recorded values"
        );
        assert!(
            sqlx::query(
                "INSERT INTO job (kind, state, payload_json, created_at) \
                         VALUES ('metadata.refresh', 'halfway', '{}', 0)"
            )
            .execute(&mut connection)
            .await
            .is_err(),
            "`job.state` accepts only the recorded values"
        );
    }

    #[tokio::test]
    async fn boolean_columns_reject_values_other_than_zero_or_one() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated(&directory).await;

        let error = sqlx::query(
            "INSERT INTO account (name, kind, can_cast, created_at) VALUES ('Test', 'adult', 2, 0)",
        )
        .execute(&mut connection)
        .await
        .expect_err("booleans are INTEGER NOT NULL CHECK (x IN (0,1))");

        assert!(error.to_string().contains("CHECK"), "got {error}");
    }

    async fn insert_job(
        connection: &mut SqliteConnection,
        state: &str,
        dedupe_key: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO job (kind, state, dedupe_key, payload_json, created_at) \
             VALUES ('metadata.refresh', ?1, ?2, '{}', 0)",
        )
        .bind(state)
        .bind(dedupe_key)
        .execute(&mut *connection)
        .await
        .map(|_| ())
    }

    /// I12: a terminal job row is never used for dedupe, so it must not occupy the key.
    #[tokio::test]
    async fn the_dedupe_index_excludes_terminal_jobs_and_null_keys() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated(&directory).await;

        insert_job(&mut connection, "queued", Some("tmdb:1"))
            .await
            .unwrap();
        assert!(
            insert_job(&mut connection, "running", Some("tmdb:1"))
                .await
                .is_err(),
            "two non-terminal jobs must not share a dedupe key"
        );
        insert_job(&mut connection, "succeeded", Some("tmdb:1"))
            .await
            .expect("a terminal job frees the key");
        insert_job(&mut connection, "succeeded", Some("tmdb:1"))
            .await
            .expect("terminal jobs are not deduplicated at all");

        insert_job(&mut connection, "queued", None).await.unwrap();
        insert_job(&mut connection, "queued", None)
            .await
            .expect("a null dedupe key is never deduplicated");
    }

    #[tokio::test]
    async fn required_event_log_columns_are_enforced() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated(&directory).await;

        assert!(
            sqlx::query("INSERT INTO event_log (kind) VALUES ('media.added')")
                .execute(&mut connection)
                .await
                .is_err(),
            "`event_log.at` is NOT NULL"
        );
        sqlx::query("INSERT INTO event_log (at, kind) VALUES (1_700_000_000_000, 'media.added')")
            .execute(&mut connection)
            .await
            .expect("an event needs only a time and a kind");
    }

    #[tokio::test]
    async fn reapplying_the_embedded_migrations_changes_nothing() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");

        let mut first = open_writer_connection(&path).await.unwrap();
        apply(&mut first).await.unwrap();
        first.close().await.unwrap();

        let mut second = open_writer_connection(&path).await.unwrap();
        let applied = apply(&mut second).await.unwrap();

        assert_eq!(applied, 1);
        assert_eq!(
            versions(&mut second).await,
            vec![1],
            "a second run must not record the migration twice"
        );
    }

    /// Migration 0001 is a `Simple` migration: it has no down script, so `sqlx` cannot
    /// revert it and this test says so rather than implying a rollback exists.
    #[tokio::test]
    async fn migration_0001_is_up_only_so_undo_cannot_revert_it() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated(&directory).await;

        assert!(
            MIGRATOR
                .iter()
                .all(|migration| migration.migration_type.is_up_migration()),
            "the repository gate numbers one file per migration, so a .down.sql pair is not possible"
        );

        MIGRATOR.undo(&mut connection, 0).await.unwrap();

        assert_eq!(
            names(
                &mut connection,
                "SELECT name FROM sqlite_master WHERE type = 'table' \
                 AND name NOT LIKE 'sqlite_%' AND name <> '_sqlx_migrations' ORDER BY name",
            )
            .await,
            DOCUMENTED_TABLES,
            "undo is a no-op because no down migration exists"
        );
    }

    /// "Reversible in a test fixture": the fixture can be thrown away and rebuilt from
    /// nothing with an identical schema, which is what the later migration tasks need in
    /// order to test an upgrade from an empty database.
    #[tokio::test]
    async fn a_reset_fixture_rebuilds_the_identical_schema() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");

        let mut connection = open_writer_connection(&path).await.unwrap();
        apply(&mut connection).await.unwrap();
        let before = schema(&mut connection).await;
        connection.close().await.unwrap();

        std::fs::remove_file(&path).unwrap();
        remove_if_present(&path.with_extension("sqlite-wal"));
        remove_if_present(&path.with_extension("sqlite-shm"));

        let mut connection = open_writer_connection(&path).await.unwrap();
        apply(&mut connection).await.unwrap();

        assert_eq!(before, schema(&mut connection).await);
    }

    fn remove_if_present(path: &std::path::Path) {
        if path.exists() {
            std::fs::remove_file(path).unwrap();
        }
    }

    async fn schema(connection: &mut SqliteConnection) -> Vec<(String, String, String)> {
        sqlx::query_as(
            "SELECT type, name, COALESCE(sql, '') FROM sqlite_master \
             WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
        )
        .fetch_all(&mut *connection)
        .await
        .unwrap()
    }

    /// The database handle must expose the same schema the raw connection does.
    #[tokio::test]
    async fn the_database_handle_opens_a_migrated_file() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");

        let database = Database::open(&path).await.unwrap();
        database
            .append_event(crate::writer::NewEvent {
                at_millis: 1_700_000_000_000,
                kind: "media.added".to_owned(),
                subject_kind: Some("title".to_owned()),
                subject_id: Some(1),
                detail_json: None,
            })
            .await
            .unwrap();

        assert!(path.is_file(), "open must have created the database file");
        assert_eq!(database.event_count().await.unwrap(), 1);
    }
}
