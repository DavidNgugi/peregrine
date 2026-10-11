//! The database handle: one writer task, a reader pool, and the open path.

use std::path::Path;

use sqlx::ConnectOptions;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection, SqlitePool, SqlitePoolOptions};
use tracing::info;

use crate::catalogue::{StubTitle, TitleExternalId, TitleIdentity};
use crate::error::StorageError;
use crate::jobs::{JobChange, JobRecord, NewJob};
use crate::page::PageRequest;
use crate::pragmas;
use crate::writer::{NewEvent, WriteRequest, WriteResponse, WriterHandle};

/// ARCHITECTURE §10: readers use a pool sized to `min(4, cores)`.
const READER_POOL_MAX: u32 = 4;

/// A handle to the one `SQLite` database.
///
/// Writes are applied by a task that owns the only write connection; reads are served
/// by a small pool, because WAL lets readers run while a writer holds the write lock.
#[derive(Debug)]
pub struct Database {
    writer: WriterHandle,
    readers: SqlitePool,
}

impl Database {
    /// Opens the database at `path`, creating it when it does not exist, applies the
    /// embedded migrations on the write connection, and starts the writer task.
    ///
    /// # Errors
    ///
    /// [`StorageError::Pragma`] means the write connection could not be brought to the
    /// documented PRAGMA set, so the database was left untouched and the caller should
    /// report the pragma rather than retry blindly.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let path = path.as_ref();
        // The path is the first thing a support question needs and is not a secret; this
        // crate never logs a setting or a credential.
        info!(path = %path.display(), "opening the database");

        let mut connection = open_writer_connection(path).await?;
        crate::migrations::apply(&mut connection)
            .await
            .map_err(StorageError::migrate)?;

        let writer = WriterHandle::spawn(connection);
        let readers = open_reader_pool(path)
            .await
            .map_err(|error| StorageError::open(path, error))?;

        Ok(Self { writer, readers })
    }

    /// Appends one row to `event_log` through the single writer.
    ///
    /// Returns the rowid `SQLite` assigned: only the writer task ever holds the write
    /// connection, so it is the only place that can observe it.
    pub async fn append_event(&self, event: NewEvent) -> Result<i64, StorageError> {
        match self.writer.send(WriteRequest::AppendEvent(event)).await? {
            WriteResponse::EventAppended { id } => Ok(id),
            other => Err(unexpected_response("append_event", &other)),
        }
    }

    /// Adds one job to the journal through the single writer.
    ///
    /// The row starts in `queued`, which is the phase the runtime reports before the
    /// scheduler has an attempt to run.
    pub async fn insert_job(&self, job: NewJob) -> Result<i64, StorageError> {
        match self
            .writer
            .send(WriteRequest::InsertJob(Box::new(job)))
            .await?
        {
            WriteResponse::JobInserted { id } => Ok(id),
            other => Err(unexpected_response("insert_job", &other)),
        }
    }

    /// Applies one change to a journal row, reporting whether the row was still there.
    ///
    /// `false` is not an error: a job that finished while a late progress report was in
    /// flight is the ordinary case, and the guards in `jobs::update` are what stop that
    /// report from resurrecting a terminal row.
    pub async fn update_job(&self, id: i64, change: JobChange) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::UpdateJob {
                id,
                change: Box::new(change),
            })
            .await?
        {
            WriteResponse::JobUpdated { changed } => Ok(changed),
            other => Err(unexpected_response("update_job", &other)),
        }
    }

    /// One journal row, or `None` when no such row exists.
    pub async fn job(&self, id: i64) -> Result<Option<JobRecord>, StorageError> {
        crate::jobs::get(&self.readers, id).await
    }

    /// A page of journal rows, newest first.
    pub async fn list_jobs(&self, request: PageRequest) -> Result<Vec<JobRecord>, StorageError> {
        crate::jobs::list(&self.readers, request).await
    }

    /// Inserts one title in its `stub` state and reports its row id.
    ///
    /// A stub is the identity a provider sync hangs its data off: the row exists, the name
    /// is known, and every field a provider fills is still NULL until that sync runs
    /// (PG-E5-06).
    pub async fn insert_stub_title(&self, stub: StubTitle) -> Result<i64, StorageError> {
        match self
            .writer
            .send(WriteRequest::InsertStubTitle(Box::new(stub)))
            .await?
        {
            WriteResponse::TitleInserted { id } => Ok(id),
            other => Err(unexpected_response("insert_stub_title", &other)),
        }
    }

    /// Attaches external ids to a title, refusing any that another title owns.
    ///
    /// # Errors
    ///
    /// [`StorageError::Conflict`] when an id in `ids` is already attached elsewhere; nothing
    /// in the call is applied in that case.
    pub async fn attach_external_ids(
        &self,
        title_id: i64,
        ids: &[TitleExternalId],
    ) -> Result<(), StorageError> {
        match self
            .writer
            .send(WriteRequest::AttachExternalIds {
                title_id,
                ids: ids.to_vec(),
            })
            .await?
        {
            WriteResponse::ExternalIdsAttached => Ok(()),
            other => Err(unexpected_response("attach_external_ids", &other)),
        }
    }

    /// One title's identity, or `None` when no such title exists.
    pub async fn title_identity(&self, id: i64) -> Result<Option<TitleIdentity>, StorageError> {
        crate::catalogue::identity(&self.readers, id).await
    }

    /// The title an external id belongs to, or `None` when no title has it.
    pub async fn title_for_external_id(
        &self,
        id: &peregrine_domain::ExternalId,
    ) -> Result<Option<i64>, StorageError> {
        crate::catalogue::title_for(&self.readers, id).await
    }

    /// Every external id attached to a title, in provider order.
    pub async fn external_ids_of(
        &self,
        title_id: i64,
    ) -> Result<Vec<TitleExternalId>, StorageError> {
        crate::catalogue::external_ids(&self.readers, title_id).await
    }

    /// How many titles the catalogue holds.
    pub async fn title_count(&self) -> Result<i64, StorageError> {
        crate::catalogue::title_count(&self.readers).await
    }

    /// How many rows `event_log` holds, read through the pool.
    ///
    /// This is the read path in miniature — a typed query on a pooled connection — and
    /// it is what makes "no write was lost" checkable from outside the writer.
    pub async fn event_count(&self) -> Result<u64, StorageError> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM event_log")
            .fetch_one(&self.readers)
            .await
            .map_err(StorageError::from_sqlx)?;
        // `COUNT(*)` cannot be negative; the conversion exists so a count cannot be
        // mistaken for the rowid `append_event` returns.
        Ok(u64::try_from(count).unwrap_or_default())
    }
}

/// The writer answered a request it was not sent.
///
/// Unreachable by construction — the handle sends one request and reads its own answer —
/// so a mismatch is an internal invariant violation rather than a database condition.
fn unexpected_response(operation: &'static str, response: &WriteResponse) -> StorageError {
    StorageError::UnexpectedValue {
        column: operation,
        found: format!("{response:?}"),
    }
}

/// Opens the connection the writer task will own, with the documented pragmas applied
/// and then verified, because an existing database cannot change every one of them.
///
/// A required pragma that the database will not report fails the open with
/// [`StorageError::Pragma`]; this is the connection every write runs on, so it is the one
/// whose pragmas actually decide whether the schema's constraints are enforced.
pub(crate) async fn open_writer_connection(path: &Path) -> Result<SqliteConnection, StorageError> {
    let mut connection = connection_options(path)
        .connect()
        .await
        .map_err(|error| StorageError::open(path, error))?;
    pragmas::enforce(&mut connection, path).await?;
    Ok(connection)
}

/// Opens the reader pool with the documented pragma set on every connection.
///
/// The options cover connections the pool opens later, and the `after_connect` hook
/// verifies them, so a connection that cannot report a required pragma is discarded by
/// the pool instead of serving reads under different pragmas. `sqlx` retries a connection
/// its `after_connect` hook rejects, so that path reaches the caller as `PoolTimedOut`
/// rather than as [`StorageError::Pragma`]; the writer connection checked in `open` is the
/// one that reports the typed failure.
pub(crate) async fn open_reader_pool(path: &Path) -> Result<SqlitePool, sqlx::Error> {
    let pragma_path = path.to_path_buf();
    SqlitePoolOptions::new()
        .max_connections(reader_pool_size())
        .after_connect(move |connection, _metadata| {
            // `after_connect` can only return a driver error, so the typed pragma failure
            // travels inside one.
            let pragma_path = pragma_path.clone();
            Box::pin(async move {
                pragmas::enforce(connection, &pragma_path)
                    .await
                    .map_err(|error| sqlx::Error::Configuration(Box::new(error)))
            })
        })
        .connect_with(connection_options(path))
        .await
}

/// `min(4, cores)`, per ARCHITECTURE §10.
fn reader_pool_size() -> u32 {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    u32::try_from(cores)
        .unwrap_or(READER_POOL_MAX)
        .min(READER_POOL_MAX)
}

fn connection_options(path: &Path) -> SqliteConnectOptions {
    pragmas::apply_to(
        SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use tempfile::TempDir;

    const WRITERS: i64 = 20;
    const WRITES_PER_WRITER: i64 = 5;

    async fn database(directory: &TempDir) -> Database {
        Database::open(directory.path().join("peregrine.sqlite"))
            .await
            .unwrap()
    }

    fn event(writer: i64, sequence: i64) -> NewEvent {
        NewEvent {
            // Injected, never the wall clock (AGENTS R14).
            at_millis: 1_700_000_000_000 + writer * WRITES_PER_WRITER + sequence,
            kind: "test.concurrent_write".to_owned(),
            subject_kind: Some("writer".to_owned()),
            subject_id: Some(writer),
            detail_json: None,
        }
    }

    /// PG-E3-02: twenty callers writing at once must serialize behind the single writer
    /// and every write must land.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn twenty_concurrent_writers_serialize_without_busy_or_lost_writes() {
        let directory = TempDir::new().unwrap();
        let database = Arc::new(database(&directory).await);

        let mut writers = Vec::new();
        for writer in 0..WRITERS {
            let database = Arc::clone(&database);
            writers.push(tokio::spawn(async move {
                let mut outcomes = Vec::new();
                for sequence in 0..WRITES_PER_WRITER {
                    outcomes.push(database.append_event(event(writer, sequence)).await);
                }
                outcomes
            }));
        }

        let mut ids = BTreeSet::new();
        let mut busy = 0_usize;
        let mut failures = Vec::new();
        for writer in writers {
            for outcome in writer.await.unwrap() {
                match outcome {
                    Ok(id) => {
                        ids.insert(id);
                    }
                    Err(error @ StorageError::Busy { .. }) => {
                        busy += 1;
                        failures.push(error.to_string());
                    }
                    Err(error) => failures.push(error.to_string()),
                }
            }
        }

        assert_eq!(busy, 0, "no writer may see SQLITE_BUSY: {failures:#?}");
        assert!(
            failures.is_empty(),
            "twenty writers must serialize through the single writer: {failures:#?}"
        );

        let expected = usize::try_from(WRITERS * WRITES_PER_WRITER).unwrap();
        assert_eq!(
            ids.len(),
            expected,
            "a lost write would reuse or skip a rowid"
        );
        assert_eq!(
            *ids.first().unwrap(),
            1,
            "SQLite numbers a fresh table's rowids from one"
        );
        assert_eq!(*ids.last().unwrap(), i64::try_from(expected).unwrap());

        let stored = database.event_count().await.unwrap();
        assert_eq!(
            usize::try_from(stored).unwrap(),
            expected,
            "every acknowledged write must be readable from the log"
        );
    }

    #[tokio::test]
    async fn the_reader_pool_is_sized_by_cores_within_the_documented_ceiling() {
        let directory = TempDir::new().unwrap();
        let pool = open_reader_pool(&directory.path().join("peregrine.sqlite"))
            .await
            .unwrap();

        let max_connections = pool.options().get_max_connections();
        assert!((1..=READER_POOL_MAX).contains(&max_connections));
        assert_eq!(max_connections, reader_pool_size());
    }

    #[tokio::test]
    async fn reopening_an_existing_database_keeps_the_writer_working() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");

        let first = Database::open(&path).await.unwrap();
        let first_id = first.append_event(event(0, 0)).await.unwrap();
        drop(first);

        let second = Database::open(&path).await.unwrap();
        let second_id = second.append_event(event(0, 1)).await.unwrap();

        assert_eq!(first_id, 1);
        assert_eq!(second_id, 2, "the reopened writer continues the log");
    }
}
