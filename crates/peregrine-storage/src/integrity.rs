//! The boot-time integrity check and the recovery it leads to (ARCHITECTURE §15).
//!
//! `PRAGMA integrity_check` reads every page, so damage is found before a migration or a
//! query can act on it. The recovery is deliberately conservative about user data: the
//! damaged file is *moved aside*, never deleted, and the fact that its rows are gone is
//! reported as a required library rescan rather than hidden.
//!
//! The check runs on its own connection, opened before the documented PRAGMAs are
//! enforced. A file that is not a database cannot report a `journal_mode`, so the pragma
//! check would report a setting it cannot repair instead of the corruption that caused it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use sqlx::ConnectOptions as _;
use sqlx::SqliteConnection;
use sqlx::sqlite::SqliteConnectOptions;
use tracing::info;

use crate::error::StorageError;

/// `SQLite`'s primary result code for a file whose pages are malformed.
const SQLITE_CORRUPT: i32 = 11;

/// `SQLite`'s primary result code for a file that is not a database.
const SQLITE_NOTADB: i32 = 26;

/// The suffix the write-ahead log carries beside the database file.
const WAL_SUFFIX: &str = "-wal";

/// The suffix the write-ahead index carries beside the database file.
const SHM_SUFFIX: &str = "-shm";

/// What the boot-time integrity check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recovery {
    /// The file passed `PRAGMA integrity_check`. Nothing was written and nothing was moved.
    Intact,
    /// The file failed the check: it was moved to `quarantine` and a clean schema was
    /// created in its place, so the library has to be rebuilt by scanning.
    Rebuilt {
        /// Where the damaged file and its write-ahead files were kept. Never deleted.
        quarantine: PathBuf,
    },
}

impl Recovery {
    /// Whether the library has to be rebuilt by scanning (ARCHITECTURE §15).
    #[must_use]
    pub fn requires_rescan(&self) -> bool {
        matches!(self, Self::Rebuilt { .. })
    }
}

/// Opens the connection the integrity check reads the file through.
///
/// The documented pragma set is deliberately not applied here; see the module comment. The
/// busy timeout is, so a database another process is holding is reported as a lock rather
/// than as damage.
pub(crate) async fn probe(path: &Path) -> Result<SqliteConnection, StorageError> {
    SqliteConnectOptions::new()
        .filename(path)
        .busy_timeout(Duration::from_millis(crate::pragmas::BUSY_TIMEOUT_MILLIS))
        .connect()
        .await
        .map_err(|error| StorageError::open(path, error))
}

/// Runs `PRAGMA integrity_check`, answering with the damage when there is any.
///
/// `Ok(None)` means the file is sound. `Ok(Some(reason))` means it is not, and `reason` is
/// what the log records; both a reported problem and a `SQLITE_CORRUPT`/`SQLITE_NOTADB`
/// failure count, because a file that is not a database at all fails the pragma instead of
/// answering it.
pub(crate) async fn check(
    connection: &mut SqliteConnection,
) -> Result<Option<String>, StorageError> {
    match sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
        .fetch_all(&mut *connection)
        .await
    {
        Ok(reported) if intact(&reported) => Ok(None),
        Ok(reported) => Ok(Some(summarise(&reported))),
        // Only the two corruption codes are damage. `SQLITE_CANTOPEN` is a permissions or
        // missing-file problem and must reach the caller as itself, or a database the OS
        // will not let us read would be quarantined and rebuilt.
        Err(error) => match malformed_code(&error) {
            Some(code) => Ok(Some(format!("SQLite error {code}: {error}"))),
            None => Err(StorageError::from_sqlx(error)),
        },
    }
}

/// Moves a damaged database and its write-ahead files aside, answering where they went.
///
/// The write-ahead files go first, so a failure leaves the database file where it was and
/// the next boot retries the whole move. The database file itself is moved last because the
/// new, clean database is created at that path.
pub(crate) async fn quarantine(path: &Path) -> Result<PathBuf, StorageError> {
    let destination = free_quarantine_path(path).await?;
    for suffix in [SHM_SUFFIX, WAL_SUFFIX] {
        move_aside(
            &with_suffix(path, suffix),
            &with_suffix(&destination, suffix),
        )
        .await?;
    }
    move_aside(path, &destination).await?;
    info!(
        path = %path.display(),
        quarantine = %destination.display(),
        "moved the damaged database aside; it is kept, not deleted"
    );
    Ok(destination)
}

/// Whether `PRAGMA integrity_check` reported the file sound.
///
/// Anything other than a single `ok` row is damage, including the empty result SQLite
/// cannot produce: a check that answers nothing has not proved anything.
fn intact(reported: &[String]) -> bool {
    matches!(reported, [only] if only == "ok")
}

/// The reported problems, bounded so a badly damaged file cannot write an unbounded log line.
fn summarise(reported: &[String]) -> String {
    match reported.split_first() {
        Some((first, [])) => first.clone(),
        Some((first, rest)) => format!("{first} (and {} more)", rest.len()),
        None => "integrity_check reported no rows".to_owned(),
    }
}

/// The extended result code when the driver reported corruption.
fn malformed_code(error: &sqlx::Error) -> Option<i32> {
    let sqlx::Error::Database(database) = error else {
        return None;
    };
    let code = database.code()?.parse::<i32>().ok()?;
    // The code is the *extended* result code, so `SQLITE_CORRUPT_VTAB` (267) and
    // `SQLITE_CORRUPT_INDEX` (779) are compared by their low byte.
    matches!(code % 256, SQLITE_CORRUPT | SQLITE_NOTADB).then_some(code)
}

/// A quarantine path no earlier one uses, counted from one.
///
/// A name whose existence cannot be established counts as taken and fails the move rather
/// than being skipped: mistaking a file that is there for one that is not would overwrite a
/// quarantined copy, which is the one thing this path must never do.
async fn free_quarantine_path(path: &Path) -> Result<PathBuf, StorageError> {
    for index in 1_u32.. {
        let candidate = with_suffix(path, &format!(".corrupt-{index}"));
        let mut taken = false;
        for suffix in ["", SHM_SUFFIX, WAL_SUFFIX] {
            taken |= tokio::fs::try_exists(with_suffix(&candidate, suffix))
                .await
                .map_err(|error| StorageError::Quarantine {
                    path: path.to_path_buf(),
                    source: Box::new(error),
                })?;
        }
        if !taken {
            return Ok(candidate);
        }
    }
    // The loop only ends when a free name is found; `u32` has no such run in a directory.
    unreachable!("every index up to u32::MAX is taken")
}

/// `path` with `suffix` appended to its last component.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Renames one file, treating an absent source as nothing to move.
async fn move_aside(source: &Path, destination: &Path) -> Result<(), StorageError> {
    match tokio::fs::rename(source, destination).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(StorageError::Quarantine {
            path: source.to_path_buf(),
            source: Box::new(std::io::Error::other(format!(
                "cannot move {} to {}: {error}",
                source.display(),
                destination.display()
            ))),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn only_a_single_ok_row_is_sound() {
        assert!(intact(&["ok".to_owned()]));
        assert!(
            !intact(&["ok".to_owned(), "ok".to_owned()]),
            "more than one row is not what a sound check reports"
        );
        assert!(
            !intact(&[]),
            "a check that reported nothing has proved nothing"
        );
        assert!(!intact(&["*** in database main ***".to_owned()]));
    }

    #[test]
    fn a_report_is_summarised_without_repeating_every_line() {
        assert_eq!(summarise(&["ok".to_owned()]), "ok");
        assert_eq!(
            summarise(&[
                "*** in database main ***".to_owned(),
                "Page 4 is never used".to_owned(),
            ]),
            "*** in database main *** (and 1 more)"
        );
        assert_eq!(summarise(&[]), "integrity_check reported no rows");
    }

    #[tokio::test]
    async fn quarantine_keeps_the_damaged_file_and_its_write_ahead_log_together() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");
        tokio::fs::write(&path, b"not a database").await.unwrap();
        tokio::fs::write(with_suffix(&path, WAL_SUFFIX), b"wal bytes")
            .await
            .unwrap();

        let quarantine = quarantine(&path).await.unwrap();

        assert_eq!(quarantine, with_suffix(&path, ".corrupt-1"));
        assert!(!path.exists(), "the damaged database is moved off its path");
        assert_eq!(
            tokio::fs::read(&quarantine).await.unwrap(),
            b"not a database",
            "the damaged file is kept byte for byte"
        );
        assert_eq!(
            tokio::fs::read(with_suffix(&quarantine, WAL_SUFFIX))
                .await
                .unwrap(),
            b"wal bytes"
        );
    }

    #[tokio::test]
    async fn a_second_quarantine_does_not_overwrite_the_first() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");
        tokio::fs::write(&path, b"first").await.unwrap();
        let first = quarantine(&path).await.unwrap();

        tokio::fs::write(&path, b"second").await.unwrap();
        let second = quarantine(&path).await.unwrap();

        assert_ne!(first, second);
        assert_eq!(tokio::fs::read(first).await.unwrap(), b"first");
        assert_eq!(tokio::fs::read(second).await.unwrap(), b"second");
    }

    /// A database that is not there at all has no write-ahead files either, and moving an
    /// absent file is not a failure: the caller may be racing a cleanup.
    #[tokio::test]
    async fn quarantine_tolerates_absent_write_ahead_files() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");
        tokio::fs::write(&path, b"garbage").await.unwrap();

        quarantine(&path).await.unwrap();

        assert!(
            !with_suffix(&path, SHM_SUFFIX).exists(),
            "an absent index file is nothing to move"
        );
    }
}
