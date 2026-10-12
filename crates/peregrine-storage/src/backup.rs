//! The pre-migration copy of the database and its rotation.
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

use std::path::{Path, PathBuf};

use sqlx::SqliteConnection;
use tracing::{info, warn};

use crate::error::StorageError;

/// How many pre-migration copies are kept, newest first, per `DATA_MODEL` §5.
const KEPT: usize = 3;

/// What separates a copy's name from the database's.
const MARKER: &str = ".backup-";

/// What a copy is named with while `VACUUM INTO` is writing it.
const PARTIAL: &str = ".partial";

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::open_writer_connection;
    use crate::migrations;
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
        let leftover = with_suffix(&path, ".backup-0008-01.partial");
        tokio::fs::write(&leftover, b"half a copy").await.unwrap();

        assert_eq!(
            existing(&path).await,
            Vec::<PathBuf>::new(),
            "a partial file is not a copy"
        );

        let copy = take(&mut connection, &path, 8).await.unwrap();

        assert_eq!(copy, with_suffix(&path, ".backup-0008-01"));
        assert!(!leftover.exists(), "the leftover must not survive the run");
        assert!(copy.is_file());
        let mut restored = open_writer_connection(&copy).await.unwrap();
        assert_eq!(versions(&mut restored).await, (1..=8).collect::<Vec<_>>());
    }
}
