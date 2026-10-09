//! The PRAGMA set that every connection must report.
//!
//! [`DATA_MODEL` §5](../../../docs/DATA_MODEL.md) requires `journal_mode=WAL`,
//! `synchronous=NORMAL`, `foreign_keys=ON`, `busy_timeout=5000`, `temp_store=MEMORY`,
//! `mmap_size=256MB`, and `page_size=8192` "at open time on every connection".
//!
//! [`apply_to`] puts them in the connection options, so the driver sends them on each
//! connection it establishes; [`enforce`] then reads them back and repairs any that did
//! not take. The read-back is not belt-and-braces: `page_size` cannot change once an
//! existing database is in WAL mode, so a mismatch is possible and must be handled
//! rather than assumed away.
//!
//! Each pragma is either required or best-effort. Required means the schema's own
//! constraints or the reader/writer model depends on it, so a connection that cannot
//! report it must be discarded. Best-effort means a performance or storage setting that a
//! database created before this policy existed may genuinely be unable to adopt:
//! `page_size` on an existing WAL database is the real case, and refusing to open the
//! user's library over it would be worse than running with the page size it already has.

use std::path::Path;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection, SqliteJournalMode, SqliteSynchronous};
use tracing::warn;

use crate::error::StorageError;

/// `busy_timeout`, in milliseconds, from `DATA_MODEL` §5 and `ARCHITECTURE` §10.
pub(crate) const BUSY_TIMEOUT_MILLIS: u64 = 5_000;

/// 256 MiB, the documented `mmap_size`.
const MMAP_SIZE_BYTES: i64 = 256 * 1024 * 1024;

/// 8192 bytes, the documented `page_size`.
const PAGE_SIZE_BYTES: i64 = 8_192;

/// How much a pragma matters to correctness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Criticality {
    /// The schema's constraints or the reader/writer model depend on it.
    Required,
    /// A performance or storage setting, where the database's own value wins.
    BestEffort,
}

/// The value a pragma is documented to report.
#[derive(Debug, Clone, Copy)]
enum Expected {
    Text(&'static str),
    Integer(i64),
}

impl Expected {
    /// The documented value, rendered for a log field or an error.
    fn render(self) -> String {
        match self {
            Self::Text(value) => value.to_owned(),
            Self::Integer(value) => value.to_string(),
        }
    }
}

/// One PRAGMA in the documented set.
struct DocumentedPragma {
    /// The pragma's name, as it appears in logs and errors.
    name: &'static str,
    /// The statement that reads the pragma back.
    query: &'static str,
    /// What the pragma must report after it is applied.
    expected: Expected,
    /// The statement that applies the pragma.
    set: &'static str,
    /// What a connection that cannot report it must do.
    criticality: Criticality,
}

/// `DATA_MODEL` §5's set, in the order `SQLite` requires: `page_size` only takes effect
/// before the database enters WAL mode, so it is applied first.
const DOCUMENTED_PRAGMAS: [DocumentedPragma; 7] = [
    DocumentedPragma {
        name: "page_size",
        query: "PRAGMA page_size",
        expected: Expected::Integer(PAGE_SIZE_BYTES),
        set: "PRAGMA page_size = 8192",
        criticality: Criticality::BestEffort,
    },
    DocumentedPragma {
        name: "journal_mode",
        query: "PRAGMA journal_mode",
        expected: Expected::Text("wal"),
        set: "PRAGMA journal_mode = WAL",
        // Without WAL the reader pool would block the writer, and `SQLITE_BUSY` would
        // become the normal outcome of two concurrent readers instead of a fault.
        criticality: Criticality::Required,
    },
    DocumentedPragma {
        name: "synchronous",
        query: "PRAGMA synchronous",
        expected: Expected::Integer(1),
        set: "PRAGMA synchronous = NORMAL",
        criticality: Criticality::Required,
    },
    DocumentedPragma {
        name: "foreign_keys",
        query: "PRAGMA foreign_keys",
        expected: Expected::Integer(1),
        set: "PRAGMA foreign_keys = ON",
        // Every `ON DELETE CASCADE` and every cross-table invariant in DATA_MODEL §3 is
        // the database's to enforce; a connection without this silently stops enforcing
        // them. `PRAGMA foreign_keys` is also a no-op inside a transaction, so the
        // failure is silent by nature and must be caught by reading it back.
        criticality: Criticality::Required,
    },
    DocumentedPragma {
        name: "busy_timeout",
        query: "PRAGMA busy_timeout",
        expected: Expected::Integer(5_000),
        set: "PRAGMA busy_timeout = 5000",
        criticality: Criticality::Required,
    },
    DocumentedPragma {
        name: "temp_store",
        query: "PRAGMA temp_store",
        expected: Expected::Integer(2),
        set: "PRAGMA temp_store = MEMORY",
        criticality: Criticality::BestEffort,
    },
    DocumentedPragma {
        name: "mmap_size",
        query: "PRAGMA mmap_size",
        expected: Expected::Integer(MMAP_SIZE_BYTES),
        set: "PRAGMA mmap_size = 268435456",
        criticality: Criticality::BestEffort,
    },
];

/// Adds the documented pragma set to connection options, so the driver applies it to
/// every connection it opens from them.
pub(crate) fn apply_to(options: SqliteConnectOptions) -> SqliteConnectOptions {
    options
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_millis(BUSY_TIMEOUT_MILLIS))
        .pragma("temp_store", "MEMORY")
        .pragma("page_size", PAGE_SIZE_BYTES.to_string())
        .pragma("mmap_size", MMAP_SIZE_BYTES.to_string())
}

/// Reads every documented pragma back and sets any the connection does not report.
///
/// Every mismatch is logged, whether or not it can be repaired: a durability or locking
/// setting that quietly points the other way is a support case nobody can diagnose.
///
/// # Errors
///
/// Returns [`StorageError::Pragma`] when a *required* pragma is still not in force after
/// the repair attempt, and the caller must then discard the connection rather than use it.
/// A best-effort pragma that cannot be applied is logged and does not fail the connection.
pub(crate) async fn enforce(
    connection: &mut SqliteConnection,
    path: &Path,
) -> Result<(), StorageError> {
    for pragma in &DOCUMENTED_PRAGMAS {
        let Some(found) = mismatch(connection, pragma)
            .await
            .map_err(StorageError::from_sqlx)?
        else {
            continue;
        };
        warn!(
            pragma = pragma.name,
            expected = %pragma.expected.render(),
            found = %found,
            "connection opened without a documented pragma; setting it now"
        );
        sqlx::query(pragma.set)
            .execute(&mut *connection)
            .await
            .map_err(StorageError::from_sqlx)?;

        if let Some(found) = mismatch(connection, pragma)
            .await
            .map_err(StorageError::from_sqlx)?
        {
            if pragma.criticality == Criticality::Required {
                return Err(StorageError::Pragma {
                    path: path.to_path_buf(),
                    pragma: pragma.name,
                    expected: pragma.expected.render(),
                    found,
                });
            }
            warn!(
                pragma = pragma.name,
                expected = %pragma.expected.render(),
                found = %found,
                "a best-effort pragma could not be applied; the database runs with the value it reports"
            );
        }
    }
    Ok(())
}

/// Reads the pragma, returning the value it reports only when that is not the
/// documented one.
async fn mismatch(
    connection: &mut SqliteConnection,
    pragma: &DocumentedPragma,
) -> Result<Option<String>, sqlx::Error> {
    match pragma.expected {
        Expected::Text(expected) => {
            let actual: String = sqlx::query_scalar(pragma.query)
                .fetch_one(&mut *connection)
                .await?;
            Ok((!expected.eq_ignore_ascii_case(&actual)).then_some(actual))
        }
        Expected::Integer(expected) => {
            let actual: i64 = sqlx::query_scalar(pragma.query)
                .fetch_one(&mut *connection)
                .await?;
            Ok((actual != expected).then_some(actual.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{Database, open_reader_pool};
    use sqlx::ConnectOptions;
    use tempfile::TempDir;

    /// The pool is the read path, and a pool hands out a fresh connection when the
    /// previous one is still checked out, which is what makes this a check of *every*
    /// connection rather than of the first one.
    #[tokio::test]
    async fn two_pooled_connections_report_every_documented_pragma() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");
        let _database = Database::open(&path).await.unwrap();
        let pool = open_reader_pool(&path).await.unwrap();

        let mut first = pool.acquire().await.unwrap();
        let mut second = pool.acquire().await.unwrap();

        for connection in [&mut *first, &mut *second] {
            for pragma in &DOCUMENTED_PRAGMAS {
                let actual = mismatch(connection, pragma).await.unwrap();
                assert_eq!(
                    actual,
                    None,
                    "a pooled connection must report {} = {}",
                    pragma.name,
                    pragma.expected.render()
                );
            }
        }
    }

    /// A connection opened without the documented options must be brought into line, and
    /// the pragmas the driver sets by default must be left alone.
    #[tokio::test]
    async fn enforce_repairs_a_connection_opened_without_the_documented_options() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("bare.sqlite");

        let mut connection = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .connect()
            .await
            .unwrap();
        // Guard the premise: without this the test could pass because the driver already
        // did the work, and would stop proving that `enforce` repairs anything.
        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&mut connection)
            .await
            .unwrap();
        assert_ne!(
            journal_mode, "wal",
            "the bare connection must not already be in WAL"
        );

        enforce(&mut connection, &path).await.unwrap();

        for pragma in &DOCUMENTED_PRAGMAS {
            assert_eq!(
                mismatch(&mut connection, pragma).await.unwrap(),
                None,
                "enforce must leave {} = {}",
                pragma.name,
                pragma.expected.render()
            );
        }
    }

    /// The integrity-critical half of the policy: a required pragma that the database will
    /// not adopt must fail the connection, not warn and continue.
    #[tokio::test]
    async fn a_required_pragma_that_cannot_be_applied_fails_the_connection() {
        // An in-memory database cannot be in WAL mode; `PRAGMA journal_mode = WAL`
        // reports `memory` and changes nothing, which is exactly the silent failure the
        // read-back exists to catch.
        let mut connection = SqliteConnectOptions::new()
            .in_memory(true)
            .connect()
            .await
            .unwrap();

        let error = enforce(&mut connection, Path::new(":memory:"))
            .await
            .expect_err("a connection that cannot be in WAL must not be used");

        assert!(
            matches!(
                error,
                StorageError::Pragma {
                    pragma: "journal_mode",
                    ..
                }
            ),
            "expected a typed pragma failure, got {error:?}"
        );
    }

    /// The other half: a database that already exists cannot change its page size in WAL
    /// mode, and that must not stop the user's library from opening.
    #[tokio::test]
    async fn a_best_effort_pragma_that_cannot_be_applied_is_not_fatal() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("older.sqlite");

        let mut connection = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .connect()
            .await
            .unwrap();
        let page_size: i64 = sqlx::query_scalar("PRAGMA page_size")
            .fetch_one(&mut connection)
            .await
            .unwrap();
        assert_eq!(page_size, 4096, "the premise: SQLite's default page size");

        enforce(&mut connection, &path)
            .await
            .expect("a page size that cannot change must not fail the connection");

        let page_size: i64 = sqlx::query_scalar("PRAGMA page_size")
            .fetch_one(&mut connection)
            .await
            .unwrap();
        assert_eq!(
            page_size, 4096,
            "the database keeps the page size it was created with"
        );
    }

    /// The set is a transcription of `DATA_MODEL` §5; dropping or reordering a pragma here
    /// must fail loudly rather than silently narrowing the policy.
    #[test]
    fn the_documented_set_is_complete() {
        let rendered: Vec<(&str, String)> = DOCUMENTED_PRAGMAS
            .iter()
            .map(|pragma| (pragma.name, pragma.expected.render()))
            .collect();

        assert_eq!(
            rendered,
            vec![
                ("page_size", "8192".to_owned()),
                ("journal_mode", "wal".to_owned()),
                ("synchronous", "1".to_owned()),
                ("foreign_keys", "1".to_owned()),
                ("busy_timeout", "5000".to_owned()),
                ("temp_store", "2".to_owned()),
                ("mmap_size", "268435456".to_owned()),
            ]
        );
        assert_eq!(BUSY_TIMEOUT_MILLIS, 5_000);
    }
}
