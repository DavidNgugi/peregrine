//! The crate's typed error.
//!
//! The variants never carry an `sqlx` type, so a caller of `peregrine-storage` cannot
//! be forced to name the driver, and a future driver swap stays inside this crate.

use std::error::Error as StdError;
use std::path::{Path, PathBuf};

/// The primary `SQLite` result code for a locked database.
const SQLITE_BUSY: i32 = 5;

/// Anything that can go wrong while opening or mutating the database.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// The database file could not be opened.
    #[error("cannot open the database at {}: {source}", path.display())]
    Open {
        /// The path that failed to open.
        path: PathBuf,
        /// The driver's error, boxed so `sqlx` stays crate-private.
        source: Box<dyn StdError + Send + Sync>,
    },

    /// The embedded migrations could not be applied.
    #[error("cannot apply the embedded migrations: {source}")]
    Migrate {
        /// The migration runner's error.
        source: Box<dyn StdError + Send + Sync>,
    },

    /// A statement failed for a reason other than lock contention.
    #[error("a database statement failed: {source}")]
    Statement {
        /// The driver's error.
        source: Box<dyn StdError + Send + Sync>,
    },

    /// The database stayed locked for longer than the `busy_timeout` pragma allows.
    ///
    /// Retrying is the caller's decision: the writer serialized its own `SQLITE_BUSY`
    /// away, so this only surfaces when something outside this process holds the lock.
    #[error("the database stayed locked for longer than the {busy_timeout_ms} ms busy timeout")]
    Busy {
        /// The `busy_timeout` in effect, in milliseconds.
        busy_timeout_ms: u64,
    },

    /// The single writer task is no longer running.
    #[error("the single-writer task has stopped; reopen the database before writing again")]
    WriterStopped,

    /// The write conflicts with a row that already exists and belongs to something else.
    ///
    /// Distinct from a statement failure because the caller can act on it: the user chose a
    /// release that is already attached elsewhere, or an id that another title owns, and the
    /// answer is to report which one rather than to retry.
    #[error("{detail}")]
    Conflict {
        /// What conflicts, phrased for a user.
        detail: String,
    },

    /// A stored value is outside the set the schema's `CHECK` constraints allow.
    ///
    /// This cannot come from a database this build wrote; it means the file was written
    /// by a different version. Reporting the column and the value is what makes that
    /// diagnosable, rather than a panic inside a row mapper.
    #[error("the column `{column}` holds the unexpected value `{found}`")]
    UnexpectedValue {
        /// The column that holds it.
        column: &'static str,
        /// What it held.
        found: String,
    },

    /// A connection cannot be brought to the documented PRAGMA set.
    ///
    /// Only the required pragmas produce this. The connection must be discarded: without
    /// `foreign_keys` the schema's own constraints stop being enforced, and without
    /// `journal_mode` the single-writer model loses the reader concurrency and crash
    /// safety it is built on.
    #[error(
        "the database at {} does not report `{pragma}` = {expected} (found: {found})",
        path.display()
    )]
    Pragma {
        /// The database file the connection was opened from.
        path: PathBuf,
        /// The pragma that is not in force.
        pragma: &'static str,
        /// The value `DOCUMENTED_PRAGMAS` requires.
        expected: String,
        /// The value the connection reports.
        found: String,
    },
}

impl StorageError {
    /// Wraps a failure to open or configure a connection.
    pub(crate) fn open(path: &Path, source: sqlx::Error) -> Self {
        Self::Open {
            path: path.to_path_buf(),
            source: Box::new(source),
        }
    }

    /// Wraps a failure to apply the embedded migrations.
    pub(crate) fn migrate(source: sqlx::migrate::MigrateError) -> Self {
        Self::Migrate {
            source: Box::new(source),
        }
    }

    /// Classifies a driver error, keeping lock contention distinguishable from a
    /// genuine statement failure so a caller can retry rather than fail.
    pub(crate) fn from_sqlx(source: sqlx::Error) -> Self {
        if is_busy(&source) {
            Self::Busy {
                busy_timeout_ms: crate::pragmas::BUSY_TIMEOUT_MILLIS,
            }
        } else {
            Self::Statement {
                source: Box::new(source),
            }
        }
    }
}

/// Whether the driver reported lock contention.
///
/// `code` is the *extended* result code, so `SQLITE_BUSY_RECOVERY` (261),
/// `SQLITE_BUSY_SNAPSHOT` (517), and `SQLITE_BUSY_TIMEOUT` (773) are compared by their
/// low byte rather than against the primary code alone.
fn is_busy(error: &sqlx::Error) -> bool {
    let sqlx::Error::Database(database) = error else {
        return false;
    };
    database
        .code()
        .and_then(|code| code.parse::<i32>().ok())
        .is_some_and(|code| code % 256 == SQLITE_BUSY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::ConnectOptions;
    use sqlx::sqlite::SqliteConnectOptions;
    use std::time::Duration;
    use tempfile::TempDir;

    async fn connection(path: &Path, busy_timeout: Duration) -> sqlx::SqliteConnection {
        crate::pragmas::apply_to(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true)
                .busy_timeout(busy_timeout),
        )
        .connect()
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_locked_database_is_reported_as_busy_not_as_a_statement_failure() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("locked.sqlite");

        let mut holder = connection(&path, Duration::from_millis(1)).await;
        sqlx::query("CREATE TABLE t (id INTEGER PRIMARY KEY)")
            .execute(&mut holder)
            .await
            .unwrap();
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut holder)
            .await
            .unwrap();

        // A second connection cannot take the write lock, and its busy timeout is one
        // millisecond so the failure arrives immediately rather than after five seconds.
        let mut contender = connection(&path, Duration::from_millis(1)).await;
        let error = sqlx::query("INSERT INTO t (id) VALUES (1)")
            .execute(&mut contender)
            .await
            .expect_err("the write lock is held by the first connection");

        assert!(
            matches!(
                StorageError::from_sqlx(error),
                StorageError::Busy {
                    busy_timeout_ms: crate::pragmas::BUSY_TIMEOUT_MILLIS
                }
            ),
            "SQLITE_BUSY must reach the caller as a distinguishable, retryable error"
        );
    }

    #[tokio::test]
    async fn opening_a_path_that_cannot_be_created_reports_open_not_a_panic() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("absent-directory").join("db.sqlite");

        let error = crate::database::Database::open(&path)
            .await
            .expect_err("a missing parent directory cannot hold a database");

        assert!(
            matches!(error, StorageError::Open { .. }),
            "expected an Open error, got {error:?}"
        );
        assert!(
            error.to_string().contains("absent-directory"),
            "the error must name the path that failed: {error}"
        );
    }
}
