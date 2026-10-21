//! Copies of the database: the pre-migration rollback and the user's own backup file.
//!
//! `DATA_MODEL` §5 requires a copy of the database beside it before a migration, rotated to
//! keep the last three, and this is the rollback path the migration gate relies on: rolling
//! a schema change back is not supported, so the copy *is* the way back.
//!
//! The copy is taken with `VACUUM INTO`, which is the only way to get one consistent file
//! out of a `WAL` database through a connection that is already open; a byte copy of the
//! file would miss whatever is still only in the write-ahead log. It is written under a
//! `.partial` name and renamed once complete, so a crash cannot leave a half-written file
//! under a name that reads like a backup (`DATA_MODEL` §5, "crash-atomic writes").
//!
//! A user's backup is the same copy with the tables that cannot survive the trip removed
//! ([ADR 0033](../../../docs/adr/0033-account-backup-and-restore.md)): cached bytes, the
//! scheduler's journal, the acquisition queue, paired sessions, and the audit log are all
//! about *this* process and *this* machine. Profiles, wishlists, watch progress, settings,
//! the library index, and the catalogue remain. Credentials were never in the database, and
//! `secret_ref` rows only name keyring entries, so a restored installation on the same
//! machine keeps working and one on another machine asks for its keys again.

use std::path::{Path, PathBuf};

use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions as _, Connection as _, SqliteConnection, SqlitePool};
use tracing::{info, warn};

use crate::error::StorageError;

/// How many pre-migration copies are kept, newest first, per `DATA_MODEL` §5.
const KEPT: usize = 3;

/// What separates a copy's name from the database's.
const MARKER: &str = ".backup-";

/// What a copy is named with while `VACUUM INTO` is writing it.
const PARTIAL: &str = ".partial";

/// The tables a user's backup does not carry.
///
/// Each is about the process or the machine that wrote it, not the household's data: the
/// scheduler's journal, the acquisition queue and its audit trail, the provider and image
/// caches, casting and LAN sessions, and the diagnostics log. A restored process writes its
/// own, and carrying these over would either claim work that no longer exists or restore a
/// session that can never be used.
///
/// The order is the foreign-key order: a child is cleared before the parent it references.
const NOT_IN_A_BACKUP: [&str; 12] = [
    "DELETE FROM download_file",
    "DELETE FROM release_decision",
    "DELETE FROM download_job",
    "DELETE FROM job",
    "DELETE FROM provider_cache",
    "DELETE FROM image_cache",
    "DELETE FROM dataset_state",
    "DELETE FROM cast_session",
    "DELETE FROM device",
    "DELETE FROM web_audit",
    "DELETE FROM paired_device",
    "DELETE FROM event_log",
];

/// The tables a restore candidate must hold before it is treated as a Peregrine database.
///
/// `_sqlx_migrations` is checked separately because its presence is what carries the schema
/// version, and the others are the four tables a database with no household data cannot
/// plausibly have.
const REQUIRED_TABLES: [&str; 4] = ["account", "settings", "title", "watchlist_entry"];

/// A backup that was just written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupReport {
    /// Where the file is.
    pub path: PathBuf,
    /// How large it is.
    pub bytes: u64,
    /// The newest migration the copy holds.
    pub schema_version: i64,
}

/// What inspection found in a file a user chose to restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackupInfo {
    /// The newest migration the candidate holds.
    pub schema_version: i64,
}

/// The steps of writing a backup, for the bar a person watches.
///
/// SQLite does not report how many pages `VACUUM INTO` has written, so this is phase
/// progress rather than byte progress: the bar moves when a step starts, and the sentence
/// beside it says which step. Each figure is a share of the whole, not a measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupPhase {
    /// `VACUUM INTO` is writing the copy.
    Copying,
    /// The process-local rows are being removed.
    Cleaning,
    /// The copy is being compacted and checkpointed.
    Compacting,
    /// The file is complete and renamed into place.
    Done,
}

impl BackupPhase {
    /// The stable spelling an event payload carries.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Copying => "copying",
            Self::Cleaning => "cleaning",
            Self::Compacting => "compacting",
            Self::Done => "done",
        }
    }

    /// How far through the whole the phase starts, in permille.
    #[must_use]
    pub const fn permille(self) -> u16 {
        match self {
            Self::Copying => 100,
            Self::Cleaning => 650,
            Self::Compacting => 900,
            Self::Done => 1_000,
        }
    }
}

/// Copies `path` to a new file beside it, then prunes the oldest beyond [`KEPT`].
///
/// `from_version` is the highest migration already applied, and it is part of the name:
/// versions move forward, so sorting the names sorts the copies by the order they were
/// taken, which is what makes rotation a name sort rather than a clock read (AGENTS R14).
pub(crate) async fn take(
    connection: &mut SqliteConnection,
    path: &Path,
    from_version: i64,
) -> Result<PathBuf, StorageError> {
    let destination = next_destination(path, from_version).await?;
    let partial = with_suffix(&destination, PARTIAL);
    // A copy killed part-way through leaves its partial file at the name this run wants,
    // and `VACUUM INTO` refuses a destination that exists, so the leftover goes first.
    if let Err(error) = tokio::fs::remove_file(&partial).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(backup_failed(
            path,
            format!(
                "the leftover copy at {} could not be removed: {error}",
                partial.display()
            ),
        ));
    }

    let Some(partial_path) = partial.to_str() else {
        return Err(backup_failed(
            path,
            format!("the copy's path {} is not valid UTF-8", partial.display()),
        ));
    };
    sqlx::query("VACUUM INTO ?1")
        .bind(partial_path)
        .execute(&mut *connection)
        .await
        .map_err(|error| backup_failed(path, format!("the copy could not be written: {error}")))?;

    tokio::fs::rename(&partial, &destination)
        .await
        .map_err(|error| {
            backup_failed(
                path,
                format!(
                    "the copy at {} could not be moved to {}: {error}",
                    partial.display(),
                    destination.display()
                ),
            )
        })?;

    info!(
        path = %path.display(),
        backup = %destination.display(),
        from_version,
        "copied the database before migrating it"
    );
    prune(path).await;
    Ok(destination)
}

/// The next free copy name for `from_version`.
///
/// The sequence only ever moves forward, because a failed migration retried later has the
/// same `from_version` and the second copy must not overwrite the first one the failure
/// report told the user to restore. A partial file is not a copy and does not hold a
/// sequence number: its bytes were never a complete database.
async fn next_destination(path: &Path, from_version: i64) -> Result<PathBuf, StorageError> {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return Err(backup_failed(
            path,
            "the database's file name is not valid UTF-8".to_owned(),
        ));
    };
    let prefix = format!("{file_name}{MARKER}{from_version:04}-");
    let directory = path.parent().unwrap_or_else(|| Path::new(""));

    let mut highest = 0_u32;
    if let Ok(mut entries) = tokio::fs::read_dir(directory).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Some(sequence) = name.strip_prefix(&prefix) else {
                continue;
            };
            if let Ok(sequence) = sequence.parse() {
                highest = highest.max(sequence);
            }
        }
    }

    Ok(directory.join(format!("{prefix}{:02}", highest + 1)))
}

/// Deletes every copy beyond the last [`KEPT`], oldest first.
///
/// Rotation is best-effort: a copy that cannot be removed is logged and the boot continues,
/// because refusing to open the library over a stale file in the data directory would trade
/// a working library for a tidy folder.
async fn prune(path: &Path) {
    let copies = existing(path).await;
    let stale = copies.len().saturating_sub(KEPT);
    let mut removed = 0;
    for copy in copies.iter().take(stale) {
        match tokio::fs::remove_file(copy).await {
            Ok(()) => removed += 1,
            Err(error) => warn!(
                backup = %copy.display(),
                %error,
                "an old pre-migration copy could not be removed"
            ),
        }
    }
    if removed > 0 {
        info!(
            removed,
            kept = KEPT,
            "pruned the oldest pre-migration copies"
        );
    }
}

/// Every complete copy of `path`, oldest first.
///
/// A partial file is not a copy: it is the remains of a copy that never finished, and
/// counting it would let rotation delete a usable copy in its place.
async fn existing(path: &Path) -> Vec<PathBuf> {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    let prefix = format!("{file_name}{MARKER}");
    let directory = path.parent().unwrap_or_else(|| Path::new(""));

    let mut copies = Vec::new();
    if let Ok(mut entries) = tokio::fs::read_dir(directory).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name.starts_with(&prefix) && !name.ends_with(PARTIAL) {
                copies.push(entry.path());
            }
        }
    }
    // The name carries a zero-padded version and sequence, so the order they were taken in
    // is the order they sort in.
    copies.sort();
    copies
}

/// The typed failure for anything that stops the copy being taken.
fn backup_failed(path: &Path, detail: String) -> StorageError {
    StorageError::Backup {
        path: path.to_path_buf(),
        source: Box::new(std::io::Error::other(detail)),
    }
}

/// `path` with `suffix` appended to its last component.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Writes one consistent copy of the database to `destination`.
///
/// `scrub` selects the user's backup (the transient tables in `NOT_IN_A_BACKUP` removed)
/// over an exact snapshot, which is what a restore safety copy wants.
///
/// The copy is written under `destination.partial` and renamed only once it is complete and,
/// when scrubbing, consistent again. A destination that already exists is refused rather than
/// overwritten: the caller names the file, and a name collision is the caller's to resolve.
pub(crate) async fn write(
    pool: &SqlitePool,
    destination: &Path,
    scrub: bool,
    progress: &(dyn Fn(BackupPhase) + Send + Sync),
) -> Result<BackupReport, StorageError> {
    if tokio::fs::try_exists(destination).await.unwrap_or(false) {
        return Err(backup_failed(
            destination,
            "that file already exists".to_owned(),
        ));
    }
    let partial = with_suffix(destination, PARTIAL);
    if let Err(error) = tokio::fs::remove_file(&partial).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(backup_failed(
            destination,
            format!(
                "the leftover copy at {} could not be removed: {error}",
                partial.display()
            ),
        ));
    }
    let Some(partial_path) = partial.to_str() else {
        return Err(backup_failed(
            destination,
            format!("the copy's path {} is not valid UTF-8", partial.display()),
        ));
    };

    progress(BackupPhase::Copying);
    let mut connection = pool.acquire().await.map_err(StorageError::from_sqlx)?;
    sqlx::query("VACUUM INTO ?1")
        .bind(partial_path)
        .execute(&mut *connection)
        .await
        .map_err(|error| {
            backup_failed(
                destination,
                format!("the copy could not be written: {error}"),
            )
        })?;
    drop(connection);

    if scrub {
        progress(BackupPhase::Cleaning);
        let result = remove_transient_tables(&partial, progress).await;
        if let Err(error) = result {
            let _ = tokio::fs::remove_file(&partial).await;
            return Err(error);
        }
    }

    let bytes = tokio::fs::metadata(&partial)
        .await
        .map_err(|error| {
            backup_failed(
                destination,
                format!("the copy could not be read back: {error}"),
            )
        })?
        .len();
    tokio::fs::rename(&partial, destination)
        .await
        .map_err(|error| {
            backup_failed(
                destination,
                format!(
                    "the copy at {} could not be moved to {}: {error}",
                    partial.display(),
                    destination.display()
                ),
            )
        })?;

    progress(BackupPhase::Done);
    info!(
        backup = %destination.display(),
        bytes,
        scrub,
        "wrote a database backup"
    );
    Ok(BackupReport {
        path: destination.to_path_buf(),
        bytes,
        schema_version: crate::migrations::latest_version(),
    })
}

/// Removes the transient tables from a copy, then reclaims their pages.
///
/// The deletes run in one transaction so a failure leaves the copy as VACUUM INTO wrote it;
/// VACUUM runs afterwards because SQLite refuses it inside a transaction.
async fn remove_transient_tables(
    path: &Path,
    progress: &(dyn Fn(BackupPhase) + Send + Sync),
) -> Result<(), StorageError> {
    let mut connection = crate::database::open_writer_connection(path).await?;
    let mut transaction = connection.begin().await.map_err(StorageError::from_sqlx)?;
    for statement in NOT_IN_A_BACKUP {
        sqlx::query(statement)
            .execute(&mut *transaction)
            .await
            .map_err(|error| {
                backup_failed(
                    path,
                    format!("the copy could not be cleared of transient rows: {error}"),
                )
            })?;
    }
    transaction
        .commit()
        .await
        .map_err(StorageError::from_sqlx)?;
    progress(BackupPhase::Compacting);
    sqlx::query("VACUUM")
        .execute(&mut connection)
        .await
        .map_err(|error| {
            backup_failed(path, format!("the copy could not be compacted: {error}"))
        })?;
    // The copy was opened in WAL mode, like every connection this crate makes, so the rows
    // that were just deleted may still live only in its write-ahead log. Renaming the main
    // file without this checkpoint would publish a backup that still has them.
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(&mut connection)
        .await
        .map_err(|error| {
            backup_failed(path, format!("the copy could not be checkpointed: {error}"))
        })?;
    drop(connection);
    for suffix in ["-wal", "-shm"] {
        let _ = tokio::fs::remove_file(with_suffix(path, suffix)).await;
    }
    Ok(())
}

/// Inspects a file a user chose to restore.
///
/// The answer is deliberately narrow: a schema version the restore path compares with this
/// build's, or a typed refusal. It never opens the candidate read-write and never runs a
/// migration, because a file from another installation is untrusted input (SECURITY.md).
pub async fn inspect(path: &Path) -> Result<BackupInfo, StorageError> {
    let mut connection = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .connect()
        .await
        .map_err(|error| invalid(path, format!("it could not be opened: {error}")))?;

    let report: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_all(&mut connection)
        .await
        .map_err(|error| invalid(path, format!("its integrity check did not run: {error}")))?;
    if report.as_slice() != ["ok"] {
        return Err(invalid(
            path,
            "its integrity check reported damage".to_owned(),
        ));
    }

    let tables: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table'")
            .fetch_all(&mut connection)
            .await
            .map_err(|error| invalid(path, format!("its schema could not be read: {error}")))?;
    if !tables.iter().any(|table| table == "_sqlx_migrations") {
        return Err(invalid(path, "it is not a Peregrine database".to_owned()));
    }
    for required in REQUIRED_TABLES {
        if !tables.iter().any(|table| table == required) {
            return Err(invalid(path, format!("it is missing the {required} table")));
        }
    }

    let version: Option<i64> = sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations")
        .fetch_one(&mut connection)
        .await
        .map_err(|error| {
            invalid(
                path,
                format!("its schema version could not be read: {error}"),
            )
        })?;
    let Some(version) = version else {
        return Err(invalid(path, "it holds no schema version".to_owned()));
    };
    let latest = crate::migrations::latest_version();
    if version > latest {
        return Err(invalid(
            path,
            "it was written by a newer version of Peregrine".to_owned(),
        ));
    }
    Ok(BackupInfo {
        schema_version: version,
    })
}

/// The typed refusal for a file that is not a backup this build can restore.
fn invalid(path: &Path, detail: String) -> StorageError {
    StorageError::InvalidBackup {
        path: path.to_path_buf(),
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Database;
    use crate::database::open_writer_connection;
    use crate::migrations;
    use jiff::Timestamp;
    use peregrine_domain::{AccountId, WatchlistEntry};
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    /// The migration versions a database records, which is how a copy is shown to be a
    /// copy of a migrated database.
    async fn versions(connection: &mut SqliteConnection) -> Vec<i64> {
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&mut *connection)
            .await
            .unwrap()
    }

    /// Takes one copy of a migrated database and returns the directory it lives in.
    async fn migrated(directory: &TempDir) -> (SqliteConnection, PathBuf) {
        let path = directory.path().join("peregrine.sqlite");
        let mut connection = open_writer_connection(&path).await.unwrap();
        migrations::apply(&mut connection, &path).await.unwrap();
        (connection, path)
    }

    /// The acceptance for the rotation half of PG-E3-13: taking more copies than the bound
    /// prunes the oldest and touches nothing else in the directory.
    #[tokio::test]
    async fn rotation_keeps_the_newest_copies_and_deletes_nothing_else() {
        let directory = TempDir::new().unwrap();
        // A file that shares the prefix but not the marker is somebody else's: rotation must
        // not treat it as a copy of this database.
        let unrelated = directory.path().join("peregrine.sqlite.backup");
        tokio::fs::write(&unrelated, b"not a backup").await.unwrap();
        let notes = directory.path().join("notes.txt");
        tokio::fs::write(&notes, b"user data").await.unwrap();

        let (mut connection, path) = migrated(&directory).await;
        let mut taken = Vec::new();
        for version in 1..=KEPT + 2 {
            taken.push(
                take(&mut connection, &path, i64::try_from(version).unwrap())
                    .await
                    .unwrap(),
            );
            // The database file is the thing being backed up; it must survive every prune.
            assert!(path.exists());
        }

        let copies = existing(&path).await;
        assert_eq!(
            copies.len(),
            KEPT,
            "the bound is how many copies are kept: {copies:?}"
        );
        assert_eq!(
            copies,
            taken[taken.len() - KEPT..].to_vec(),
            "the newest copies are the ones kept"
        );
        for pruned in &taken[..taken.len() - KEPT] {
            assert!(
                !pruned.exists(),
                "{} is older than the bound and must be pruned",
                pruned.display()
            );
        }
        assert_eq!(
            tokio::fs::read(&unrelated).await.unwrap(),
            b"not a backup",
            "a file without the copy marker must not be pruned"
        );
        assert_eq!(tokio::fs::read(&notes).await.unwrap(), b"user data");
    }

    /// A copy is a complete database: it opens, it holds the schema, and it holds the rows
    /// that were committed before it was taken.
    #[tokio::test]
    async fn a_copy_is_a_readable_database_holding_the_rows_from_before_the_migration() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");
        let mut connection = open_writer_connection(&path).await.unwrap();
        migrations::apply(&mut connection, &path).await.unwrap();
        sqlx::query("INSERT INTO event_log (at, kind) VALUES (7, 'test.before_backup')")
            .execute(&mut connection)
            .await
            .unwrap();

        let copy = take(&mut connection, &path, 8).await.unwrap();

        let mut restored = open_writer_connection(&copy).await.unwrap();
        let events: Vec<String> = sqlx::query_scalar("SELECT kind FROM event_log")
            .fetch_all(&mut restored)
            .await
            .unwrap();
        assert_eq!(events, vec!["test.before_backup".to_owned()]);
        assert!(
            !with_suffix(&copy, PARTIAL).exists(),
            "the partial name must not survive a completed copy"
        );
    }

    #[tokio::test]
    async fn the_second_copy_does_not_overwrite_the_first() {
        let directory = TempDir::new().unwrap();
        let (mut connection, path) = migrated(&directory).await;
        let first = take(&mut connection, &path, 8).await.unwrap();
        let second = take(&mut connection, &path, 8).await.unwrap();

        assert_ne!(first, second);
        assert!(first.exists() && second.exists());
    }

    /// A copy left half-written by a crash is not a copy: rotation must not count it as one
    /// and then delete a usable copy to stay inside the bound, and the next run reuses its
    /// name after removing the leftover, because `VACUUM INTO` refuses a destination that
    /// exists.
    #[tokio::test]
    async fn a_partial_file_is_not_a_copy_and_is_reused() {
        let directory = TempDir::new().unwrap();
        let (mut connection, path) = migrated(&directory).await;
        let leftover = with_suffix(&path, ".backup-0009-01.partial");
        tokio::fs::write(&leftover, b"half a copy").await.unwrap();

        assert_eq!(
            existing(&path).await,
            Vec::<PathBuf>::new(),
            "a partial file is not a copy"
        );

        let copy = take(&mut connection, &path, 9).await.unwrap();

        assert_eq!(copy, with_suffix(&path, ".backup-0009-01"));
        assert!(!leftover.exists(), "the leftover must not survive the run");
        assert!(copy.is_file());
        let mut restored = open_writer_connection(&copy).await.unwrap();
        // Every embedded migration, derived rather than counted by hand: the number this
        // asserted was the reason adding a migration used to break a backup test.
        let embedded = i64::try_from(crate::migrations::count()).unwrap();
        assert_eq!(
            versions(&mut restored).await,
            (1..=embedded).collect::<Vec<_>>()
        );
    }

    /// One database with one household row in every shape a backup has to keep, and one
    /// process-local row in every table a backup has to leave behind.
    async fn populated(directory: &TempDir) -> (Database, PathBuf, i64) {
        let path = directory.path().join("peregrine.sqlite");
        let database = Database::open(&path).await.unwrap();
        let account = database
            .create_profile(crate::NewAccount {
                name: "Ada".to_owned(),
                kind: "adult".to_owned(),
                avatar_kind: "initials".to_owned(),
                avatar_ref: None,
                rating_ceiling: None,
                curated_only: false,
                can_acquire: true,
                created_at: 1_700_000_000_000,
            })
            .await
            .unwrap();
        let title = database
            .insert_stub_title(crate::StubTitle {
                kind: peregrine_domain::catalogue::TitleKind::Movie,
                subtype: peregrine_domain::catalogue::TitleSubtype::Standard,
                name: "The Matrix".to_owned(),
                original_name: None,
                sort_name: "the matrix".to_owned(),
                at: 1_700_000_000_000,
            })
            .await
            .unwrap();
        database
            .add_to_watchlist(WatchlistEntry::new(
                AccountId::new(account).unwrap(),
                title,
                Timestamp::from_millisecond(1_700_000_000_000).unwrap(),
            ))
            .await
            .unwrap();

        let mut connection = open_writer_connection(&path).await.unwrap();
        sqlx::query("INSERT INTO job (kind, state, payload_json, created_at) VALUES ('test', 'queued', '{}', 1)")
            .execute(&mut connection)
            .await
            .unwrap();
        sqlx::query("INSERT INTO event_log (at, kind) VALUES (1, 'test.backup')")
            .execute(&mut connection)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO provider_cache (key, provider, resource, fetched_at, expires_at, schema_version, body, size_bytes)              VALUES ('cache', 'tmdb', 'movie', 1, 2, 1, X'00', 1)",
        )
        .execute(&mut connection)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO paired_device (name, token_hash, scopes, created_at)              VALUES ('TV', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'read', 1)",
        )
        .execute(&mut connection)
        .await
        .unwrap();
        // The audit is a child of the paired device. A backup that clears the parent first is
        // refused by the foreign key, which is how the shipped rc.4 failed on a real library.
        sqlx::query(
            "INSERT INTO web_audit (device_id, action, title_id, created_at)              SELECT id, 'acquire', ?1, 1 FROM paired_device LIMIT 1",
        )
        .bind(title)
        .execute(&mut connection)
        .await
        .unwrap();
        drop(connection);

        (database, path, account)
    }

    /// The acceptance for a user's backup: the household's rows survive, the rows that are
    /// about this process and this machine do not, and the file passes inspection.
    #[tokio::test]
    async fn a_user_backup_keeps_household_rows_and_drops_process_rows() {
        let directory = TempDir::new().unwrap();
        let (database, _source, account) = populated(&directory).await;
        let destination = directory.path().join("backup.sqlite");

        let report = database.write_backup(&destination, &|_| {}).await.unwrap();
        assert_eq!(report.path, destination);
        assert!(report.bytes > 0);
        assert_eq!(report.schema_version, migrations::latest_version());

        let mut copy = open_writer_connection(&destination).await.unwrap();
        let profile: String = sqlx::query_scalar("SELECT name FROM account WHERE id = ?1")
            .bind(account)
            .fetch_one(&mut copy)
            .await
            .unwrap();
        assert_eq!(profile, "Ada");
        let watchlist: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM watchlist_entry")
            .fetch_one(&mut copy)
            .await
            .unwrap();
        assert_eq!(watchlist, 1, "the wishlist is the point of the backup");

        for (table, count) in [
            ("job", "SELECT COUNT(*) FROM job"),
            ("event_log", "SELECT COUNT(*) FROM event_log"),
            ("provider_cache", "SELECT COUNT(*) FROM provider_cache"),
            ("web_audit", "SELECT COUNT(*) FROM web_audit"),
            ("paired_device", "SELECT COUNT(*) FROM paired_device"),
        ] {
            let rows: i64 = sqlx::query_scalar(count)
                .fetch_one(&mut copy)
                .await
                .unwrap();
            assert_eq!(rows, 0, "{table} must not travel in a backup");
        }
        drop(copy);

        let info = inspect(&destination).await.unwrap();
        assert_eq!(info.schema_version, migrations::latest_version());
    }

    /// The bar a screen draws follows the storage work: each phase is reported once, in the
    /// order the copy goes through them, and the final one says the file exists.
    #[tokio::test]
    async fn a_backup_reports_each_phase_once_in_order() {
        let directory = TempDir::new().unwrap();
        let (database, _source, _account) = populated(&directory).await;
        let destination = directory.path().join("backup.sqlite");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);

        database
            .write_backup(&destination, &move |phase| {
                sink.lock()
                    .expect("the phase log is not poisoned")
                    .push(phase);
            })
            .await
            .unwrap();

        assert_eq!(
            *seen.lock().expect("the phase log is not poisoned"),
            vec![
                BackupPhase::Copying,
                BackupPhase::Cleaning,
                BackupPhase::Compacting,
                BackupPhase::Done,
            ]
        );
    }

    /// A destination is the caller's, so a second backup never overwrites the first, and a
    /// refusal leaves no partial file behind.
    #[tokio::test]
    async fn a_backup_does_not_overwrite_an_existing_file() {
        let directory = TempDir::new().unwrap();
        let (database, _source, _account) = populated(&directory).await;
        let destination = directory.path().join("backup.sqlite");
        database.write_backup(&destination, &|_| {}).await.unwrap();

        let error = database
            .write_backup(&destination, &|_| {})
            .await
            .unwrap_err();
        assert!(matches!(error, StorageError::Backup { .. }));
        assert!(
            !with_suffix(&destination, PARTIAL).exists(),
            "the refusal must not leave a partial copy"
        );
    }

    /// Inspection is the boundary between a chosen file and a database: an ordinary SQLite
    /// file is refused, and so is a backup written by a newer schema.
    #[tokio::test]
    async fn inspection_refuses_a_plain_database_and_a_newer_schema() {
        let directory = TempDir::new().unwrap();
        let plain = directory.path().join("notes.sqlite");
        let mut connection = open_writer_connection(&plain).await.unwrap();
        sqlx::query("CREATE TABLE notes (body TEXT)")
            .execute(&mut connection)
            .await
            .unwrap();
        drop(connection);
        assert!(matches!(
            inspect(&plain).await,
            Err(StorageError::InvalidBackup { .. })
        ));

        let (database, _source, _account) = populated(&directory).await;
        let backup = directory.path().join("backup.sqlite");
        database.write_backup(&backup, &|_| {}).await.unwrap();
        let newer = directory.path().join("newer.sqlite");
        tokio::fs::copy(&backup, &newer).await.unwrap();
        let mut connection = open_writer_connection(&newer).await.unwrap();
        sqlx::query(
            "UPDATE _sqlx_migrations SET version = version + 100000              WHERE version = (SELECT MAX(version) FROM _sqlx_migrations)",
        )
        .execute(&mut connection)
        .await
        .unwrap();
        drop(connection);
        let error = inspect(&newer).await.unwrap_err();
        assert!(matches!(error, StorageError::InvalidBackup { .. }));
    }
}
