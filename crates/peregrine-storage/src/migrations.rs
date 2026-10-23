//! The embedded migration set and the runner that applies it.
//!
//! Migrations are numbered, append-only files under `migrations/`, embedded in the
//! binary at compile time so a packaged application cannot run with a migration
//! directory that is missing or was replaced. `sqlx` records each migration's checksum
//! in `_sqlx_migrations`, so an edited migration that has already run fails the open
//! rather than leaving the schema half-described by two versions of the same file.
//!
//! The runner is deliberately more than `Migrator::run`: an existing database is copied
//! before the first pending migration ([`crate::backup`]), because rolling a schema change
//! back is not supported and the copy is the only way back (`DATA_MODEL` §5). A failure is
//! reported as [`StorageError::MigrationFailed`], naming the migration and the copy.

use std::path::{Path, PathBuf};

use sqlx::SqliteConnection;
use sqlx::migrate::{MigrateError, Migration, Migrator};
use tracing::{info, warn};

use crate::error::StorageError;

/// The embedded migrations. `build.rs` keeps cargo watching the directory.
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Applies every pending migration, copying an existing database first.
///
/// Runs on the writer connection before the writer task starts, so no reader exists
/// while the schema is changing and every later write shares the same connection.
///
/// Returns how many migrations the embedded set now holds; after a successful run every
/// one of them is applied, so that is also the count of migrations in force.
///
/// # Errors
///
/// [`StorageError::Backup`] when the pre-migration copy cannot be taken — the migrations
/// are then not run at all — and [`StorageError::MigrationFailed`] when one fails, leaving
/// the database as it was.
pub(crate) async fn apply(
    connection: &mut SqliteConnection,
    path: &Path,
) -> Result<usize, StorageError> {
    run(connection, path, &MIGRATOR).await
}

/// How many migrations the embedded set holds.
///
/// The number a test needs without applying anything: a copy holds every applied migration, and a
/// test that writes the count by hand breaks every time one is added.
#[cfg(test)]
#[must_use]
pub(crate) fn count() -> usize {
    MIGRATOR.iter().count()
}

/// The newest migration version this build embeds.
///
/// A backup records the newest version it holds, and a restore refuses one above this number
/// rather than running an older binary against a schema it cannot read.
#[must_use]
pub(crate) fn latest_version() -> i64 {
    MIGRATOR
        .iter()
        .map(|migration| migration.version)
        .max()
        .unwrap_or_default()
}

/// Applies `migrator`'s pending migrations, copying an existing database first.
///
/// The migrator is a parameter so a test can run a set that contains a migration designed
/// to fail; the product always passes the embedded set.
async fn run(
    connection: &mut SqliteConnection,
    path: &Path,
    migrator: &Migrator,
) -> Result<usize, StorageError> {
    let in_force = applied_versions(connection).await?;
    let pending = migrator
        .iter()
        .filter(|migration| !in_force.contains(&migration.version))
        .count();

    // A database with nothing applied is a first install: there is no user data to copy,
    // and a copy of an empty file would put a file in the data directory that says nothing.
    let backup = if pending > 0 && !in_force.is_empty() {
        let from_version = in_force.iter().copied().max().unwrap_or_default();
        Some(crate::backup::take(connection, path, from_version).await?)
    } else {
        None
    };

    if let Err(error) = migrator.run(&mut *connection).await {
        return Err(failure(error, backup, migrator));
    }

    let applied: Vec<String> = migrator.iter().map(rendered).collect();
    info!(
        migrations = ?applied,
        applied = pending,
        "embedded migrations applied"
    );
    Ok(applied.len())
}

/// The migration versions the database has recorded, oldest first.
///
/// `_sqlx_migrations` is the table `sqlx`'s migrator owns and names by default; the count
/// is read here rather than through the migrator because the decision to copy the database
/// has to be taken before the migrator runs.
async fn applied_versions(connection: &mut SqliteConnection) -> Result<Vec<i64>, StorageError> {
    let table: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
    )
    .fetch_one(&mut *connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    if table == 0 {
        return Ok(Vec::new());
    }

    sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
        .fetch_all(&mut *connection)
        .await
        .map_err(StorageError::from_sqlx)
}

/// `0001_init`, the form every log line and error uses to name a migration.
fn rendered(migration: &Migration) -> String {
    format!("{:04}_{}", migration.version, migration.description)
}

/// The typed error for a failed run, naming the migration when the driver named one.
///
/// A failure the driver did not attribute to a migration — a checksum mismatch, a dirty
/// record — is not one migration's fault, so it keeps the generic variant rather than
/// guessing at a version.
fn failure(error: MigrateError, backup: Option<PathBuf>, migrator: &Migrator) -> StorageError {
    let MigrateError::ExecuteMigration(_, version) = &error else {
        return StorageError::migrate(error);
    };
    let version = *version;
    let description = migrator
        .iter()
        .find(|migration| migration.version == version)
        .map_or_else(
            || "unknown".to_owned(),
            |migration| migration.description.to_string(),
        );
    warn!(
        version,
        description,
        backup = ?backup,
        %error,
        "a pending migration failed; the database was left unchanged"
    );
    StorageError::MigrationFailed {
        version,
        description,
        backup,
        source: Box::new(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{Database, open_writer_connection};
    use sqlx::Connection;
    use sqlx::SqlSafeStr as _;
    use sqlx::migrate::MigrationType;
    use tempfile::TempDir;

    /// The FTS5 index and the shadow tables `SQLite` keeps beside it.
    ///
    /// They hold no user row — the index is derived from `title` and can be rebuilt from it
    /// (`PG-E3-12`) — so they are not part of the table set that asserts the schema's tables,
    /// and `the_search_index_is_a_contentless_fts5_table` asserts them on their own. The four
    /// shadow tables are created by the FTS5 module itself and are not named by a migration.
    const FTS_TIER: [&str; 5] = [
        "title_fts",
        "title_fts_config",
        "title_fts_data",
        "title_fts_docsize",
        "title_fts_idx",
    ];

    /// The tables the migrations create, per `DATA_MODEL` §2.1–§2.8.
    const DOCUMENTED_TABLES: [&str; 51] = [
        "account",
        "cast_session",
        "company",
        "credit",
        "dataset_state",
        "device",
        "discover_rail",
        "download_file",
        "download_job",
        "download_schedule",
        "episode",
        "episode_ordering",
        "event_log",
        "external_id",
        "genre",
        "image_cache",
        "image_ref",
        "indexer",
        "job",
        "keyword",
        "library_root",
        "match_rule",
        "media_file",
        "media_item",
        "notification",
        "paired_device",
        "person",
        "playback_preference",
        "provider_cache",
        "quality_profile",
        "release_decision",
        "season",
        "secret_ref",
        "settings",
        "smart_collection",
        "subtitle_download",
        "subtitle_track",
        "title",
        "title_alias",
        "title_company",
        "title_genre",
        "title_keyword",
        "unmatched_file",
        "usenet_server",
        "usenet_setting",
        "watch_availability",
        "watch_offer",
        "watch_progress",
        "watch_sync",
        "watchlist_entry",
        "web_audit",
    ];

    /// `(name, declared type, notnull, default, pk)`, exactly as `PRAGMA table_info` reports it.
    type ColumnShape = (&'static str, &'static str, i64, Option<&'static str>, i64);

    // Migration 0001.
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

    // Migration 0002.
    const TITLE: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("kind", "TEXT", 1, None, 0),
        ("subtype", "TEXT", 1, Some("'standard'"), 0),
        ("name", "TEXT", 1, None, 0),
        ("original_name", "TEXT", 0, None, 0),
        ("sort_name", "TEXT", 1, None, 0),
        ("overview", "TEXT", 0, None, 0),
        ("tagline", "TEXT", 0, None, 0),
        ("status", "TEXT", 0, None, 0),
        ("release_date", "TEXT", 0, None, 0),
        ("first_air_date", "TEXT", 0, None, 0),
        ("last_air_date", "TEXT", 0, None, 0),
        ("runtime_minutes", "INTEGER", 0, None, 0),
        ("episode_runtime_minutes", "INTEGER", 0, None, 0),
        ("original_language", "TEXT", 0, None, 0),
        ("origin_country", "TEXT", 0, None, 0),
        ("content_rating", "TEXT", 0, None, 0),
        ("imdb_rating", "REAL", 0, None, 0),
        ("imdb_votes", "INTEGER", 0, None, 0),
        ("tmdb_rating", "REAL", 0, None, 0),
        ("tmdb_votes", "INTEGER", 0, None, 0),
        ("popularity", "REAL", 0, None, 0),
        ("adult", "INTEGER", 1, Some("0"), 0),
        ("in_production", "INTEGER", 1, Some("0"), 0),
        ("season_count", "INTEGER", 0, None, 0),
        ("episode_count", "INTEGER", 0, None, 0),
        ("poster_provider", "TEXT", 0, None, 0),
        ("poster_path", "TEXT", 0, None, 0),
        ("poster_blurhash", "TEXT", 0, None, 0),
        ("backdrop_provider", "TEXT", 0, None, 0),
        ("backdrop_path", "TEXT", 0, None, 0),
        ("backdrop_blurhash", "TEXT", 0, None, 0),
        ("logo_path", "TEXT", 0, None, 0),
        ("trailer_site", "TEXT", 0, None, 0),
        ("trailer_key", "TEXT", 0, None, 0),
        ("trailer_language", "TEXT", 0, None, 0),
        ("trailer_published_at", "TEXT", 0, None, 0),
        ("homepage", "TEXT", 0, None, 0),
        ("metadata_state", "TEXT", 1, Some("'stub'"), 0),
        ("last_synced_at", "INTEGER", 0, None, 0),
        ("created_at", "INTEGER", 1, None, 0),
        ("updated_at", "INTEGER", 1, None, 0),
        // Migration 0019.
        ("season_synced_at", "INTEGER", 0, None, 0),
    ];

    const TITLE_ALIAS: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("title_id", "INTEGER", 1, None, 0),
        ("alias", "TEXT", 1, None, 0),
        ("region", "TEXT", 0, None, 0),
        ("kind", "TEXT", 1, Some("'alternative'"), 0),
    ];

    const EXTERNAL_ID: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("title_id", "INTEGER", 0, None, 0),
        ("person_id", "INTEGER", 0, None, 0),
        ("season_id", "INTEGER", 0, None, 0),
        ("episode_id", "INTEGER", 0, None, 0),
        ("provider", "TEXT", 1, None, 0),
        ("value", "TEXT", 1, None, 0),
        ("is_primary", "INTEGER", 1, Some("0"), 0),
        ("fetched_at", "INTEGER", 1, None, 0),
    ];

    const GENRE: &[ColumnShape] = &[("id", "INTEGER", 0, None, 1), ("name", "TEXT", 1, None, 0)];

    const TITLE_GENRE: &[ColumnShape] = &[
        ("title_id", "INTEGER", 1, None, 1),
        ("genre_id", "INTEGER", 1, None, 2),
    ];

    const KEYWORD: &[ColumnShape] = &[("id", "INTEGER", 0, None, 1), ("name", "TEXT", 1, None, 0)];

    const TITLE_KEYWORD: &[ColumnShape] = &[
        ("title_id", "INTEGER", 1, None, 1),
        ("keyword_id", "INTEGER", 1, None, 2),
    ];

    const COMPANY: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("name", "TEXT", 1, None, 0),
        ("logo_path", "TEXT", 0, None, 0),
        ("provider", "TEXT", 0, None, 0),
        ("external_id", "TEXT", 0, None, 0),
    ];

    const TITLE_COMPANY: &[ColumnShape] = &[
        ("title_id", "INTEGER", 1, None, 1),
        ("company_id", "INTEGER", 1, None, 2),
        ("role", "TEXT", 1, None, 3),
    ];

    const PERSON: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("name", "TEXT", 1, None, 0),
        ("sort_name", "TEXT", 1, None, 0),
        ("known_for_department", "TEXT", 0, None, 0),
        ("profile_provider", "TEXT", 0, None, 0),
        ("profile_path", "TEXT", 0, None, 0),
        ("profile_blurhash", "TEXT", 0, None, 0),
        ("updated_at", "INTEGER", 0, None, 0),
        ("provider_id", "TEXT", 0, None, 0),
        ("profile_cache_path", "TEXT", 0, None, 0),
    ];

    const CREDIT: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("title_id", "INTEGER", 1, None, 0),
        ("episode_id", "INTEGER", 0, None, 0),
        ("person_id", "INTEGER", 1, None, 0),
        ("kind", "TEXT", 1, None, 0),
        ("character", "TEXT", 0, None, 0),
        ("job", "TEXT", 0, None, 0),
        ("department", "TEXT", 0, None, 0),
        ("order_index", "INTEGER", 0, None, 0),
        ("episode_count", "INTEGER", 0, None, 0),
    ];

    // Migration 0003.
    const SEASON: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("title_id", "INTEGER", 1, None, 0),
        ("season_number", "INTEGER", 1, None, 0),
        ("name", "TEXT", 0, None, 0),
        ("overview", "TEXT", 0, None, 0),
        ("air_date", "TEXT", 0, None, 0),
        ("poster_provider", "TEXT", 0, None, 0),
        ("poster_path", "TEXT", 0, None, 0),
        ("poster_blurhash", "TEXT", 0, None, 0),
        ("episode_count", "INTEGER", 0, None, 0),
        ("is_specials", "INTEGER", 1, Some("0"), 0),
        ("updated_at", "INTEGER", 1, None, 0),
    ];

    const EPISODE: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("title_id", "INTEGER", 1, None, 0),
        ("season_id", "INTEGER", 1, None, 0),
        ("season_number", "INTEGER", 1, None, 0),
        ("episode_number", "INTEGER", 1, None, 0),
        ("absolute_number", "INTEGER", 0, None, 0),
        ("name", "TEXT", 0, None, 0),
        ("overview", "TEXT", 0, None, 0),
        ("air_date", "TEXT", 0, None, 0),
        ("runtime_minutes", "INTEGER", 0, None, 0),
        ("still_provider", "TEXT", 0, None, 0),
        ("still_path", "TEXT", 0, None, 0),
        ("still_blurhash", "TEXT", 0, None, 0),
        ("vote_average", "REAL", 0, None, 0),
        ("vote_count", "INTEGER", 0, None, 0),
        ("episode_type", "TEXT", 0, None, 0),
        ("production_code", "TEXT", 0, None, 0),
        ("updated_at", "INTEGER", 1, None, 0),
    ];

    const EPISODE_ORDERING: &[ColumnShape] = &[
        ("episode_id", "INTEGER", 1, None, 1),
        ("scheme", "TEXT", 1, None, 2),
        ("season_number", "INTEGER", 1, None, 0),
        ("episode_number", "INTEGER", 1, None, 0),
    ];

    // Migration 0004.
    const USENET_SERVER: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("name", "TEXT", 1, None, 0),
        ("host", "TEXT", 1, None, 0),
        ("port", "INTEGER", 1, None, 0),
        ("tls", "INTEGER", 1, Some("1"), 0),
        ("username", "TEXT", 1, Some("''"), 0),
        ("password_secret", "TEXT", 1, None, 0),
        ("connections", "INTEGER", 1, Some("8"), 0),
        ("priority", "INTEGER", 1, Some("100"), 0),
        ("enabled", "INTEGER", 1, Some("0"), 0),
        ("health_state", "TEXT", 1, Some("'unknown'"), 0),
        ("last_check_at", "INTEGER", 0, None, 0),
        ("created_at", "INTEGER", 1, None, 0),
    ];

    const USENET_SETTING: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("allow_archives", "INTEGER", 1, Some("0"), 0),
        ("delete_archives_after_import", "INTEGER", 1, Some("1"), 0),
        ("updated_at", "INTEGER", 1, None, 0),
    ];

    const MEDIA_ITEM: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("title_id", "INTEGER", 1, None, 0),
        ("episode_id", "INTEGER", 0, None, 0),
        ("state", "TEXT", 1, None, 0),
        ("origin", "TEXT", 1, None, 0),
        ("preferred_file_id", "INTEGER", 0, None, 0),
        ("quality_label", "TEXT", 0, None, 0),
        ("created_at", "INTEGER", 1, None, 0),
        ("updated_at", "INTEGER", 1, None, 0),
    ];

    const MEDIA_FILE: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("media_item_id", "INTEGER", 1, None, 0),
        ("path", "TEXT", 1, None, 0),
        ("root_id", "INTEGER", 0, None, 0),
        ("size_bytes", "INTEGER", 1, None, 0),
        ("mtime_ms", "INTEGER", 1, None, 0),
        ("added_at", "INTEGER", 1, None, 0),
        ("missing_since", "INTEGER", 0, None, 0),
        ("container", "TEXT", 0, None, 0),
        ("duration_ms", "INTEGER", 0, None, 0),
        ("video_codec", "TEXT", 0, None, 0),
        ("video_profile", "TEXT", 0, None, 0),
        ("width", "INTEGER", 0, None, 0),
        ("height", "INTEGER", 0, None, 0),
        ("bit_depth", "INTEGER", 0, None, 0),
        ("hdr_format", "TEXT", 0, None, 0),
        ("frame_rate", "REAL", 0, None, 0),
        ("bitrate_kbps", "INTEGER", 0, None, 0),
        ("audio_codec", "TEXT", 0, None, 0),
        ("audio_channels", "INTEGER", 0, None, 0),
        ("audio_layout", "TEXT", 0, None, 0),
        ("audio_languages", "TEXT", 0, None, 0),
        ("subtitle_format", "TEXT", 0, None, 0),
        ("probe_version", "INTEGER", 0, None, 0),
        ("probe_json", "TEXT", 0, None, 0),
        ("quality_label", "TEXT", 0, None, 0),
        ("updated_at", "INTEGER", 1, None, 0),
    ];

    const LIBRARY_ROOT: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("path", "TEXT", 1, None, 0),
        ("kind", "TEXT", 1, None, 0),
        ("scan_mode", "TEXT", 1, Some("'incremental'"), 0),
        ("last_scan_at", "INTEGER", 0, None, 0),
        ("created_at", "INTEGER", 1, None, 0),
    ];

    const UNMATCHED_FILE: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("path", "TEXT", 1, None, 0),
        ("size_bytes", "INTEGER", 1, None, 0),
        ("mtime_ms", "INTEGER", 1, None, 0),
        ("parsed_title", "TEXT", 0, None, 0),
        ("parsed_year", "INTEGER", 0, None, 0),
        ("parsed_season", "INTEGER", 0, None, 0),
        ("parsed_episode", "INTEGER", 0, None, 0),
        ("confidence", "REAL", 0, None, 0),
        ("reason", "TEXT", 0, None, 0),
        ("suggested_title_id", "INTEGER", 0, None, 0),
        ("suggested_episode_id", "INTEGER", 0, None, 0),
        ("first_seen_at", "INTEGER", 1, None, 0),
    ];

    // Migration 0005.
    const SUBTITLE_TRACK: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("media_file_id", "INTEGER", 0, None, 0),
        ("episode_id", "INTEGER", 0, None, 0),
        ("origin", "TEXT", 1, None, 0),
        ("format", "TEXT", 1, None, 0),
        ("language", "TEXT", 1, None, 0),
        ("label", "TEXT", 0, None, 0),
        ("is_forced", "INTEGER", 1, Some("0"), 0),
        ("is_sdh", "INTEGER", 1, Some("0"), 0),
        ("is_default", "INTEGER", 1, Some("0"), 0),
        ("stream_index", "INTEGER", 0, None, 0),
        ("path", "TEXT", 0, None, 0),
        ("provider", "TEXT", 0, None, 0),
        ("provider_subtitle_id", "TEXT", 0, None, 0),
        ("provider_url", "TEXT", 0, None, 0),
        ("release_name", "TEXT", 0, None, 0),
        ("encoding", "TEXT", 0, None, 0),
        ("offset_ms", "INTEGER", 1, Some("0"), 0),
        ("hash_matched", "INTEGER", 1, Some("0"), 0),
        ("downloaded_at", "INTEGER", 0, None, 0),
        ("last_used_at", "INTEGER", 0, None, 0),
    ];

    const SUBTITLE_DOWNLOAD: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("media_file_id", "INTEGER", 0, None, 0),
        ("episode_id", "INTEGER", 0, None, 0),
        ("provider", "TEXT", 1, None, 0),
        ("provider_subtitle_id", "TEXT", 1, None, 0),
        ("language", "TEXT", 1, None, 0),
        ("score", "REAL", 1, None, 0),
        ("score_reasons", "TEXT", 0, None, 0),
        ("created_at", "INTEGER", 1, None, 0),
    ];

    const WATCH_PROGRESS: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("account_id", "INTEGER", 1, None, 0),
        ("title_id", "INTEGER", 1, None, 0),
        ("episode_id", "INTEGER", 0, None, 0),
        ("position_ms", "INTEGER", 1, Some("0"), 0),
        ("duration_ms", "INTEGER", 0, None, 0),
        ("percent", "REAL", 1, Some("0"), 0),
        ("watched", "INTEGER", 1, Some("0"), 0),
        ("watched_at", "INTEGER", 0, None, 0),
        ("play_count", "INTEGER", 1, Some("0"), 0),
        ("updated_at", "INTEGER", 1, None, 0),
        ("source", "TEXT", 1, Some("'local'"), 0),
        ("device_label", "TEXT", 0, None, 0),
    ];

    // Migration 0022.
    const WATCH_SYNC: &[ColumnShape] = &[
        // A single-column INTEGER PRIMARY KEY is SQLite's rowid alias, which PRAGMA reports as
        // nullable even though it never is.
        ("title_id", "INTEGER", 0, None, 1),
        ("synced_at", "INTEGER", 1, None, 0),
    ];

    const WATCH_AVAILABILITY: &[ColumnShape] = &[
        ("title_id", "INTEGER", 1, None, 1),
        ("region", "TEXT", 1, None, 2),
        ("source", "TEXT", 1, None, 0),
        ("link", "TEXT", 0, None, 0),
    ];

    const WATCH_OFFER: &[ColumnShape] = &[
        ("title_id", "INTEGER", 1, None, 1),
        ("region", "TEXT", 1, None, 2),
        ("category", "TEXT", 1, None, 3),
        ("provider_id", "INTEGER", 1, None, 4),
        ("provider_name", "TEXT", 1, None, 0),
        ("logo_path", "TEXT", 0, None, 0),
        ("display_priority", "INTEGER", 0, None, 0),
    ];

    const WATCHLIST_ENTRY: &[ColumnShape] = &[
        ("account_id", "INTEGER", 1, None, 1),
        ("title_id", "INTEGER", 1, None, 2),
        ("added_at", "INTEGER", 1, None, 0),
        ("position", "INTEGER", 0, None, 0),
        ("note", "TEXT", 0, None, 0),
        ("auto_acquire", "INTEGER", 1, Some("0"), 0),
    ];

    const SMART_COLLECTION: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("name", "TEXT", 1, None, 0),
        ("icon", "TEXT", 0, None, 0),
        ("query_json", "TEXT", 1, None, 0),
        ("sort", "TEXT", 1, None, 0),
        ("pinned", "INTEGER", 1, Some("0"), 0),
        ("created_at", "INTEGER", 1, None, 0),
    ];

    // Migration 0006.
    const QUALITY_PROFILE: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("name", "TEXT", 1, None, 0),
        ("is_default", "INTEGER", 1, Some("0"), 0),
        ("rules_json", "TEXT", 1, None, 0),
        ("cutoff_rank", "INTEGER", 1, None, 0),
        ("upgrade_allowed", "INTEGER", 1, Some("1"), 0),
        ("updated_at", "INTEGER", 1, None, 0),
    ];

    const INDEXER: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("kind", "TEXT", 1, None, 0),
        ("name", "TEXT", 1, None, 0),
        ("base_url", "TEXT", 0, None, 0),
        ("api_key_ref", "TEXT", 0, None, 0),
        ("enabled", "INTEGER", 1, Some("0"), 0),
        ("priority", "INTEGER", 1, Some("100"), 0),
        ("categories", "TEXT", 0, None, 0),
        ("rate_limit_json", "TEXT", 0, None, 0),
        ("health_state", "TEXT", 1, Some("'unknown'"), 0),
        ("last_check_at", "INTEGER", 0, None, 0),
    ];

    const DOWNLOAD_JOB: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("job_id", "INTEGER", 0, None, 0),
        ("title_id", "INTEGER", 1, None, 0),
        ("episode_id", "INTEGER", 0, None, 0),
        ("media_item_id", "INTEGER", 0, None, 0),
        ("quality_profile_id", "INTEGER", 0, None, 0),
        ("indexer_id", "INTEGER", 0, None, 0),
        ("state", "TEXT", 1, None, 0),
        ("priority", "INTEGER", 1, Some("100"), 0),
        ("is_upgrade", "INTEGER", 1, Some("0"), 0),
        ("release_title", "TEXT", 1, None, 0),
        ("release_source", "TEXT", 0, None, 0),
        ("release_group", "TEXT", 0, None, 0),
        ("resolution", "TEXT", 0, None, 0),
        ("video_codec", "TEXT", 0, None, 0),
        ("hdr_format", "TEXT", 0, None, 0),
        ("audio_summary", "TEXT", 0, None, 0),
        ("reported_size_bytes", "INTEGER", 0, None, 0),
        ("info_hash", "TEXT", 0, None, 0),
        ("magnet_uri", "TEXT", 0, None, 0),
        ("torrent_file_path", "TEXT", 0, None, 0),
        ("save_path", "TEXT", 1, None, 0),
        ("downloaded_bytes", "INTEGER", 1, Some("0"), 0),
        ("uploaded_bytes", "INTEGER", 1, Some("0"), 0),
        ("progress_permille", "INTEGER", 1, Some("0"), 0),
        ("download_rate_bps", "INTEGER", 1, Some("0"), 0),
        ("upload_rate_bps", "INTEGER", 1, Some("0"), 0),
        ("peers_connected", "INTEGER", 1, Some("0"), 0),
        ("peers_swarm", "INTEGER", 1, Some("0"), 0),
        ("eta_seconds", "INTEGER", 0, None, 0),
        ("ratio", "REAL", 0, None, 0),
        ("safety_report_json", "TEXT", 0, None, 0),
        ("engine_state_json", "TEXT", 0, None, 0),
        ("error_code", "TEXT", 0, None, 0),
        ("error_message", "TEXT", 0, None, 0),
        ("added_by", "TEXT", 1, Some("'user'"), 0),
        ("added_at", "INTEGER", 1, None, 0),
        ("started_at", "INTEGER", 0, None, 0),
        ("completed_at", "INTEGER", 0, None, 0),
        ("seeding_until", "INTEGER", 0, None, 0),
        ("removed_at", "INTEGER", 0, None, 0),
        ("protocol", "TEXT", 1, Some("'torrent'"), 0),
        ("nzb_sha256", "TEXT", 0, None, 0),
        ("nzb_path", "TEXT", 0, None, 0),
        ("stage", "TEXT", 1, Some("'transfer'"), 0),
        // Added by migration 0030, so the physical column comes after every original one.
        ("season_number", "INTEGER", 0, None, 0),
    ];

    const DOWNLOAD_FILE: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("download_job_id", "INTEGER", 1, None, 0),
        ("file_index", "INTEGER", 1, None, 0),
        ("path", "TEXT", 1, None, 0),
        ("size_bytes", "INTEGER", 1, None, 0),
        ("selected", "INTEGER", 1, Some("1"), 0),
        ("is_media", "INTEGER", 1, Some("0"), 0),
        ("blocked_reason", "TEXT", 0, None, 0),
    ];

    const RELEASE_DECISION: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("download_job_id", "INTEGER", 0, None, 0),
        ("title_id", "INTEGER", 1, None, 0),
        ("episode_id", "INTEGER", 0, None, 0),
        ("release_title", "TEXT", 1, None, 0),
        ("indexer", "TEXT", 0, None, 0),
        ("score", "REAL", 1, None, 0),
        ("rank_position", "INTEGER", 1, None, 0),
        ("chosen", "INTEGER", 1, Some("0"), 0),
        ("reasons_json", "TEXT", 1, None, 0),
        ("created_at", "INTEGER", 1, None, 0),
        // Migration 0017: the offer's source and the size/seeder figures the chooser shows, and
        // the dismissal the rail keeps a row for.
        ("source_json", "TEXT", 0, None, 0),
        ("size_bytes", "INTEGER", 0, None, 0),
        ("seeders", "INTEGER", 0, None, 0),
        ("dismissed", "INTEGER", 1, Some("0"), 0),
        ("protocol", "TEXT", 1, Some("'torrent'"), 0),
    ];

    // Migration 0007.
    const PROVIDER_CACHE: &[ColumnShape] = &[
        ("key", "TEXT", 0, None, 1),
        ("provider", "TEXT", 1, None, 0),
        ("resource", "TEXT", 1, None, 0),
        ("etag", "TEXT", 0, None, 0),
        ("last_modified", "TEXT", 0, None, 0),
        ("fetched_at", "INTEGER", 1, None, 0),
        ("expires_at", "INTEGER", 1, None, 0),
        ("schema_version", "INTEGER", 1, None, 0),
        ("body", "BLOB", 1, None, 0),
        ("size_bytes", "INTEGER", 1, None, 0),
    ];

    const IMAGE_CACHE: &[ColumnShape] = &[
        ("key", "TEXT", 0, None, 1),
        ("provider", "TEXT", 1, None, 0),
        ("path", "TEXT", 1, None, 0),
        ("variant", "TEXT", 1, None, 0),
        ("disk_path", "TEXT", 1, None, 0),
        ("bytes", "INTEGER", 1, None, 0),
        ("width", "INTEGER", 0, None, 0),
        ("height", "INTEGER", 0, None, 0),
        ("created_at", "INTEGER", 1, None, 0),
        ("last_used_at", "INTEGER", 1, None, 0),
        ("hits", "INTEGER", 1, Some("0"), 0),
        ("pinned", "INTEGER", 1, Some("0"), 0),
    ];

    const IMAGE_REF: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("owner_kind", "TEXT", 1, None, 0),
        ("owner_id", "INTEGER", 1, None, 0),
        ("role", "TEXT", 1, None, 0),
        ("provider", "TEXT", 1, None, 0),
        ("path", "TEXT", 1, None, 0),
        ("language", "TEXT", 1, Some("''"), 0),
        ("width", "INTEGER", 0, None, 0),
        ("height", "INTEGER", 0, None, 0),
        ("vote_average", "REAL", 0, None, 0),
        ("is_primary", "INTEGER", 1, Some("0"), 0),
        ("blurhash", "TEXT", 0, None, 0),
    ];

    const DATASET_STATE: &[ColumnShape] = &[
        ("name", "TEXT", 0, None, 1),
        ("version", "TEXT", 0, None, 0),
        ("etag", "TEXT", 0, None, 0),
        ("last_modified", "TEXT", 0, None, 0),
        ("rows", "INTEGER", 0, None, 0),
        ("size_bytes", "INTEGER", 0, None, 0),
        ("last_checked_at", "INTEGER", 0, None, 0),
        ("last_updated_at", "INTEGER", 0, None, 0),
        ("error", "TEXT", 0, None, 0),
    ];

    const NOTIFICATION: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("event", "TEXT", 1, None, 0),
        ("severity", "TEXT", 1, Some("'normal'"), 0),
        ("title", "TEXT", 1, None, 0),
        ("body", "TEXT", 0, None, 0),
        ("deep_link", "TEXT", 0, None, 0),
        ("subject_kind", "TEXT", 0, None, 0),
        ("subject_id", "INTEGER", 0, None, 0),
        ("dedupe_key", "TEXT", 0, None, 0),
        ("group_key", "TEXT", 0, None, 0),
        ("sound", "TEXT", 0, None, 0),
        ("delivery_state", "TEXT", 1, Some("'pending'"), 0),
        ("suppressed_reason", "TEXT", 0, None, 0),
        ("native_id", "INTEGER", 0, None, 0),
        ("delivered_at", "INTEGER", 0, None, 0),
        ("delivery_error", "TEXT", 0, None, 0),
        ("clicked_at", "INTEGER", 0, None, 0),
        ("created_at", "INTEGER", 1, None, 0),
        ("read_at", "INTEGER", 0, None, 0),
        ("dismissed_at", "INTEGER", 0, None, 0),
    ];

    const DEVICE: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("protocol", "TEXT", 1, None, 0),
        ("device_uid", "TEXT", 1, None, 0),
        ("name", "TEXT", 1, None, 0),
        ("model", "TEXT", 0, None, 0),
        ("address", "TEXT", 0, None, 0),
        ("port", "INTEGER", 0, None, 0),
        ("capabilities_json", "TEXT", 0, None, 0),
        ("trusted", "INTEGER", 1, Some("0"), 0),
        ("last_seen_at", "INTEGER", 1, None, 0),
    ];

    const CAST_SESSION: &[ColumnShape] = &[
        ("id", "INTEGER", 0, None, 1),
        ("device_id", "INTEGER", 1, None, 0),
        ("media_file_id", "INTEGER", 0, None, 0),
        ("episode_id", "INTEGER", 0, None, 0),
        ("state", "TEXT", 1, None, 0),
        ("stream_url", "TEXT", 0, None, 0),
        ("delivery", "TEXT", 1, None, 0),
        ("ffmpeg_args", "TEXT", 0, None, 0),
        ("position_ms", "INTEGER", 1, Some("0"), 0),
        ("started_at", "INTEGER", 1, None, 0),
        ("ended_at", "INTEGER", 0, None, 0),
        ("error_message", "TEXT", 0, None, 0),
    ];

    // Migration 0015.
    const DISCOVER_RAIL: &[ColumnShape] = &[
        ("rail", "TEXT", 1, None, 1),
        ("position", "INTEGER", 1, None, 2),
        ("title_id", "INTEGER", 1, None, 0),
    ];

    /// Every documented table with the column shape its migration must produce.
    const DOCUMENTED_COLUMNS: [(&str, &[ColumnShape]); 46] = [
        ("account", ACCOUNT),
        ("cast_session", CAST_SESSION),
        ("company", COMPANY),
        ("credit", CREDIT),
        ("dataset_state", DATASET_STATE),
        ("device", DEVICE),
        ("discover_rail", DISCOVER_RAIL),
        ("download_file", DOWNLOAD_FILE),
        ("download_job", DOWNLOAD_JOB),
        ("episode", EPISODE),
        ("episode_ordering", EPISODE_ORDERING),
        ("event_log", EVENT_LOG),
        ("external_id", EXTERNAL_ID),
        ("genre", GENRE),
        ("image_cache", IMAGE_CACHE),
        ("image_ref", IMAGE_REF),
        ("indexer", INDEXER),
        ("job", JOB),
        ("keyword", KEYWORD),
        ("library_root", LIBRARY_ROOT),
        ("media_file", MEDIA_FILE),
        ("media_item", MEDIA_ITEM),
        ("notification", NOTIFICATION),
        ("person", PERSON),
        ("provider_cache", PROVIDER_CACHE),
        ("quality_profile", QUALITY_PROFILE),
        ("release_decision", RELEASE_DECISION),
        ("season", SEASON),
        ("secret_ref", SECRET_REF),
        ("usenet_server", USENET_SERVER),
        ("usenet_setting", USENET_SETTING),
        ("settings", SETTINGS),
        ("smart_collection", SMART_COLLECTION),
        ("subtitle_download", SUBTITLE_DOWNLOAD),
        ("subtitle_track", SUBTITLE_TRACK),
        ("title", TITLE),
        ("title_alias", TITLE_ALIAS),
        ("title_company", TITLE_COMPANY),
        ("title_genre", TITLE_GENRE),
        ("title_keyword", TITLE_KEYWORD),
        ("unmatched_file", UNMATCHED_FILE),
        ("watch_availability", WATCH_AVAILABILITY),
        ("watch_offer", WATCH_OFFER),
        ("watch_progress", WATCH_PROGRESS),
        ("watch_sync", WATCH_SYNC),
        ("watchlist_entry", WATCHLIST_ENTRY),
    ];

    /// `(child, parent, child column, parent column, ON DELETE)`, per `DATA_MODEL` §2.
    const DOCUMENTED_FOREIGN_KEYS: &[(&str, &str, &str, &str, &str)] = &[
        ("cast_session", "device", "device_id", "id", "CASCADE"),
        ("cast_session", "episode", "episode_id", "id", "SET NULL"),
        (
            "cast_session",
            "media_file",
            "media_file_id",
            "id",
            "SET NULL",
        ),
        ("credit", "episode", "episode_id", "id", "CASCADE"),
        ("credit", "person", "person_id", "id", "CASCADE"),
        ("credit", "title", "title_id", "id", "CASCADE"),
        ("discover_rail", "title", "title_id", "id", "CASCADE"),
        (
            "download_file",
            "download_job",
            "download_job_id",
            "id",
            "CASCADE",
        ),
        ("download_job", "episode", "episode_id", "id", "CASCADE"),
        ("download_job", "indexer", "indexer_id", "id", "SET NULL"),
        ("download_job", "job", "job_id", "id", "SET NULL"),
        (
            "download_job",
            "media_item",
            "media_item_id",
            "id",
            "SET NULL",
        ),
        (
            "download_job",
            "quality_profile",
            "quality_profile_id",
            "id",
            "SET NULL",
        ),
        ("download_job", "title", "title_id", "id", "CASCADE"),
        (
            "download_schedule",
            "account",
            "account_id",
            "id",
            "CASCADE",
        ),
        (
            "download_schedule",
            "episode",
            "episode_id",
            "id",
            "CASCADE",
        ),
        ("download_schedule", "title", "title_id", "id", "CASCADE"),
        ("episode", "season", "season_id", "id", "CASCADE"),
        ("episode", "title", "title_id", "id", "CASCADE"),
        ("episode_ordering", "episode", "episode_id", "id", "CASCADE"),
        ("external_id", "episode", "episode_id", "id", "CASCADE"),
        ("external_id", "person", "person_id", "id", "CASCADE"),
        ("external_id", "season", "season_id", "id", "CASCADE"),
        ("external_id", "title", "title_id", "id", "CASCADE"),
        ("indexer", "secret_ref", "api_key_ref", "key", "SET NULL"),
        ("match_rule", "title", "title_id", "id", "CASCADE"),
        ("media_file", "library_root", "root_id", "id", "NO ACTION"),
        ("media_file", "media_item", "media_item_id", "id", "CASCADE"),
        ("media_item", "episode", "episode_id", "id", "CASCADE"),
        ("media_item", "title", "title_id", "id", "CASCADE"),
        (
            "release_decision",
            "download_job",
            "download_job_id",
            "id",
            "CASCADE",
        ),
        ("release_decision", "episode", "episode_id", "id", "CASCADE"),
        ("release_decision", "title", "title_id", "id", "CASCADE"),
        ("season", "title", "title_id", "id", "CASCADE"),
        (
            "subtitle_download",
            "episode",
            "episode_id",
            "id",
            "CASCADE",
        ),
        (
            "subtitle_download",
            "media_file",
            "media_file_id",
            "id",
            "CASCADE",
        ),
        ("subtitle_track", "episode", "episode_id", "id", "CASCADE"),
        (
            "subtitle_track",
            "media_file",
            "media_file_id",
            "id",
            "CASCADE",
        ),
        ("title_alias", "title", "title_id", "id", "CASCADE"),
        ("title_company", "company", "company_id", "id", "CASCADE"),
        ("title_company", "title", "title_id", "id", "CASCADE"),
        ("title_genre", "genre", "genre_id", "id", "CASCADE"),
        ("title_genre", "title", "title_id", "id", "CASCADE"),
        ("title_keyword", "keyword", "keyword_id", "id", "CASCADE"),
        ("title_keyword", "title", "title_id", "id", "CASCADE"),
        (
            "unmatched_file",
            "episode",
            "suggested_episode_id",
            "id",
            "NO ACTION",
        ),
        (
            "unmatched_file",
            "title",
            "suggested_title_id",
            "id",
            "NO ACTION",
        ),
        ("watch_availability", "title", "title_id", "id", "CASCADE"),
        (
            "watch_offer",
            "watch_availability",
            "region",
            "region",
            "CASCADE",
        ),
        (
            "watch_offer",
            "watch_availability",
            "title_id",
            "title_id",
            "CASCADE",
        ),
        ("watch_sync", "title", "title_id", "id", "CASCADE"),
        ("watch_progress", "account", "account_id", "id", "CASCADE"),
        ("watch_progress", "episode", "episode_id", "id", "CASCADE"),
        ("watch_progress", "title", "title_id", "id", "CASCADE"),
        ("watchlist_entry", "account", "account_id", "id", "CASCADE"),
        ("watchlist_entry", "title", "title_id", "id", "CASCADE"),
        ("web_audit", "paired_device", "device_id", "id", "NO ACTION"),
    ];

    /// Every named index the migrations create.
    const DOCUMENTED_INDICES: [&str; 48] = [
        "credit_person_idx",
        "credit_title_order_idx",
        "discover_rail_title_idx",
        "download_job_active_idx",
        "download_job_hash_uq",
        "download_job_nzb_uq",
        "download_job_title_idx",
        "download_schedule_due_idx",
        "episode_absolute_idx",
        "episode_air_date_idx",
        "episode_ordering_lookup_idx",
        "event_log_at_idx",
        "external_id_episode_idx",
        "external_id_title_idx",
        "image_cache_eviction_idx",
        "image_cache_lru_idx",
        "job_dedupe_uq",
        "job_ready_idx",
        "media_file_item_idx",
        "media_file_missing_idx",
        "media_file_usage_idx",
        "media_item_episode_uq",
        "media_item_movie_uq",
        "notification_centre_idx",
        "notification_dedupe_idx",
        "notification_pending_idx",
        "notification_unread_idx",
        "person_name_idx",
        "person_provider_idx",
        "provider_cache_expiry_idx",
        "provider_cache_resource_idx",
        "provider_cache_size_idx",
        "release_decision_title_idx",
        "subtitle_download_uq",
        "subtitle_track_episode_idx",
        "subtitle_track_file_idx",
        "title_alias_alias_idx",
        "title_imdb_rating_idx",
        "title_kind_popularity_idx",
        "title_kind_release_idx",
        "title_sort_name_idx",
        "title_sync_idx",
        "usenet_server_enabled_idx",
        "watch_offer_title_idx",
        "watch_progress_account_idx",
        "watch_progress_continue_idx",
        "watchlist_added_idx",
        "web_audit_device_created",
    ];

    /// The part of each named index's `CREATE` statement that makes it correct: the
    /// columns and ordering, and for a partial index the `WHERE` clause. Split from
    /// [`DOCUMENTED_INDICES`] so a regression in the predicate is not hidden by the name.
    const DOCUMENTED_INDEX_FRAGMENTS: &[(&str, &str)] = &[
        (
            "job_dedupe_uq",
            "UNIQUE INDEX job_dedupe_uq ON job (kind, dedupe_key)",
        ),
        (
            "job_dedupe_uq",
            "WHERE dedupe_key IS NOT NULL AND state IN ('queued','running','deferred')",
        ),
        ("job_ready_idx", "(state, priority, next_attempt_at)"),
        ("event_log_at_idx", "(at DESC)"),
        ("title_kind_popularity_idx", "(kind, popularity DESC)"),
        (
            "title_kind_release_idx",
            "(kind, release_date DESC, first_air_date DESC)",
        ),
        (
            "title_imdb_rating_idx",
            "(imdb_rating DESC) WHERE imdb_rating IS NOT NULL",
        ),
        ("title_sort_name_idx", "(sort_name)"),
        ("title_sync_idx", "(metadata_state, last_synced_at)"),
        ("watch_offer_title_idx", "(title_id, region)"),
        ("web_audit_device_created", "(device_id, created_at)"),
        ("title_alias_alias_idx", "(alias)"),
        ("external_id_title_idx", "(title_id)"),
        ("external_id_episode_idx", "(episode_id)"),
        ("person_name_idx", "(name)"),
        ("credit_title_order_idx", "(title_id, kind, order_index)"),
        ("credit_person_idx", "(person_id)"),
        ("episode_air_date_idx", "(title_id, air_date)"),
        ("episode_absolute_idx", "(title_id, absolute_number)"),
        (
            "episode_ordering_lookup_idx",
            "(scheme, season_number, episode_number)",
        ),
        ("media_item_movie_uq", "UNIQUE INDEX media_item_movie_uq"),
        ("media_item_movie_uq", "(title_id) WHERE episode_id IS NULL"),
        (
            "media_item_episode_uq",
            "UNIQUE INDEX media_item_episode_uq",
        ),
        (
            "media_item_episode_uq",
            "(episode_id) WHERE episode_id IS NOT NULL",
        ),
        ("media_file_item_idx", "(media_item_id)"),
        (
            "media_file_missing_idx",
            "(missing_since) WHERE missing_since IS NOT NULL",
        ),
        (
            "media_file_usage_idx",
            "(root_id, quality_label, media_item_id, size_bytes)",
        ),
        ("media_file_usage_idx", "WHERE missing_since IS NULL"),
        ("subtitle_track_file_idx", "(media_file_id, language)"),
        ("subtitle_track_episode_idx", "(episode_id, language)"),
        ("subtitle_download_uq", "UNIQUE INDEX subtitle_download_uq"),
        (
            "subtitle_download_uq",
            "COALESCE(media_file_id, -1), COALESCE(episode_id, -1)",
        ),
        (
            "watch_progress_account_idx",
            "INDEX watch_progress_account_idx ON watch_progress (account_id, updated_at DESC)",
        ),
        (
            "watch_progress_continue_idx",
            "(account_id, updated_at DESC)",
        ),
        (
            "watch_progress_continue_idx",
            "WHERE watched = 0 AND percent BETWEEN 0.02 AND 0.95",
        ),
        ("watchlist_added_idx", "(account_id, added_at DESC)"),
        ("download_job_active_idx", "(state, priority)"),
        (
            "download_job_active_idx",
            "WHERE state IN ('queued','inspecting','metadata','downloading','paused')",
        ),
        ("download_job_title_idx", "(title_id, episode_id)"),
        ("download_job_hash_uq", "UNIQUE INDEX download_job_hash_uq"),
        ("download_job_hash_uq", "WHERE info_hash IS NOT NULL"),
        ("download_job_hash_uq", "'seeding'"),
        ("download_job_nzb_uq", "UNIQUE INDEX download_job_nzb_uq"),
        ("download_job_nzb_uq", "WHERE nzb_sha256 IS NOT NULL"),
        ("download_job_nzb_uq", "'seeding'"),
        ("usenet_server_enabled_idx", "(enabled, priority)"),
        ("release_decision_title_idx", "(title_id, created_at DESC)"),
        ("provider_cache_expiry_idx", "(expires_at)"),
        ("provider_cache_resource_idx", "(provider, resource)"),
        ("provider_cache_size_idx", "(size_bytes)"),
        (
            "image_cache_eviction_idx",
            "(last_used_at, key) WHERE pinned = 0",
        ),
        ("image_cache_lru_idx", "(last_used_at)"),
        (
            "notification_centre_idx",
            "(created_at DESC) WHERE dismissed_at IS NULL",
        ),
        (
            "notification_unread_idx",
            "(created_at DESC) WHERE read_at IS NULL",
        ),
        ("notification_dedupe_idx", "(dedupe_key, created_at DESC)"),
        ("notification_dedupe_idx", "WHERE dedupe_key IS NOT NULL"),
        (
            "notification_pending_idx",
            "(delivery_state) WHERE delivery_state = 'pending'",
        ),
        ("discover_rail_title_idx", "(title_id)"),
    ];

    /// The file every fixture in this module keeps its database in.
    fn database_path(directory: &TempDir) -> std::path::PathBuf {
        directory.path().join("peregrine.sqlite")
    }

    /// A connection with every embedded migration applied, plus the resources it needs to live.
    async fn migrated(directory: &TempDir) -> SqliteConnection {
        let mut connection = open_writer_connection(&database_path(directory))
            .await
            .unwrap();
        apply(&mut connection, &database_path(directory))
            .await
            .unwrap();
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

    /// Runs one statement that needs no binds, which is all the fixture lists use.
    ///
    /// The statement is `&'static str` because `sqlx` refuses a query string it cannot see
    /// as a literal: interpolating an identifier into SQL is how injection starts, and the
    /// fixture lists are consts of literals so the guard costs nothing here.
    async fn statement(
        connection: &mut SqliteConnection,
        sql: &'static str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(sql).execute(&mut *connection).await.map(|_| ())
    }

    /// Runs one statement the test built from its own literal table.
    ///
    /// Separate from [`statement`] only in lifetime, and the `AssertSqlSafe` is the audit
    /// that difference needs: every value interpolated here comes from a `const` list of
    /// literals in this module, never from a caller or a fixture file, so there is nothing
    /// for an injection to reach.
    async fn literal_statement(
        connection: &mut SqliteConnection,
        sql: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .execute(&mut *connection)
            .await
            .map(|_| ())
    }

    /// Writes every statement, failing the test on the first refused one.
    async fn write_all(connection: &mut SqliteConnection, statements: &[&'static str]) {
        for sql in statements {
            statement(connection, sql)
                .await
                .unwrap_or_else(|error| panic!("fixture must be accepted: {sql}: {error}"));
        }
    }

    /// Asserts that every statement in `rejects` fails a `CHECK` constraint while every one
    /// in `accepts` is stored. The error has to name a `CHECK`, so a unique-constraint
    /// failure caused by a careless fixture cannot masquerade as the constraint working.
    async fn assert_check_behaviour(
        connection: &mut SqliteConnection,
        rejects: &[&'static str],
        accepts: &[&'static str],
    ) {
        write_all(connection, accepts).await;
        for sql in rejects {
            let error = statement(connection, sql)
                .await
                .expect_err(&format!("must be rejected: {sql}"));
            assert!(
                error.to_string().contains("CHECK"),
                "expected a CHECK failure, got {error} for {sql}"
            );
        }
    }

    /// `(parent table, child column, parent column, ON DELETE)` for one table's FKs.
    async fn foreign_keys(
        connection: &mut SqliteConnection,
        table: &str,
    ) -> Vec<(String, String, String, String)> {
        sqlx::query_as(
            "SELECT \"table\", \"from\", \"to\", on_delete \
             FROM pragma_foreign_key_list(?1) ORDER BY id, seq",
        )
        .bind(table)
        .fetch_all(&mut *connection)
        .await
        .unwrap()
    }

    /// The tables the migrations declare, which is every table except the derived FTS tier.
    async fn documented_tables(connection: &mut SqliteConnection) -> Vec<String> {
        names(
            connection,
            "SELECT name FROM sqlite_master WHERE type = 'table' \
             AND name NOT LIKE 'sqlite_%' AND name <> '_sqlx_migrations' ORDER BY name",
        )
        .await
        .into_iter()
        .filter(|name| !FTS_TIER.contains(&name.as_str()))
        .collect()
    }

    /// The whole schema except the disposable cache tier, so a test can prove the tier's
    /// removal changed nothing else.
    async fn schema_without_cache(
        connection: &mut SqliteConnection,
    ) -> Vec<(String, String, String)> {
        sqlx::query_as(
            "SELECT type, name, COALESCE(sql, '') FROM sqlite_master \
             WHERE name NOT LIKE 'sqlite_%' \
             AND tbl_name NOT IN ('provider_cache','image_cache') ORDER BY type, name",
        )
        .fetch_all(&mut *connection)
        .await
        .unwrap()
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
        let applied = apply(&mut connection, &path).await.unwrap();

        assert_eq!(
            applied,
            super::count(),
            "one migration per embedded file, whatever the set holds"
        );
        assert_eq!(
            documented_tables(&mut connection).await,
            DOCUMENTED_TABLES,
            "the search index is excluded: it is derived, and asserted on its own below"
        );
        assert_eq!(
            versions(&mut connection).await,
            (1..=i64::try_from(super::count()).unwrap()).collect::<Vec<_>>(),
            "every applied migration must be recorded, in order"
        );
    }

    /// PG-E3-12: the index is a contentless FTS5 table that accepts a per-row delete, and
    /// `SQLite` creates the shadow tables it needs beside it.
    ///
    /// The two options are asserted by name rather than by behaviour because they are what
    /// `DATA_MODEL` §2.9 records, and a rebuild of this schema must produce the same table.
    /// `contentless_delete=1` is the load-bearing one: without it `SQLite` refuses
    /// `DELETE FROM title_fts` outright, and the maintenance path has no way to remove a
    /// title from the index short of rebuilding all of it.
    #[tokio::test]
    async fn the_search_index_is_a_contentless_fts5_table() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated(&directory).await;

        let sql: String = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'title_fts'",
        )
        .fetch_one(&mut connection)
        .await
        .expect("the index is declared by the embedded migrations");

        assert!(
            sql.to_uppercase().contains("FTS5"),
            "the index must be an FTS5 table: {sql}"
        );
        for option in ["content=''", "contentless_delete=1"] {
            assert!(
                sql.to_lowercase().contains(option),
                "`title_fts` must declare `{option}`: {sql}"
            );
        }
        assert!(
            !sql.contains("content='title'"),
            "an external-content table would keep a second copy of the corpus: {sql}"
        );

        // `SQLite` creates these itself; a migration that named them would break the moment
        // the module's internal layout changed. The set is asserted whole, so the tier the
        // table-set assertion filters is exactly what is in the database.
        let mut expected = FTS_TIER.to_vec();
        expected.sort_unstable();
        assert_eq!(
            names(
                &mut connection,
                "SELECT name FROM sqlite_master WHERE type = 'table' \
                 AND name LIKE 'title\\_fts%' ESCAPE '\\' ORDER BY name",
            )
            .await,
            expected,
            "the index and the shadow tables `SQLite` keeps beside it, and nothing else"
        );
    }

    #[tokio::test]
    async fn every_documented_column_is_present_with_its_declared_shape() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated(&directory).await;

        for (table, shape) in DOCUMENTED_COLUMNS {
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
            DOCUMENTED_INDICES.to_vec()
        );

        let index_sql: Vec<(String, String)> = sqlx::query_as(
            "SELECT name, sql FROM sqlite_master WHERE type = 'index' AND sql IS NOT NULL \
             ORDER BY name",
        )
        .fetch_all(&mut connection)
        .await
        .unwrap();

        // Each documented index is checked for the columns, ordering, and `WHERE` clause
        // that make it correct. An index that lost its predicate or a `DESC` would still
        // be listed here and would still satisfy the name check above.
        for (index, fragment) in DOCUMENTED_INDEX_FRAGMENTS.iter().copied() {
            let sql = &index_sql
                .iter()
                .find(|(name, _)| name == index)
                .unwrap_or_else(|| panic!("`{index}` is not in sqlite_master"))
                .1;
            assert!(
                sql.contains(fragment),
                "`{index}` must contain {fragment:?}: {sql}"
            );
        }

        // `download_job_hash_uq` is unique among live jobs only: history may repeat, so a
        // terminal row must not block a re-download of the same release (PG-E3-19).
        let hash = &index_sql
            .iter()
            .find(|(name, _)| name == "download_job_hash_uq")
            .unwrap()
            .1;
        assert!(
            hash.to_uppercase().contains("UNIQUE"),
            "`download_job_hash_uq` must be unique: {hash}"
        );
        for terminal in ["'completed'", "'failed'", "'blocked'", "'removed'"] {
            assert!(
                !hash.contains(terminal),
                "`{terminal}` is terminal, so it must not be in the live set: {hash}"
            );
        }
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

    /// The parent rows every constraint test may rely on, with explicit ids so an assertion
    /// can name them: one adult account, a movie and a show, one season and episode, a
    /// person and a company, a library root, one movie media item, and one cast device.
    const BASE_FIXTURES: &[&str] = &[
        "INSERT OR REPLACE INTO account (id, name, kind, created_at) VALUES (1, 'Ada', 'adult', 0)",
        "INSERT INTO title (id, kind, name, sort_name, created_at, updated_at) \
         VALUES (1, 'movie', 'Alien', 'alien', 0, 0)",
        "INSERT INTO title (id, kind, name, sort_name, created_at, updated_at) \
         VALUES (2, 'show', 'The Wire', 'wire', 0, 0)",
        "INSERT INTO season (id, title_id, season_number, updated_at) VALUES (1, 1, 1, 0)",
        "INSERT INTO episode (id, title_id, season_id, season_number, episode_number, updated_at) \
         VALUES (1, 1, 1, 1, 1, 0)",
        "INSERT INTO person (id, name, sort_name) VALUES (1, 'Sigourney Weaver', 'weaver')",
        "INSERT INTO company (id, name, provider) VALUES (1, 'Brandywine', 'tmdb')",
        "INSERT INTO library_root (id, path, kind, created_at) \
         VALUES (1, '/media/movies', 'movies', 0)",
        "INSERT INTO media_item (id, title_id, state, origin, created_at, updated_at) \
         VALUES (1, 1, 'available', 'scan', 0, 0)",
        "INSERT INTO device (id, protocol, device_uid, name, last_seen_at) \
         VALUES (1, 'dlna', 'uuid:1', 'Living room TV', 0)",
    ];

    /// One unknown value per documented enumeration `CHECK`. Every row would otherwise be
    /// valid, so the only thing that can refuse it is the `CHECK`.
    const ENUM_REJECTS: &[&str] = &[
        "INSERT INTO account (name, kind, created_at) VALUES ('Teen', 'teen', 0)",
        "INSERT INTO job (kind, state, payload_json, created_at) \
         VALUES ('metadata.refresh', 'halfway', '{}', 0)",
        "INSERT INTO title (id, kind, name, sort_name, created_at, updated_at) \
         VALUES (3, 'trailer', 'X', 'x', 0, 0)",
        "INSERT INTO title (id, kind, name, sort_name, metadata_state, created_at, updated_at) \
         VALUES (4, 'movie', 'X', 'x', 'unknown', 0, 0)",
        "INSERT INTO title_alias (title_id, alias, region, kind) VALUES (1, 'Alien', 'US', 'variant')",
        "INSERT INTO external_id (title_id, provider, value, fetched_at) \
         VALUES (1, 'letterboxd', 'x', 0)",
        "INSERT INTO external_id (provider, value, fetched_at) VALUES ('imdb', 'x', 0)",
        "INSERT INTO credit (title_id, person_id, kind, job, character) \
         VALUES (1, 1, 'writer', 'writer', 'x')",
        "INSERT INTO title_company (title_id, company_id, role) VALUES (1, 1, 'label')",
        "INSERT INTO episode (title_id, season_id, season_number, episode_number, episode_type, updated_at) \
         VALUES (1, 1, 1, 2, 'filler', 0)",
        "INSERT INTO library_root (path, kind, created_at) VALUES ('/media/music', 'music', 0)",
        "INSERT INTO library_root (path, kind, scan_mode, created_at) \
         VALUES ('/media/other', 'mixed', 'eager', 0)",
        "INSERT INTO media_item (title_id, state, origin, created_at, updated_at) \
         VALUES (2, 'done', 'scan', 0, 0)",
        "INSERT INTO media_item (title_id, state, origin, created_at, updated_at) \
         VALUES (2, 'wanted', 'import', 0, 0)",
        "INSERT INTO media_file (media_item_id, path, size_bytes, mtime_ms, added_at, hdr_format, updated_at) \
         VALUES (1, '/media/bad.mkv', 1, 0, 0, 'hdr12', 0)",
        "INSERT INTO subtitle_track (episode_id, origin, format, language) \
         VALUES (1, 'ocr', 'srt', 'en')",
        "INSERT INTO subtitle_track (episode_id, origin, format, language) \
         VALUES (1, 'external', 'txt', 'en')",
        "INSERT INTO subtitle_track (origin, format, language) VALUES ('external', 'srt', 'en')",
        "INSERT INTO watch_progress (account_id, title_id, position_ms, updated_at, source) \
         VALUES (1, 2, 0, 0, 'plex')",
        "INSERT INTO indexer (kind, name) VALUES ('sabnzbd', 'Indexer')",
        "INSERT INTO indexer (kind, name, health_state) VALUES ('rss', 'Other', 'sick')",
        "INSERT INTO image_ref (owner_kind, owner_id, role, provider, path, language) \
         VALUES ('collection', 1, 'poster', 'tmdb', '/a.jpg', 'en')",
        "INSERT INTO image_ref (owner_kind, owner_id, role, provider, path, language) \
         VALUES ('title', 1, 'thumbnail', 'tmdb', '/b.jpg', 'en')",
        "INSERT INTO notification (event, severity, title, created_at) \
         VALUES ('disk.low', 'urgent', 'Disk low', 0)",
        "INSERT INTO notification (event, title, delivery_state, created_at) \
         VALUES ('disk.low', 'Disk low', 'dropped', 0)",
        "INSERT INTO notification (event, title, suppressed_reason, created_at) \
         VALUES ('disk.low', 'Disk low', 'because', 0)",
        "INSERT INTO notification (event, title, subject_kind, created_at) \
         VALUES ('disk.low', 'Disk low', 'movie', 0)",
        "INSERT INTO device (protocol, device_uid, name, last_seen_at) \
         VALUES ('airplay', 'x', 'X', 0)",
        "INSERT INTO download_job (title_id, state, release_title, save_path, added_at) \
         VALUES (1, 'zombie', 'Alien', '/downloads', 0)",
        "INSERT INTO cast_session (device_id, state, delivery, started_at) \
         VALUES (1, 'zapping', 'direct', 0)",
        "INSERT INTO cast_session (device_id, state, delivery, started_at) \
         VALUES (1, 'playing', 'dash', 0)",
    ];

    /// One row per enumeration whose recorded values must be accepted.
    const ENUM_ACCEPTS: &[&str] = &[
        "INSERT INTO account (name, kind, created_at) VALUES ('Adult', 'adult', 0)",
        "INSERT INTO title_alias (title_id, alias, region, kind) \
         VALUES (1, 'Alien (working)', 'US', 'working')",
        "INSERT INTO external_id (title_id, provider, value, fetched_at) \
         VALUES (1, 'imdb', 'tt0078748', 0)",
        "INSERT INTO credit (title_id, person_id, kind, job, character) \
         VALUES (1, 1, 'cast', 'Actor', 'Ripley')",
        "INSERT INTO title_company (title_id, company_id, role) VALUES (1, 1, 'network')",
        "INSERT INTO episode (title_id, season_id, season_number, episode_number, episode_type, updated_at) \
         VALUES (1, 1, 1, 3, 'finale', 0)",
        "INSERT INTO media_item (title_id, episode_id, state, origin, created_at, updated_at) \
         VALUES (1, 1, 'available', 'scan', 0, 0)",
        "INSERT INTO media_file (media_item_id, path, size_bytes, mtime_ms, added_at, hdr_format, updated_at) \
         VALUES (1, '/media/alien.mkv', 1, 0, 0, 'hdr10', 0)",
        "INSERT INTO subtitle_track (episode_id, origin, format, language) \
         VALUES (1, 'external', 'srt', 'en')",
        "INSERT INTO watch_progress (account_id, title_id, position_ms, updated_at, source) \
         VALUES (1, 1, 0, 0, 'manual')",
        "INSERT INTO indexer (kind, name, health_state) VALUES ('torznab', 'Indexer', 'ok')",
        "INSERT INTO image_ref (owner_kind, owner_id, role, provider, path, language) \
         VALUES ('title', 1, 'poster', 'tmdb', '/poster.jpg', 'en')",
        "INSERT INTO notification (event, severity, title, delivery_state, created_at) \
         VALUES ('disk.low', 'critical', 'Disk low', 'delivered', 0)",
        "INSERT INTO download_job (title_id, state, release_title, save_path, added_at) \
         VALUES (1, 'downloading', 'Alien 1979 1080p', '/downloads', 0)",
        "INSERT INTO cast_session (device_id, state, delivery, started_at) \
         VALUES (1, 'playing', 'direct', 0)",
    ];

    /// `(column, statement)`: each statement sets a documented boolean to 2.
    const BOOLEAN_REJECTS: &[(&str, &str)] = &[
        (
            "account.can_cast",
            "INSERT INTO account (name, kind, can_cast, created_at) VALUES ('B', 'adult', 2, 0)",
        ),
        (
            "season.is_specials",
            "INSERT INTO season (title_id, season_number, is_specials, updated_at) VALUES (1, 9, 2, 0)",
        ),
        (
            "subtitle_track.is_forced",
            "INSERT INTO subtitle_track (episode_id, origin, format, language, is_forced) \
             VALUES (1, 'external', 'srt', 'en', 2)",
        ),
        (
            "watch_progress.watched",
            "INSERT INTO watch_progress (account_id, title_id, position_ms, updated_at, watched) \
             VALUES (1, 1, 0, 0, 2)",
        ),
        (
            "quality_profile.is_default",
            "INSERT INTO quality_profile (name, rules_json, cutoff_rank, updated_at, is_default) \
             VALUES ('Any', '{}', 0, 0, 2)",
        ),
        (
            "indexer.enabled",
            "INSERT INTO indexer (kind, name, enabled) VALUES ('rss', 'R', 2)",
        ),
        (
            "download_job.is_upgrade",
            "INSERT INTO download_job (title_id, state, release_title, save_path, added_at, is_upgrade) \
             VALUES (1, 'queued', 'Alien', '/downloads', 0, 2)",
        ),
        (
            "image_ref.is_primary",
            "INSERT INTO image_ref (owner_kind, owner_id, role, provider, path, is_primary) \
             VALUES ('title', 1, 'poster', 'tmdb', '/p.jpg', 2)",
        ),
        (
            "device.trusted",
            "INSERT INTO device (protocol, device_uid, name, last_seen_at, trusted) \
             VALUES ('dlna', 'u', 'T', 0, 2)",
        ),
    ];

    /// The disposable tier, exactly as invariant I9 names it.
    const CACHE_TIER: [&str; 2] = ["provider_cache", "image_cache"];

    #[tokio::test]
    async fn unknown_enumeration_values_are_rejected() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated(&directory).await;
        write_all(&mut connection, BASE_FIXTURES).await;

        assert_check_behaviour(&mut connection, ENUM_REJECTS, ENUM_ACCEPTS).await;
    }

    #[tokio::test]
    async fn boolean_columns_reject_values_other_than_zero_or_one() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated(&directory).await;
        write_all(&mut connection, BASE_FIXTURES).await;

        for (column, sql) in BOOLEAN_REJECTS.iter().copied() {
            let error = statement(&mut connection, sql)
                .await
                .expect_err(&format!("`{column}` must reject 2"));
            assert!(error.to_string().contains("CHECK"), "{column}: got {error}");
        }
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

    /// I12 and PG-E3-08: the dedupe index is partial over the non-terminal states, so the
    /// key is occupied while the work is live and released the moment the job reaches a
    /// terminal state. `job_dedupe_uq` is defined by migration 0001; 0006 is the migration
    /// PG-E3-08 owns and this is the acceptance it is gated on.
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

        // The key stays occupied while the live row moves between non-terminal states and
        // is released only by a terminal one.
        sqlx::query("UPDATE job SET state = 'running' WHERE dedupe_key = 'tmdb:1'")
            .execute(&mut connection)
            .await
            .unwrap();
        assert!(
            insert_job(&mut connection, "deferred", Some("tmdb:1"))
                .await
                .is_err(),
            "a running job still occupies the key"
        );
        sqlx::query("UPDATE job SET state = 'failed' WHERE dedupe_key = 'tmdb:1'")
            .execute(&mut connection)
            .await
            .unwrap();
        insert_job(&mut connection, "queued", Some("tmdb:1"))
            .await
            .expect("a terminal job frees the key for a retry");

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
        apply(&mut first, &path).await.unwrap();
        first.close().await.unwrap();

        let mut second = open_writer_connection(&path).await.unwrap();
        let applied = apply(&mut second, &path).await.unwrap();

        assert_eq!(applied, super::count());
        assert_eq!(
            versions(&mut second).await,
            (1..=i64::try_from(super::count()).unwrap()).collect::<Vec<_>>(),
            "a second run must not record the migrations twice"
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
            documented_tables(&mut connection).await,
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
        apply(&mut connection, &path).await.unwrap();
        let before = schema(&mut connection).await;
        connection.close().await.unwrap();

        std::fs::remove_file(&path).unwrap();
        remove_if_present(&path.with_extension("sqlite-wal"));
        remove_if_present(&path.with_extension("sqlite-shm"));

        let mut connection = open_writer_connection(&path).await.unwrap();
        apply(&mut connection, &path).await.unwrap();

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

    /// Every foreign key the migrations declare, with the delete behaviour that
    /// `pragma_foreign_key_list` reports. A dropped `ON DELETE` clause is invisible to
    /// `pragma_table_info`, so it gets its own test.
    #[tokio::test]
    async fn the_documented_foreign_keys_carry_their_delete_behaviour() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated(&directory).await;

        let mut actual = Vec::new();
        for table in DOCUMENTED_TABLES {
            for (parent, from, to, on_delete) in foreign_keys(&mut connection, table).await {
                actual.push((table.to_owned(), parent, from, to, on_delete));
            }
        }

        let mut expected: Vec<(String, String, String, String, String)> = DOCUMENTED_FOREIGN_KEYS
            .iter()
            .map(|(child, parent, from, to, on_delete)| {
                (
                    (*child).to_owned(),
                    (*parent).to_owned(),
                    (*from).to_owned(),
                    (*to).to_owned(),
                    (*on_delete).to_owned(),
                )
            })
            .collect();
        actual.sort();
        expected.sort();

        assert_eq!(actual, expected);
    }

    /// I1: exactly one `external_id` row per `(provider, value)`, and exactly one owner
    /// column set on any row.
    #[tokio::test]
    async fn an_external_id_has_one_owner_and_is_unique_per_provider() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated(&directory).await;
        write_all(&mut connection, BASE_FIXTURES).await;

        statement(
            &mut connection,
            "INSERT INTO external_id (title_id, provider, value, fetched_at) \
             VALUES (1, 'imdb', 'tt0078748', 0)",
        )
        .await
        .unwrap();
        assert!(
            statement(
                &mut connection,
                "INSERT INTO external_id (title_id, provider, value, fetched_at) \
                 VALUES (1, 'imdb', 'tt0078748', 0)",
            )
            .await
            .is_err(),
            "I1: a second row for the same (provider, value) must be rejected"
        );
        statement(
            &mut connection,
            "INSERT INTO external_id (title_id, provider, value, fetched_at) \
             VALUES (1, 'tmdb', 'tt0078748', 0)",
        )
        .await
        .expect("the same value from another provider is a different identifier");
        assert!(
            statement(
                &mut connection,
                "INSERT INTO external_id (title_id, person_id, provider, value, fetched_at) \
                 VALUES (1, 1, 'tvdb', 'x', 0)",
            )
            .await
            .is_err(),
            "exactly one owner column may be set"
        );
    }

    /// PG-E3-05: every catalogue `UNIQUE` constraint, each proven by a refused duplicate.
    #[tokio::test]
    async fn the_catalogue_unique_constraints_reject_duplicates() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated(&directory).await;
        write_all(&mut connection, BASE_FIXTURES).await;

        for (label, sql) in [
            (
                "title_alias (title_id, alias, region)",
                "INSERT INTO title_alias (title_id, alias, region) VALUES (1, 'Alien', 'US')",
            ),
            (
                "genre.name",
                "INSERT INTO genre (name) VALUES ('Science Fiction')",
            ),
            (
                "keyword.name",
                "INSERT INTO keyword (name) VALUES ('space')",
            ),
            (
                "company (name, provider)",
                "INSERT INTO company (name, provider) VALUES ('Weyland-Yutani', 'tmdb')",
            ),
            (
                "credit (title_id, person_id, kind, job, character)",
                "INSERT INTO credit (title_id, person_id, kind, job, character) \
                 VALUES (1, 1, 'cast', 'Actor', 'Ripley')",
            ),
        ] {
            statement(&mut connection, sql)
                .await
                .unwrap_or_else(|error| panic!("the first row of {label} is free: {error}"));
            assert!(
                statement(&mut connection, sql).await.is_err(),
                "{label} must reject a duplicate"
            );
        }

        statement(
            &mut connection,
            "INSERT INTO company (name, provider) VALUES ('Weyland-Yutani', 'imdb')",
        )
        .await
        .expect("the same name from another provider is a different company");
    }

    /// I7 and PG-E3-05: a season number is unique within its title, and an episode number
    /// within its title and season.
    #[tokio::test]
    async fn an_episode_is_unique_within_its_title_and_season() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated(&directory).await;
        write_all(&mut connection, BASE_FIXTURES).await;

        assert!(
            statement(
                &mut connection,
                "INSERT INTO season (title_id, season_number, updated_at) VALUES (1, 1, 0)",
            )
            .await
            .is_err(),
            "I7: a season number is unique within its title"
        );
        assert!(
            statement(
                &mut connection,
                "INSERT INTO episode (title_id, season_id, season_number, episode_number, updated_at) \
                 VALUES (1, 1, 1, 1, 0)",
            )
            .await
            .is_err(),
            "I7: an episode number is unique within its title and season"
        );
        statement(
            &mut connection,
            "INSERT INTO episode (title_id, season_id, season_number, episode_number, updated_at) \
             VALUES (1, 1, 1, 2, 0)",
        )
        .await
        .expect("the next episode number is free");
        statement(
            &mut connection,
            "INSERT INTO season (id, title_id, season_number, updated_at) VALUES (2, 1, 2, 0)",
        )
        .await
        .expect("a second season is free");
        statement(
            &mut connection,
            "INSERT INTO episode (title_id, season_id, season_number, episode_number, updated_at) \
             VALUES (1, 2, 2, 1, 0)",
        )
        .await
        .expect("season 2 episode 1 is a different key");
    }

    /// I2 and PG-E3-06: `media_item_movie_uq` and `media_item_episode_uq` are partial, so a
    /// movie item and an episode item for the same title coexist while a second item for
    /// either target is refused.
    #[tokio::test]
    async fn one_media_item_per_movie_and_one_per_episode() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated(&directory).await;
        write_all(&mut connection, BASE_FIXTURES).await;

        assert!(
            statement(
                &mut connection,
                "INSERT INTO media_item (title_id, state, origin, created_at, updated_at) \
                 VALUES (1, 'wanted', 'scan', 0, 0)",
            )
            .await
            .is_err(),
            "I2: a movie item exists at most once per title"
        );
        statement(
            &mut connection,
            "INSERT INTO media_item (title_id, episode_id, state, origin, created_at, updated_at) \
             VALUES (1, 1, 'wanted', 'scan', 0, 0)",
        )
        .await
        .expect("an episode item may coexist with the movie item");
        assert!(
            statement(
                &mut connection,
                "INSERT INTO media_item (title_id, episode_id, state, origin, created_at, updated_at) \
                 VALUES (1, 1, 'wanted', 'scan', 0, 0)",
            )
            .await
            .is_err(),
            "I2: an episode item exists at most once per episode"
        );
        statement(
            &mut connection,
            "INSERT INTO episode (id, title_id, season_id, season_number, episode_number, updated_at) \
             VALUES (2, 1, 1, 1, 2, 0)",
        )
        .await
        .unwrap();
        statement(
            &mut connection,
            "INSERT INTO media_item (title_id, episode_id, state, origin, created_at, updated_at) \
             VALUES (1, 2, 'wanted', 'scan', 0, 0)",
        )
        .await
        .expect("another episode is a different key");
        statement(
            &mut connection,
            "INSERT INTO media_item (title_id, state, origin, created_at, updated_at) \
             VALUES (2, 'wanted', 'scan', 0, 0)",
        )
        .await
        .expect("another title's movie is a different key");
    }

    /// I3: two items never claim one file.
    #[tokio::test]
    async fn a_media_file_path_is_unique() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated(&directory).await;
        write_all(&mut connection, BASE_FIXTURES).await;

        let first = "INSERT INTO media_file (media_item_id, path, size_bytes, mtime_ms, added_at, updated_at) \
                     VALUES (1, '/media/alien.mkv', 1, 0, 0, 0)";
        statement(&mut connection, first).await.unwrap();
        assert!(
            statement(&mut connection, first).await.is_err(),
            "I3: a path belongs to at most one media_file row"
        );
        statement(
            &mut connection,
            "INSERT INTO media_file (media_item_id, path, size_bytes, mtime_ms, added_at, updated_at) \
             VALUES (1, '/media/alien.1080p.mkv', 1, 0, 0, 0)",
        )
        .await
        .expect("a different path is a different file");
    }

    /// PG-E3-07: `entity_key` is generated from the target, which is what the unique
    /// constraint and the Continue Watching partial index (asserted above) are built on.
    #[tokio::test]
    async fn watch_progress_entity_key_is_generated_from_the_target() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated(&directory).await;
        write_all(&mut connection, BASE_FIXTURES).await;

        statement(
            &mut connection,
            "INSERT INTO watch_progress (account_id, title_id, position_ms, updated_at) \
             VALUES (1, 1, 0, 0)",
        )
        .await
        .unwrap();
        statement(
            &mut connection,
            "INSERT INTO watch_progress (account_id, title_id, episode_id, position_ms, updated_at) \
             VALUES (1, 1, 1, 0, 0)",
        )
        .await
        .unwrap();

        let keys: Vec<String> =
            sqlx::query_scalar("SELECT entity_key FROM watch_progress ORDER BY entity_key")
                .fetch_all(&mut connection)
                .await
                .unwrap();
        assert_eq!(
            keys,
            vec!["e1", "t1"],
            "a movie is 't<title_id>' and an episode is 'e<episode_id>'"
        );
        assert!(
            statement(
                &mut connection,
                "INSERT INTO watch_progress (account_id, title_id, position_ms, updated_at) \
                 VALUES (1, 1, 0, 0)",
            )
            .await
            .is_err(),
            "the generated key is part of UNIQUE (account_id, entity_key)"
        );
    }

    /// The expression index wraps both targets in `COALESCE`, so a re-download is refused
    /// even when the two target columns are NULL rather than only when they are set.
    #[tokio::test]
    async fn a_subtitle_download_is_idempotent_for_the_same_target() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated(&directory).await;
        write_all(&mut connection, BASE_FIXTURES).await;

        let null_target = "INSERT INTO subtitle_download \
            (media_file_id, episode_id, provider, provider_subtitle_id, language, score, created_at) \
            VALUES (NULL, NULL, 'opensubtitles', '123', 'en', 1.0, 0)";
        statement(&mut connection, null_target).await.unwrap();
        assert!(
            statement(&mut connection, null_target).await.is_err(),
            "two NULL targets must still collide through COALESCE"
        );
        statement(
            &mut connection,
            "INSERT INTO subtitle_download \
             (media_file_id, episode_id, provider, provider_subtitle_id, language, score, created_at) \
             VALUES (NULL, 1, 'opensubtitles', '123', 'en', 1.0, 0)",
        )
        .await
        .expect("a different target is a different download");
    }

    /// I13: a suppressed notification always records why; nothing is dropped silently.
    #[tokio::test]
    async fn a_suppressed_notification_must_record_its_reason() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated(&directory).await;

        assert!(
            statement(
                &mut connection,
                "INSERT INTO notification (event, title, delivery_state, created_at) \
                 VALUES ('download.completed', 'Done', 'suppressed', 0)",
            )
            .await
            .is_err(),
            "I13: a suppressed notification without a reason must be rejected"
        );
        statement(
            &mut connection,
            "INSERT INTO notification (event, title, delivery_state, suppressed_reason, created_at) \
             VALUES ('download.completed', 'Done', 'suppressed', 'quiet_hours', 0)",
        )
        .await
        .expect("a suppressed notification that records its reason is stored");
        statement(
            &mut connection,
            "INSERT INTO notification (event, title, delivery_state, created_at) \
             VALUES ('download.completed', 'Done', 'delivered', 0)",
        )
        .await
        .expect("only suppression requires a reason");
    }

    /// One row in every user-data table, so the I9 test can show that the cache tier's
    /// removal changes none of them. The two cache tables get a row in the test itself.
    const USER_DATA_FIXTURES: &[&str] = &[
        "INSERT INTO match_rule (pattern, title_id, created_at) VALUES ('alien', 1, 0)",
        "INSERT INTO settings (key, value_json, updated_at) VALUES ('theme', '\"dark\"', 0)",
        "INSERT INTO secret_ref (key, keyring_service, keyring_account, created_at) \
         VALUES ('tmdb.api_key', 'peregrine', 'tmdb', 0)",
        "INSERT INTO job (kind, state, payload_json, created_at) VALUES ('library.scan', 'queued', '{}', 0)",
        "INSERT INTO event_log (at, kind) VALUES (0, 'media.added')",
        "INSERT INTO title_alias (title_id, alias, region) VALUES (1, 'Alien', 'US')",
        "INSERT INTO external_id (title_id, provider, value, fetched_at) VALUES (1, 'imdb', 'tt0078748', 0)",
        "INSERT INTO genre (id, name) VALUES (1, 'Science Fiction')",
        "INSERT INTO title_genre (title_id, genre_id) VALUES (1, 1)",
        "INSERT INTO keyword (id, name) VALUES (1, 'space')",
        "INSERT INTO title_keyword (title_id, keyword_id) VALUES (1, 1)",
        "INSERT INTO title_company (title_id, company_id, role) VALUES (1, 1, 'production')",
        "INSERT INTO credit (title_id, person_id, kind, job, character) \
         VALUES (1, 1, 'cast', 'Actor', 'Ripley')",
        "INSERT INTO episode_ordering (episode_id, scheme, season_number, episode_number) \
         VALUES (1, 'dvd', 1, 1)",
        "INSERT INTO media_file (media_item_id, path, size_bytes, mtime_ms, added_at, updated_at) \
         VALUES (1, '/media/alien.mkv', 1, 0, 0, 0)",
        "INSERT INTO unmatched_file (path, size_bytes, mtime_ms, first_seen_at) \
         VALUES ('/media/unknown.mkv', 1, 0, 0)",
        "INSERT INTO subtitle_track (episode_id, origin, format, language) \
         VALUES (1, 'external', 'srt', 'en')",
        "INSERT INTO subtitle_download \
         (episode_id, provider, provider_subtitle_id, language, score, created_at) \
         VALUES (1, 'opensubtitles', '123', 'en', 1.0, 0)",
        "INSERT INTO watch_progress (account_id, title_id, position_ms, updated_at) VALUES (1, 1, 0, 0)",
        "INSERT INTO watchlist_entry (account_id, title_id, added_at) VALUES (1, 1, 0)",
        "INSERT INTO smart_collection (name, query_json, sort, created_at) \
         VALUES ('Recently added', '{}', 'created_at DESC', 0)",
        "INSERT INTO quality_profile (name, rules_json, cutoff_rank, updated_at) \
         VALUES ('Any', '{}', 0, 0)",
        "INSERT INTO indexer (kind, name) VALUES ('builtin', 'Local')",
        "INSERT INTO download_job (title_id, state, release_title, save_path, added_at) \
         VALUES (1, 'queued', 'Alien 1979 1080p', '/downloads', 0)",
        "INSERT INTO download_file (download_job_id, file_index, path, size_bytes) \
         VALUES (1, 0, 'alien.mkv', 1)",
        "INSERT INTO download_schedule (account_id, title_id, release_on, created_at, updated_at) \
         VALUES (1, 1, '2030-01-01', 0, 0)",
        "INSERT INTO release_decision \
         (download_job_id, title_id, release_title, score, rank_position, reasons_json, created_at) \
         VALUES (1, 1, 'Alien 1979 1080p', 1.0, 1, '{}', 0)",
        "INSERT INTO image_ref (owner_kind, owner_id, role, provider, path, language) \
         VALUES ('title', 1, 'poster', 'tmdb', '/poster.jpg', 'en')",
        "INSERT INTO dataset_state (name) VALUES ('imdb.ratings')",
        "INSERT INTO notification (event, title, created_at) \
         VALUES ('download.completed', 'Download complete', 0)",
        "INSERT INTO paired_device (name, token_hash, scopes, created_at) \
         VALUES ('Test TV', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'read,stream', 0)",
        "INSERT INTO web_audit (device_id, action, title_id, created_at) \
         VALUES (1, 'acquire', 1, 0)",
        "INSERT INTO cast_session (device_id, state, delivery, started_at) \
         VALUES (1, 'playing', 'direct', 0)",
        "INSERT INTO discover_rail (rail, position, title_id) VALUES ('popular_movies', 0, 1)",
        "INSERT INTO provider_cache \
         (key, provider, resource, fetched_at, expires_at, schema_version, body, size_bytes) \
         VALUES ('sha256:provider', 'tmdb', 'movie', 0, 0, 1, X'00', 1)",
        "INSERT INTO image_cache \
         (key, provider, path, variant, disk_path, bytes, created_at, last_used_at) \
         VALUES ('sha256:image', 'tmdb', '/poster.jpg', 'w342', '/cache/ab.webp', 1, 0, 0)",
        "INSERT INTO usenet_server (name, host, port, password_secret, created_at) \
         VALUES ('Primary', 'news.example.test', 563, 'provider.usenet.one', 0)",
        "INSERT INTO watch_sync (title_id, synced_at) VALUES (1, 0)",
        "INSERT INTO watch_availability (title_id, region, source, link) \
         VALUES (1, 'US', 'tmdb', 'https://example.test/watch')",
        "INSERT INTO watch_offer \
         (title_id, region, category, provider_id, provider_name, logo_path, display_priority) \
         VALUES (1, 'US', 'flatrate', 8, 'Netflix', '/netflix.jpg', 0)",
    ];

    /// One `COUNT(*)` per user-data table, as a single static query. A dynamic
    /// `SELECT COUNT(*) FROM {table}` would be refused by `sqlx`'s injection guard,
    /// correctly, so the table list is a literal and the comparison is in Rust.
    const USER_DATA_COUNTS: &str = "
        SELECT 'account' AS table_name, COUNT(*) AS n FROM account
        UNION ALL
        SELECT 'cast_session' AS table_name, COUNT(*) AS n FROM cast_session
        UNION ALL
        SELECT 'company' AS table_name, COUNT(*) AS n FROM company
        UNION ALL
        SELECT 'credit' AS table_name, COUNT(*) AS n FROM credit
        UNION ALL
        SELECT 'dataset_state' AS table_name, COUNT(*) AS n FROM dataset_state
        UNION ALL
        SELECT 'device' AS table_name, COUNT(*) AS n FROM device
        UNION ALL
        SELECT 'discover_rail' AS table_name, COUNT(*) AS n FROM discover_rail
        UNION ALL
        SELECT 'download_file' AS table_name, COUNT(*) AS n FROM download_file
        UNION ALL
        SELECT 'download_job' AS table_name, COUNT(*) AS n FROM download_job
        UNION ALL
        SELECT 'download_schedule' AS table_name, COUNT(*) AS n FROM download_schedule
        UNION ALL
        SELECT 'episode' AS table_name, COUNT(*) AS n FROM episode
        UNION ALL
        SELECT 'episode_ordering' AS table_name, COUNT(*) AS n FROM episode_ordering
        UNION ALL
        SELECT 'event_log' AS table_name, COUNT(*) AS n FROM event_log
        UNION ALL
        SELECT 'external_id' AS table_name, COUNT(*) AS n FROM external_id
        UNION ALL
        SELECT 'genre' AS table_name, COUNT(*) AS n FROM genre
        UNION ALL
        SELECT 'image_ref' AS table_name, COUNT(*) AS n FROM image_ref
        UNION ALL
        SELECT 'indexer' AS table_name, COUNT(*) AS n FROM indexer
        UNION ALL
        SELECT 'job' AS table_name, COUNT(*) AS n FROM job
        UNION ALL
        SELECT 'keyword' AS table_name, COUNT(*) AS n FROM keyword
        UNION ALL
        SELECT 'library_root' AS table_name, COUNT(*) AS n FROM library_root
        UNION ALL
        SELECT 'match_rule' AS table_name, COUNT(*) AS n FROM match_rule
        UNION ALL
        SELECT 'media_file' AS table_name, COUNT(*) AS n FROM media_file
        UNION ALL
        SELECT 'media_item' AS table_name, COUNT(*) AS n FROM media_item
        UNION ALL
        SELECT 'notification' AS table_name, COUNT(*) AS n FROM notification
        UNION ALL
        SELECT 'paired_device' AS table_name, COUNT(*) AS n FROM paired_device
        UNION ALL
        SELECT 'person' AS table_name, COUNT(*) AS n FROM person
        UNION ALL
        SELECT 'playback_preference' AS table_name, COUNT(*) AS n FROM playback_preference
        UNION ALL
        SELECT 'quality_profile' AS table_name, COUNT(*) AS n FROM quality_profile
        UNION ALL
        SELECT 'release_decision' AS table_name, COUNT(*) AS n FROM release_decision
        UNION ALL
        SELECT 'season' AS table_name, COUNT(*) AS n FROM season
        UNION ALL
        SELECT 'secret_ref' AS table_name, COUNT(*) AS n FROM secret_ref
        UNION ALL
        SELECT 'settings' AS table_name, COUNT(*) AS n FROM settings
        UNION ALL
        SELECT 'smart_collection' AS table_name, COUNT(*) AS n FROM smart_collection
        UNION ALL
        SELECT 'subtitle_download' AS table_name, COUNT(*) AS n FROM subtitle_download
        UNION ALL
        SELECT 'subtitle_track' AS table_name, COUNT(*) AS n FROM subtitle_track
        UNION ALL
        SELECT 'title' AS table_name, COUNT(*) AS n FROM title
        UNION ALL
        SELECT 'title_alias' AS table_name, COUNT(*) AS n FROM title_alias
        UNION ALL
        SELECT 'title_company' AS table_name, COUNT(*) AS n FROM title_company
        UNION ALL
        SELECT 'title_genre' AS table_name, COUNT(*) AS n FROM title_genre
        UNION ALL
        SELECT 'title_keyword' AS table_name, COUNT(*) AS n FROM title_keyword
        UNION ALL
        SELECT 'unmatched_file' AS table_name, COUNT(*) AS n FROM unmatched_file
        UNION ALL
        SELECT 'watch_availability' AS table_name, COUNT(*) AS n FROM watch_availability
        UNION ALL
        SELECT 'watch_offer' AS table_name, COUNT(*) AS n FROM watch_offer
        UNION ALL
        SELECT 'watch_progress' AS table_name, COUNT(*) AS n FROM watch_progress
        UNION ALL
        SELECT 'usenet_server' AS table_name, COUNT(*) AS n FROM usenet_server
        UNION ALL
        SELECT 'usenet_setting' AS table_name, COUNT(*) AS n FROM usenet_setting
        UNION ALL
        SELECT 'watch_sync' AS table_name, COUNT(*) AS n FROM watch_sync
        UNION ALL
        SELECT 'watchlist_entry' AS table_name, COUNT(*) AS n FROM watchlist_entry
        UNION ALL
        SELECT 'web_audit' AS table_name, COUNT(*) AS n FROM web_audit
    ";

    /// The two disposable tables, counted before they are dropped.
    const CACHE_COUNTS: &str = "
        SELECT 'image_cache' AS table_name, COUNT(*) AS n FROM image_cache
        UNION ALL
        SELECT 'provider_cache' AS table_name, COUNT(*) AS n FROM provider_cache
    ";

    /// I9 and PG-E3-09: `provider_cache` and `image_cache` are always droppable without
    /// loss of user data. Nothing may reference them, and dropping both must leave every
    /// other table, and every row in it, exactly as it was.
    #[tokio::test]
    async fn dropping_the_cache_tables_loses_no_user_data() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated(&directory).await;
        write_all(&mut connection, BASE_FIXTURES).await;
        write_all(&mut connection, USER_DATA_FIXTURES).await;

        // Nothing depends on the cache tier; that is what makes it disposable.
        for table in DOCUMENTED_TABLES {
            for (parent, from, ..) in foreign_keys(&mut connection, table).await {
                assert!(
                    !CACHE_TIER.contains(&parent.as_str()),
                    "{table}.{from} depends on the cache tier, so I9 does not hold"
                );
            }
        }

        // `UNION ALL` does not promise a row order, so every comparison is on a sorted
        // `Vec` rather than on the order the planner happened to return.
        let mut before: Vec<(String, i64)> = sqlx::query_as(USER_DATA_COUNTS)
            .fetch_all(&mut connection)
            .await
            .unwrap();
        before.sort();
        let schema_before = schema_without_cache(&mut connection).await;
        assert_eq!(
            before.len(),
            DOCUMENTED_TABLES.len() - CACHE_TIER.len(),
            "one count per user-data table: {before:?}"
        );
        assert!(
            before.iter().all(|(_, count)| *count > 0),
            "the fixture must hold a row in every user-data table: {before:?}"
        );
        let mut cache_rows: Vec<(String, i64)> = sqlx::query_as(CACHE_COUNTS)
            .fetch_all(&mut connection)
            .await
            .unwrap();
        cache_rows.sort();
        assert_eq!(
            cache_rows,
            vec![
                ("image_cache".to_owned(), 1),
                ("provider_cache".to_owned(), 1)
            ],
            "each cache table holds a representative row"
        );

        for drop in ["DROP TABLE provider_cache", "DROP TABLE image_cache"] {
            sqlx::query(drop).execute(&mut connection).await.unwrap();
        }

        let mut after: Vec<(String, i64)> = sqlx::query_as(USER_DATA_COUNTS)
            .fetch_all(&mut connection)
            .await
            .unwrap();
        after.sort();
        assert_eq!(
            after, before,
            "I9: dropping the cache tier must not remove or change a user-data row"
        );
        assert_eq!(
            schema_without_cache(&mut connection).await,
            schema_before,
            "I9: the rest of the schema must be unchanged"
        );

        // The catalogue still renders: the artwork reference it reads survived the drop.
        let poster: String = sqlx::query_scalar(
            "SELECT path FROM image_ref WHERE owner_kind = 'title' AND owner_id = 1",
        )
        .fetch_one(&mut connection)
        .await
        .unwrap();
        assert_eq!(poster, "/poster.jpg");
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
                event: peregrine_domain::Event::DownloadQueued { job_id: 1 },
                detail_json: None,
            })
            .await
            .unwrap();

        assert!(path.is_file(), "open must have created the database file");
        assert_eq!(database.event_count().await.unwrap(), 1);
    }

    /// A connection holding migrations 0001-0007 only, with the base fixtures in it.
    ///
    /// PG-E3-19's acceptance asks for an upgrade fixture that preserves existing rows, and
    /// the only honest way to test that is to write rows under the shape that preceded the
    /// correction. `run_to` stops the embedded migrator at a version and records it, so
    /// applying the rest afterwards is the real upgrade path rather than a replay.
    async fn migrated_up_to_0007(directory: &TempDir) -> SqliteConnection {
        let mut connection = open_writer_connection(&directory.path().join("peregrine.sqlite"))
            .await
            .unwrap();
        MIGRATOR.run_to(7, &mut connection).await.unwrap();
        assert_eq!(
            versions(&mut connection).await,
            vec![1, 2, 3, 4, 5, 6, 7],
            "the upgrade fixture must stop at the migration before the correction"
        );
        write_all(&mut connection, BASE_FIXTURES).await;
        connection
    }

    /// PG-E3-19: the name says `_uq`, so it is unique — among live jobs only, because
    /// re-downloading a release after a failure or a removal is ordinary.
    #[tokio::test]
    async fn the_info_hash_index_is_unique_among_live_jobs_only() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated(&directory).await;
        write_all(&mut connection, BASE_FIXTURES).await;

        let live = "INSERT INTO download_job (title_id, state, release_title, save_path, added_at, info_hash) \
                    VALUES (1, 'downloading', 'Alien 1979 1080p', '/downloads', 0, 'aabb')";
        statement(&mut connection, live).await.unwrap();
        let refused = statement(&mut connection, live)
            .await
            .expect_err("two live jobs may not share one info hash");
        assert!(
            refused.to_string().to_uppercase().contains("UNIQUE"),
            "the refusal must be the uniqueness constraint: {refused}"
        );

        // A terminal row for the same hash is history, not a duplicate.
        for terminal in ["completed", "failed", "blocked", "removed"] {
            statement(
                &mut connection,
                match terminal {
                    "completed" => "INSERT INTO download_job (title_id, state, release_title, save_path, added_at, info_hash) \
                                    VALUES (1, 'completed', 'Alien', '/downloads', 0, 'aabb')",
                    "failed" => "INSERT INTO download_job (title_id, state, release_title, save_path, added_at, info_hash) \
                                 VALUES (1, 'failed', 'Alien', '/downloads', 0, 'aabb')",
                    "blocked" => "INSERT INTO download_job (title_id, state, release_title, save_path, added_at, info_hash) \
                                  VALUES (1, 'blocked', 'Alien', '/downloads', 0, 'aabb')",
                    _ => "INSERT INTO download_job (title_id, state, release_title, save_path, added_at, info_hash) \
                          VALUES (1, 'removed', 'Alien', '/downloads', 0, 'aabb')",
                },
            )
            .await
            .unwrap_or_else(|error| panic!("`{terminal}` is terminal, so it may repeat: {error}"));
        }

        // A job with no hash never collides with another job with no hash, which is what
        // the `IS NOT NULL` half of the predicate is for.
        let hashless = "INSERT INTO download_job (title_id, state, release_title, save_path, added_at) \
                        VALUES (1, 'queued', 'Alien', '/downloads', 0)";
        statement(&mut connection, hashless).await.unwrap();
        statement(&mut connection, hashless).await.unwrap();

        // Once the live job is gone the hash is free again: a re-download is a new job.
        statement(
            &mut connection,
            "UPDATE download_job SET state = 'removed' WHERE state = 'downloading'",
        )
        .await
        .unwrap();
        statement(&mut connection, live).await.unwrap();
    }

    /// PG-E3-19: `language` is a sentinel, not NULL, because a NULL defeats a UNIQUE
    /// constraint in SQLite and identical artwork rows could coexist.
    #[tokio::test]
    async fn artwork_language_is_a_sentinel_and_never_null() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated(&directory).await;
        write_all(&mut connection, BASE_FIXTURES).await;

        // Omitting the column means "the provider gave no language".
        statement(
            &mut connection,
            "INSERT INTO image_ref (owner_kind, owner_id, role, provider, path) \
             VALUES ('title', 1, 'poster', 'tmdb', '/poster.jpg')",
        )
        .await
        .unwrap();
        let language: String =
            sqlx::query_scalar("SELECT language FROM image_ref WHERE path = '/poster.jpg'")
                .fetch_one(&mut connection)
                .await
                .unwrap();
        assert_eq!(language, "", "the sentinel is the empty string");

        let duplicate = "INSERT INTO image_ref (owner_kind, owner_id, role, provider, path) \
                         VALUES ('title', 1, 'poster', 'tmdb', '/poster.jpg')";
        let refused = statement(&mut connection, duplicate)
            .await
            .expect_err("the same artwork must not be storable twice");
        assert!(
            refused.to_string().to_uppercase().contains("UNIQUE"),
            "the refusal must be the uniqueness constraint: {refused}"
        );

        let null_language = "INSERT INTO image_ref (owner_kind, owner_id, role, provider, path, language) \
                             VALUES ('title', 1, 'poster', 'tmdb', '/other.jpg', NULL)";
        let refused = statement(&mut connection, null_language)
            .await
            .expect_err("NULL is no longer a storable language");
        assert!(
            refused.to_string().to_uppercase().contains("NOT NULL"),
            "the refusal must be the NOT NULL constraint: {refused}"
        );

        // A different language is a different row: the key still includes it.
        statement(
            &mut connection,
            "INSERT INTO image_ref (owner_kind, owner_id, role, provider, path, language) \
             VALUES ('title', 1, 'poster', 'tmdb', '/poster.jpg', 'de')",
        )
        .await
        .unwrap();
    }

    /// PG-E3-19: the rename carries the values that were written under the old name, and
    /// the old name is gone.
    #[tokio::test]
    async fn an_upgrade_renames_the_video_profile_column_and_keeps_its_values() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated_up_to_0007(&directory).await;

        statement(
            &mut connection,
            "INSERT INTO media_file \
             (media_item_id, path, size_bytes, mtime_ms, added_at, video_account, updated_at) \
             VALUES (1, '/media/alien.mkv', 1, 0, 0, 'High', 0)",
        )
        .await
        .unwrap();
        // The fixture needs a media item to point at; BASE_FIXTURES creates one.
        apply(&mut connection, &database_path(&directory))
            .await
            .unwrap();

        let profile: Option<String> = sqlx::query_scalar(
            "SELECT video_profile FROM media_file WHERE path = '/media/alien.mkv'",
        )
        .fetch_one(&mut connection)
        .await
        .unwrap();
        assert_eq!(
            profile.as_deref(),
            Some("High"),
            "the probe value must survive the rename"
        );

        assert!(
            sqlx::query_scalar::<_, String>("SELECT video_account FROM media_file")
                .fetch_all(&mut connection)
                .await
                .is_err(),
            "the mistyped column must be gone"
        );
    }

    /// PG-E3-19: two live jobs for one hash cannot be reconciled by refusing to open the
    /// database, so the later ones are marked failed — and nothing is deleted.
    #[tokio::test]
    async fn an_upgrade_reconciles_duplicate_live_info_hashes_without_deleting_a_row() {
        /// The live download states, which is the predicate the corrected index uses.
        const LIVE: [&str; 6] = [
            "queued",
            "inspecting",
            "metadata",
            "downloading",
            "paused",
            "seeding",
        ];

        let directory = TempDir::new().unwrap();
        let mut connection = migrated_up_to_0007(&directory).await;

        // Under the old index two live jobs could share a hash, and a third live row could
        // share it with a job that had already finished.
        for (state, hash) in [
            ("downloading", "aabb"),
            ("metadata", "aabb"),
            ("seeding", "aabb"),
            ("queued", "ccdd"),
            ("completed", "aabb"),
        ] {
            literal_statement(
                &mut connection,
                &format!(
                    "INSERT INTO download_job \
                     (title_id, state, release_title, save_path, added_at, info_hash) \
                     VALUES (1, '{state}', 'Alien', '/downloads', 0, '{hash}')"
                ),
            )
            .await
            .unwrap();
        }

        apply(&mut connection, &database_path(&directory))
            .await
            .unwrap();

        let rows: Vec<(i64, String, String, Option<String>)> = sqlx::query_as(
            "SELECT id, state, COALESCE(info_hash, ''), error_code FROM download_job ORDER BY id",
        )
        .fetch_all(&mut connection)
        .await
        .unwrap();

        assert_eq!(rows.len(), 5, "no row may be deleted by an upgrade");

        let live_aabb: Vec<&(i64, String, String, Option<String>)> = rows
            .iter()
            .filter(|row| row.2 == "aabb" && LIVE.contains(&row.1.as_str()))
            .collect();
        assert_eq!(
            live_aabb.len(),
            1,
            "one live job per hash after the upgrade"
        );
        assert_eq!(
            live_aabb[0].0, 1,
            "the earliest live row is the one that stays live"
        );
        for row in rows.iter().filter(|row| row.0 == 2 || row.0 == 3) {
            assert_eq!(row.1, "failed", "a superseded duplicate is marked");
            assert_eq!(row.3.as_deref(), Some("DUPLICATE_INFO_HASH"));
        }
        assert_eq!(rows[3].1, "queued", "a different hash is untouched");
        assert_eq!(rows[4].1, "completed", "history is left alone");
        assert_eq!(rows[4].3, None, "a terminal row keeps its own error state");
    }

    /// PG-E3-19: identical artwork collapses to one row, language becomes a sentinel, and
    /// the surviving rows keep their ids.
    #[tokio::test]
    async fn an_upgrade_collapses_identical_artwork_and_makes_language_not_null() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated_up_to_0007(&directory).await;

        // Under the old schema the first two rows were both storable, because a NULL
        // language and an empty-string language are different values to a UNIQUE
        // constraint. The third is a different path and the fourth a real language.
        for (path, language) in [
            ("/poster.jpg", "NULL"),
            ("/poster.jpg", "''"),
            ("/backdrop.jpg", "NULL"),
            ("/poster.jpg", "'de'"),
        ] {
            literal_statement(
                &mut connection,
                &format!(
                    "INSERT INTO image_ref \
                     (owner_kind, owner_id, role, provider, path, language) \
                     VALUES ('title', 1, 'poster', 'tmdb', '{path}', {language})"
                ),
            )
            .await
            .unwrap();
        }

        apply(&mut connection, &database_path(&directory))
            .await
            .unwrap();

        let rows: Vec<(i64, String, String)> =
            sqlx::query_as("SELECT id, path, language FROM image_ref ORDER BY id")
                .fetch_all(&mut connection)
                .await
                .unwrap();

        assert_eq!(
            rows.len(),
            3,
            "the two `poster.jpg` rows without a language collapse into one"
        );
        assert_eq!(
            rows[0],
            (1, "/poster.jpg".to_owned(), String::new()),
            "the earliest row survives, with the sentinel"
        );
        assert_eq!(
            rows[1],
            (3, "/backdrop.jpg".to_owned(), String::new()),
            "a different path is a different row, and its NULL became the sentinel"
        );
        assert_eq!(
            rows[2],
            (4, "/poster.jpg".to_owned(), "de".to_owned()),
            "a real language is a different row, and ids are preserved"
        );
    }

    /// The fixture migration's SQL: it creates and writes before it fails.
    const FAILING_MIGRATION: &str = include_str!("../tests/fixtures/failing_migration.sql");

    /// The version the fixture takes, past every merged migration.
    /// The version after the embedded set: this fixture migration is appended to the real
    /// ones, so it moves whenever a migration is added.
    /// A version past the embedded set, so a migration written by this test cannot collide with one
    /// the product ships — which is what a hardcoded number did the first time a migration landed.
    fn failing_version() -> i64 {
        i64::try_from(super::count()).unwrap() + 1
    }

    /// A corrected version of the fixture, used to prove a failed run can be retried.
    const FIXED_MIGRATION: &str = "CREATE TABLE migration_probe (id INTEGER PRIMARY KEY, note TEXT NOT NULL); \
                                   INSERT INTO migration_probe (id, note) VALUES (1, 'fixed');";

    /// The embedded set plus one migration that always fails.
    ///
    /// The embedded migrations are reused with their own checksums, so the run is a real
    /// upgrade of a database that already holds them rather than a replay from empty.
    fn migrator_with_a_failure() -> Migrator {
        with_extra(Migration::new(
            failing_version(),
            "deliberately_failing".into(),
            MigrationType::Simple,
            FAILING_MIGRATION.into_sql_str(),
            false,
        ))
    }

    /// The embedded set plus `migration`.
    fn with_extra(migration: Migration) -> Migrator {
        let mut migrations: Vec<Migration> = MIGRATOR.iter().cloned().collect();
        migrations.push(migration);
        Migrator::with_migrations(migrations)
    }

    /// Whether `table` exists, which is how a rolled-back `CREATE TABLE` is visible.
    async fn table_exists(connection: &mut SqliteConnection, table: &str) -> bool {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        )
        .bind(table)
        .fetch_one(&mut *connection)
        .await
        .unwrap();
        count > 0
    }

    /// The pre-migration copies in `directory`, by file name.
    async fn copies(directory: &TempDir) -> Vec<String> {
        let mut names = Vec::new();
        let mut entries = tokio::fs::read_dir(directory.path()).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.contains(".backup-") {
                names.push(name);
            }
        }
        names.sort();
        names
    }

    /// Migration 0019 repairs what 0014 left behind, and backfills what it can know.
    ///
    /// 'partial' was what a *list* answer stamped before 'listed' existed, and 0014 copied the table
    /// without correcting the rows it invalidated, so a title a rail listed claims a details read to
    /// this day and every rule that reads `has_details` treats it as finished. Only a details read
    /// writes the columns the repair tests, which is what makes the predicate sayable.
    #[tokio::test]
    async fn migration_0019_repairs_the_rows_a_list_answer_stamped() {
        let directory = TempDir::new().unwrap();
        let mut connection = open_writer_connection(&database_path(&directory))
            .await
            .unwrap();
        MIGRATOR.run_to(18, &mut connection).await.unwrap();

        // A list answer's row: a name, a synopsis, and none of the columns only a details read
        // writes.
        statement(
            &mut connection,
            "INSERT INTO title (id, kind, name, sort_name, metadata_state, overview, created_at, updated_at) VALUES (1, 'show', 'The Wire', 'wire', 'partial', 'A show.', 0, 0)",
        )
        .await
        .unwrap();
        // The same stamp on a row a details read did reach: a season count is the tell.
        statement(
            &mut connection,
            "INSERT INTO title (id, kind, name, sort_name, metadata_state, season_count, created_at, updated_at) VALUES (2, 'show', 'The Sopranos', 'sopranos', 'partial', 6, 0, 0)",
        )
        .await
        .unwrap();
        // A show that already holds a season row was answered, and the backfill says so rather than
        // asking every show the catalogue knows about once more.
        statement(
            &mut connection,
            "INSERT INTO title (id, kind, name, sort_name, metadata_state, season_count, last_synced_at, created_at, updated_at) VALUES (3, 'show', 'The Wire', 'wire', 'partial', 1, 500, 0, 0)",
        )
        .await
        .unwrap();
        statement(
            &mut connection,
            "INSERT INTO season (id, title_id, season_number, updated_at) VALUES (1, 3, 1, 0)",
        )
        .await
        .unwrap();

        MIGRATOR.run(&mut connection).await.unwrap();

        let rows: Vec<(i64, String, Option<i64>)> =
            sqlx::query_as("SELECT id, metadata_state, season_synced_at FROM title ORDER BY id")
                .fetch_all(&mut connection)
                .await
                .unwrap();
        assert_eq!(
            rows,
            vec![
                (1, "listed".to_owned(), None),
                (2, "partial".to_owned(), None),
                (3, "partial".to_owned(), Some(500)),
            ],
            "a list-stamped row is due again, a details read is left alone, and an answered show \
             is not asked about its seasons twice"
        );
    }

    /// PG-E3-13's acceptance: a failing migration leaves the database untouched and a copy
    /// of it exists.
    #[tokio::test]
    async fn a_failing_migration_leaves_the_database_unchanged_and_backed_up() {
        let directory = TempDir::new().unwrap();
        let path = database_path(&directory);
        let mut connection = migrated(&directory).await;
        write_all(&mut connection, BASE_FIXTURES).await;
        let before = schema(&mut connection).await;

        let error = run(&mut connection, &path, &migrator_with_a_failure())
            .await
            .expect_err("the fixture migration always fails");

        let StorageError::MigrationFailed {
            version,
            description,
            backup,
            ..
        } = &error
        else {
            panic!("expected a migration failure naming the migration, got {error:?}");
        };
        assert_eq!(*version, failing_version());
        assert_eq!(description, "deliberately_failing");
        let message = error.to_string();
        assert!(
            message.contains(&format!("{:04}_deliberately_failing", failing_version())),
            "the error must name the migration: {message}"
        );
        assert!(
            message.contains("restore the copy taken before the upgrade"),
            "the error must say what to do: {message}"
        );
        let backup = backup
            .clone()
            .expect("an existing database is copied first");
        assert!(
            backup.is_file(),
            "the copy must exist: {}",
            backup.display()
        );

        // Nothing the failed migration wrote survives: the fixture creates a table and
        // inserts a row before it fails, so a runner without a transaction per migration
        // would leave both behind.
        assert!(
            !table_exists(&mut connection, "migration_probe").await,
            "the failed migration's table must be rolled back"
        );
        assert_eq!(schema(&mut connection).await, before);
        assert_eq!(
            versions(&mut connection).await,
            (1..=i64::try_from(super::count()).unwrap()).collect::<Vec<_>>(),
            "a failed migration must not be recorded"
        );
        let titles: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM title")
            .fetch_one(&mut connection)
            .await
            .unwrap();
        assert_eq!(titles, 2, "the user's rows are untouched");

        // The copy is the database as it was *before* the migration: no probe table, and
        // no record of the version that failed.
        let mut copy = open_writer_connection(&backup).await.unwrap();
        assert!(!table_exists(&mut copy, "migration_probe").await);
        assert_eq!(
            versions(&mut copy).await,
            (1..=i64::try_from(super::count()).unwrap()).collect::<Vec<_>>(),
            "the copy holds what was applied before the failing migration"
        );
        let copied_titles: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM title")
            .fetch_one(&mut copy)
            .await
            .unwrap();
        assert_eq!(copied_titles, 2);
    }

    /// The failure is recoverable: the database was not left dirty, so the corrected
    /// migration applies to it without a restore.
    #[tokio::test]
    async fn a_failed_migration_is_applied_when_it_is_fixed() {
        let directory = TempDir::new().unwrap();
        let path = database_path(&directory);
        let mut connection = migrated(&directory).await;

        run(&mut connection, &path, &migrator_with_a_failure())
            .await
            .expect_err("the fixture migration always fails");

        let fixed = with_extra(Migration::new(
            failing_version(),
            "deliberately_failing".into(),
            MigrationType::Simple,
            FIXED_MIGRATION.into_sql_str(),
            false,
        ));
        let applied = run(&mut connection, &path, &fixed).await.unwrap();

        // The embedded set plus the one this test fixed, derived for the same reason.
        assert_eq!(applied, super::count() + 1);
        assert_eq!(
            versions(&mut connection).await,
            (1..=i64::try_from(super::count()).unwrap() + 1).collect::<Vec<_>>()
        );
        assert!(table_exists(&mut connection, "migration_probe").await);
    }

    /// A copy is taken before the first pending migration, and it holds the schema as it
    /// was, which is what makes it a way back rather than a second copy of the new one.
    #[tokio::test]
    async fn an_upgrade_copies_the_database_before_the_pending_migration() {
        let directory = TempDir::new().unwrap();
        let mut connection = migrated_up_to_0007(&directory).await;

        let applied = apply(&mut connection, &database_path(&directory))
            .await
            .unwrap();

        // `apply` answers how many migrations the embedded set holds once it has run, not how many
        // this call applied — the fixture started seven migrations in.
        assert_eq!(applied, super::count());
        let copies = copies(&directory).await;
        assert_eq!(
            copies,
            vec!["peregrine.sqlite.backup-0007-01".to_owned()],
            "one copy, named after the version it holds"
        );
        let mut copy = open_writer_connection(&directory.path().join(&copies[0]))
            .await
            .unwrap();
        assert_eq!(
            versions(&mut copy).await,
            vec![1, 2, 3, 4, 5, 6, 7],
            "the copy predates the migration that was pending"
        );
        assert_eq!(
            versions(&mut connection).await,
            (1..=i64::try_from(super::count()).unwrap()).collect::<Vec<_>>()
        );
    }

    /// A database that is already up to date is not copied again: the copy is for an
    /// upgrade, and taking one on every boot would grow the data directory for nothing.
    #[tokio::test]
    async fn an_up_to_date_database_is_not_copied_again() {
        let directory = TempDir::new().unwrap();
        let path = database_path(&directory);

        let mut first = open_writer_connection(&path).await.unwrap();
        run(&mut first, &path, &MIGRATOR).await.unwrap();
        assert!(
            copies(&directory).await.is_empty(),
            "a first install has no user data to copy"
        );
        first.close().await.unwrap();

        let mut second = open_writer_connection(&path).await.unwrap();
        run(&mut second, &path, &MIGRATOR).await.unwrap();

        assert_eq!(
            copies(&directory).await,
            Vec::<String>::new(),
            "an up-to-date database is not copied"
        );
    }

    /// The permissions edge case: a database that cannot be copied is not migrated, because
    /// the copy is the only way back from an upgrade.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_database_that_cannot_be_copied_is_not_migrated() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = TempDir::new().unwrap();
        let mut connection = migrated_up_to_0007(&directory).await;
        let path = database_path(&directory);

        let original = tokio::fs::metadata(directory.path())
            .await
            .unwrap()
            .permissions();
        tokio::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o500))
            .await
            .unwrap();

        let error = apply(&mut connection, &path)
            .await
            .expect_err("the copy cannot be written into a read-only directory");

        // Restore before asserting, or a failure here leaves the temporary directory
        // undeletable.
        tokio::fs::set_permissions(directory.path(), original)
            .await
            .unwrap();

        assert!(
            matches!(error, StorageError::Backup { .. }),
            "expected a typed backup failure, got {error:?}"
        );
        assert!(
            error.to_string().contains("free some space"),
            "the error must say what to do: {error}"
        );
        assert_eq!(
            versions(&mut connection).await,
            vec![1, 2, 3, 4, 5, 6, 7],
            "the upgrade must not have run without a copy"
        );
    }
}
