//! The single write connection.
//!
//! `SQLite` permits one writer at a time, and ARCHITECTURE §5 turns that constraint into
//! the design: one task owns the write connection and every mutation arrives as a
//! message on one channel. Callers therefore cannot contend for the lock, and a write
//! is serialized behind the writer task instead of failing with `SQLITE_BUSY`.

use tokio::sync::{mpsc, oneshot};

use crate::error::StorageError;

/// How many requests may wait behind the one the writer is applying.
///
/// A single slot is enough because the writer is itself the serialization point: a
/// deeper queue shortens the wait for a caller but not the wait for the write, and it
/// hides backpressure from the caller that creates it.
const WRITE_QUEUE_DEPTH: usize = 1;

/// A mutation the single writer applies, in the order the requests arrive.
#[derive(Debug)]
pub(crate) enum WriteRequest {
    /// Append one row to `event_log`.
    AppendEvent(NewEvent),
}

/// The writer's answer to a [`WriteRequest`].
#[derive(Debug)]
pub(crate) enum WriteResponse {
    /// The rowid `SQLite` assigned to the appended `event_log` row.
    EventAppended { id: i64 },
}

/// A row to append to `event_log` (`DATA_MODEL` §2.8).
///
/// Every column is supplied rather than derived: the caller owns the clock, so a test
/// can write a deterministic timestamp and a replay can write the original one.
#[derive(Debug, Clone)]
pub struct NewEvent {
    /// Unix milliseconds UTC.
    pub at_millis: i64,
    /// Event kind, for example `media.added` or `download.failed`.
    pub kind: String,
    /// The kind of the subject the event is about, when it has one.
    pub subject_kind: Option<String>,
    /// The subject's row id, when the event has one.
    pub subject_id: Option<i64>,
    /// Pre-serialized JSON detail; never a secret (ARCHITECTURE §12).
    pub detail_json: Option<String>,
}

/// A cloneable handle to the writer task.
#[derive(Debug, Clone)]
pub(crate) struct WriterHandle {
    requests: mpsc::Sender<Envelope>,
}

/// A request together with the channel its answer comes back on.
#[derive(Debug)]
struct Envelope {
    request: WriteRequest,
    reply: oneshot::Sender<Result<WriteResponse, StorageError>>,
}

impl WriterHandle {
    /// Starts the writer task that owns `connection`.
    pub(crate) fn spawn(connection: sqlx::SqliteConnection) -> Self {
        let (requests, receiver) = mpsc::channel(WRITE_QUEUE_DEPTH);
        tokio::spawn(run(connection, receiver));
        Self { requests }
    }

    /// Applies one request and waits for its outcome.
    pub(crate) async fn send(&self, request: WriteRequest) -> Result<WriteResponse, StorageError> {
        let (reply, answer) = oneshot::channel();
        self.requests
            .send(Envelope { request, reply })
            .await
            .map_err(|_| StorageError::WriterStopped)?;
        answer.await.map_err(|_| StorageError::WriterStopped)?
    }
}

/// Applies requests one at a time until every handle is dropped.
async fn run(mut connection: sqlx::SqliteConnection, mut requests: mpsc::Receiver<Envelope>) {
    while let Some(envelope) = requests.recv().await {
        let outcome = apply(&mut connection, envelope.request).await;
        // A caller that gave up (a cancelled task dropping its oneshot receiver) must not
        // tear down the only write connection: the write already happened, and the next
        // caller still needs a writer.
        let _ = envelope.reply.send(outcome);
    }
}

/// Applies one request on the writer's connection.
async fn apply(
    connection: &mut sqlx::SqliteConnection,
    request: WriteRequest,
) -> Result<WriteResponse, StorageError> {
    match request {
        WriteRequest::AppendEvent(event) => append_event(connection, &event)
            .await
            .map(|id| WriteResponse::EventAppended { id })
            .map_err(StorageError::from_sqlx),
    }
}

/// Inserts one `event_log` row and reports its rowid.
async fn append_event(
    connection: &mut sqlx::SqliteConnection,
    event: &NewEvent,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO event_log (at, kind, subject_kind, subject_id, detail_json) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(event.at_millis)
    .bind(&event.kind)
    .bind(event.subject_kind.as_deref())
    .bind(event.subject_id)
    .bind(event.detail_json.as_deref())
    .execute(&mut *connection)
    .await?;
    Ok(result.last_insert_rowid())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::open_writer_connection;
    use crate::migrations;
    use tempfile::TempDir;

    fn event(sequence: i64) -> NewEvent {
        NewEvent {
            at_millis: 1_700_000_000_000 + sequence,
            kind: "test.dropped_reply".to_owned(),
            subject_kind: None,
            subject_id: None,
            detail_json: None,
        }
    }

    /// A caller that is cancelled while waiting for its answer must not take the writer
    /// with it: the write already happened, and the next caller still needs a writer.
    ///
    /// The default `#[tokio::test]` flavour is a single thread, so the writer task cannot
    /// run between the request being queued and the reply receiver being dropped, which
    /// makes the dropped-reply branch deterministic rather than a race.
    #[tokio::test]
    async fn a_dropped_reply_does_not_stop_the_writer() {
        let directory = TempDir::new().unwrap();
        let mut connection = open_writer_connection(&directory.path().join("peregrine.sqlite"))
            .await
            .unwrap();
        migrations::apply(&mut connection).await.unwrap();

        let (requests, receiver) = mpsc::channel(WRITE_QUEUE_DEPTH);
        let writer = tokio::spawn(run(connection, receiver));

        let (reply, answer) = oneshot::channel();
        requests
            .send(Envelope {
                request: WriteRequest::AppendEvent(event(0)),
                reply,
            })
            .await
            .unwrap();
        drop(answer);

        let handle = WriterHandle { requests };
        let response = handle
            .send(WriteRequest::AppendEvent(event(1)))
            .await
            .unwrap();
        let WriteResponse::EventAppended { id } = response;
        assert!(
            id >= 1,
            "the writer must still answer after a reply was dropped"
        );

        // The task exits when the last sender is dropped; awaiting it proves it was not
        // torn down by the failed reply.
        drop(handle);
        writer.await.unwrap();
    }

    #[tokio::test]
    async fn a_write_with_no_writer_reports_writer_stopped() {
        // The receiver is dropped, which is what a finished or panicked writer task
        // leaves behind; the caller must get an error rather than hang forever.
        let (requests, receiver) = mpsc::channel(WRITE_QUEUE_DEPTH);
        drop(receiver);
        let handle = WriterHandle { requests };

        let error = handle
            .send(WriteRequest::AppendEvent(NewEvent {
                at_millis: 0,
                kind: "test.no_writer".to_owned(),
                subject_kind: None,
                subject_id: None,
                detail_json: None,
            }))
            .await
            .expect_err("a closed channel has no writer behind it");

        assert!(
            matches!(error, StorageError::WriterStopped),
            "got {error:?}"
        );
    }
}
