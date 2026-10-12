//! The single write connection.
//!
//! `SQLite` permits one writer at a time, and ARCHITECTURE §5 turns that constraint into
//! the design: one task owns the write connection and every mutation arrives as a
//! message on one channel. Callers therefore cannot contend for the lock, and a write
//! is serialized behind the writer task instead of failing with `SQLITE_BUSY`.

use peregrine_domain::Event;
use sqlx::Connection as _;
use tokio::sync::{mpsc, oneshot};

use peregrine_domain::centre::{Delivery, NewCentreEntry, NotificationId, Recorded};

use crate::catalogue::{StubTitle, TitleExternalId};
use crate::error::StorageError;
use crate::jobs::{JobChange, NewJob};

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
    /// Insert one row into `job`.
    ///
    /// Boxed because the request travels through a channel sized at one slot, so the
    /// enum's size is the size of its largest variant, and a job row is much larger than
    /// the alternative.
    InsertJob(Box<NewJob>),
    /// Apply one change to a `job` row.
    UpdateJob { id: i64, change: Box<JobChange> },
    /// Insert one title in its `stub` state.
    InsertStubTitle(Box<StubTitle>),
    /// Attach external ids to one title.
    AttachExternalIds {
        title_id: i64,
        ids: Vec<TitleExternalId>,
    },
    /// Insert or refresh one `notification` row.
    ///
    /// The read-then-write of the dedupe window happens on the writer's own connection,
    /// so two events arriving at once cannot both decide they are the first.
    RecordNotification(Box<NewCentreEntry>),
    /// Apply one delivery outcome to a `notification` row.
    MarkNotificationDelivered {
        id: NotificationId,
        delivery: Box<Delivery>,
    },
    /// Rewrite one title's search index row from the source tables.
    ReindexTitle { title_id: i64 },
    /// Remove one title's search index row.
    RemoveTitleIndex { title_id: i64 },
    /// Rebuild the whole search index from the source tables.
    RebuildSearchIndex,
}

/// The writer's answer to a [`WriteRequest`].
#[derive(Debug)]
pub(crate) enum WriteResponse {
    /// The rowid `SQLite` assigned to the appended `event_log` row.
    EventAppended { id: i64 },
    /// The rowid `SQLite` assigned to the inserted `job` row.
    JobInserted { id: i64 },
    /// Whether a `job` row was there to change.
    JobUpdated { changed: bool },
    /// The rowid `SQLite` assigned to the inserted `title` row.
    TitleInserted { id: i64 },
    /// The external ids are attached.
    ExternalIdsAttached,
    /// The notification row was inserted, or refreshed inside its dedupe window.
    NotificationRecorded { recorded: Recorded },
    /// Whether a `notification` row was there to change.
    NotificationDelivered { changed: bool },
    /// Whether a search index row was written, which is `false` for an id no title holds.
    TitleReindexed { indexed: bool },
    /// Whether a search index row was removed.
    TitleIndexRemoved { removed: bool },
    /// How many titles the rebuild indexed.
    SearchIndexRebuilt { indexed: u64 },
}

/// A row to append to `event_log` (`DATA_MODEL` §2.8).
///
/// The timestamp and the detail are supplied rather than derived: the caller owns the clock, so a
/// test can write a deterministic timestamp and a replay can write the original one. The kind,
/// the subject kind, and the subject id are the one thing a caller cannot supply, because they
/// come from [`peregrine_domain::Event`]: the declared vocabulary is the only way to name an
/// event, so a repository cannot write a name this build does not know.
///
/// ```
/// # use peregrine_domain::Event;
/// # use peregrine_storage::NewEvent;
/// let stored = NewEvent {
///     at_millis: 0,
///     event: Event::DownloadFailed { job_id: 7 },
///     detail_json: None,
/// };
/// assert_eq!(stored.event.as_str(), "download.failed");
/// assert_eq!(stored.event.subject_id(), 7);
/// ```
///
/// ```compile_fail
/// # use peregrine_storage::NewEvent;
/// // A string kind is not a constructor: `NewEvent` has no `kind` field.
/// let _ = NewEvent {
///     at_millis: 0,
///     kind: "download.invented".to_owned(),
///     subject_kind: Some("download_job".to_owned()),
///     subject_id: Some(7),
///     detail_json: None,
/// };
/// ```
#[derive(Debug, Clone)]
pub struct NewEvent {
    /// Unix milliseconds UTC.
    pub at_millis: i64,
    /// The declared operation, which is also the row's kind and subject.
    pub event: Event,
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
        WriteRequest::InsertJob(job) => crate::jobs::insert(connection, &job)
            .await
            .map(|id| WriteResponse::JobInserted { id })
            .map_err(StorageError::from_sqlx),
        WriteRequest::UpdateJob { id, change } => crate::jobs::update(connection, id, &change)
            .await
            .map(|changed| WriteResponse::JobUpdated { changed })
            .map_err(StorageError::from_sqlx),
        WriteRequest::InsertStubTitle(stub) => crate::catalogue::insert_stub(connection, &stub)
            .await
            .map(|id| WriteResponse::TitleInserted { id }),
        WriteRequest::AttachExternalIds { title_id, ids } => {
            crate::catalogue::attach_external_ids(connection, title_id, &ids)
                .await
                .map(|()| WriteResponse::ExternalIdsAttached)
        }
        WriteRequest::RecordNotification(entry) => {
            crate::notify::insert_or_refresh(connection, &entry)
                .await
                .map(|recorded| WriteResponse::NotificationRecorded { recorded })
        }
        WriteRequest::MarkNotificationDelivered { id, delivery } => {
            crate::notify::apply_delivery(connection, id, &delivery)
                .await
                .map(|changed| WriteResponse::NotificationDelivered { changed })
        }
        WriteRequest::ReindexTitle { title_id } => {
            let mut transaction = connection.begin().await.map_err(StorageError::from_sqlx)?;
            let indexed = crate::fts::reindex(&mut transaction, title_id)
                .await
                .map_err(StorageError::from_sqlx)?;
            transaction
                .commit()
                .await
                .map_err(StorageError::from_sqlx)?;
            Ok(WriteResponse::TitleReindexed { indexed })
        }
        WriteRequest::RemoveTitleIndex { title_id } => {
            let mut transaction = connection.begin().await.map_err(StorageError::from_sqlx)?;
            let removed = crate::fts::unindex(&mut transaction, title_id)
                .await
                .map_err(StorageError::from_sqlx)?;
            transaction
                .commit()
                .await
                .map_err(StorageError::from_sqlx)?;
            Ok(WriteResponse::TitleIndexRemoved { removed })
        }
        // The rebuild runs on the writer's connection, never a second one: a rebuild that
        // raced the write path would interleave its `delete-all` with a title being indexed.
        WriteRequest::RebuildSearchIndex => crate::fts::rebuild(connection)
            .await
            .map(|indexed| WriteResponse::SearchIndexRebuilt { indexed })
            .map_err(StorageError::from_sqlx),
    }
}

/// Inserts one `event_log` row and reports its rowid.
async fn append_event(
    connection: &mut sqlx::SqliteConnection,
    new_event: &NewEvent,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO event_log (at, kind, subject_kind, subject_id, detail_json) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(new_event.at_millis)
    .bind(new_event.event.as_str())
    .bind(new_event.event.subject_kind().as_str())
    .bind(new_event.event.subject_id())
    .bind(new_event.detail_json.as_deref())
    .execute(&mut *connection)
    .await?;
    Ok(result.last_insert_rowid())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::open_writer_connection;
    use crate::migrations;
    use peregrine_domain::{SubjectKind, WatchSubject};
    use sqlx::Row as _;
    use tempfile::TempDir;

    /// A declared event, because the writer accepts nothing else (PG-E4-16); the sequence
    /// stands in for a job id so two rows are distinguishable.
    fn event(sequence: i64) -> NewEvent {
        NewEvent {
            at_millis: 1_700_000_000_000 + sequence,
            event: Event::DownloadFailed { job_id: sequence },
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
        let path = directory.path().join("peregrine.sqlite");
        let mut connection = open_writer_connection(&path).await.unwrap();
        migrations::apply(&mut connection, &path).await.unwrap();

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
        let WriteResponse::EventAppended { id } = response else {
            panic!("the writer answered `append_event` with another request's response");
        };
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
                event: Event::DownloadQueued { job_id: 1 },
                detail_json: None,
            }))
            .await
            .expect_err("a closed channel has no writer behind it");

        assert!(
            matches!(error, StorageError::WriterStopped),
            "got {error:?}"
        );
    }

    /// PG-E4-16: what the vocabulary declares is what the row holds and what a reader rebuilds.
    /// The row is read back through the three columns rather than through the caller's value, so
    /// the check covers the write path and the read path and not just the type.
    #[tokio::test]
    async fn an_appended_event_reads_back_as_the_declared_event() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");
        let mut connection = open_writer_connection(&path).await.unwrap();
        migrations::apply(&mut connection, &path).await.unwrap();

        let (requests, receiver) = mpsc::channel(WRITE_QUEUE_DEPTH);
        let writer = tokio::spawn(run(connection, receiver));
        let handle = WriterHandle { requests };

        let declared = Event::ProgressWatched {
            subject: WatchSubject::Episode(9),
        };
        let response = handle
            .send(WriteRequest::AppendEvent(NewEvent {
                at_millis: 1_700_000_000_000,
                event: declared,
                detail_json: Some("{\"percent\":0.97}".to_owned()),
            }))
            .await
            .unwrap();
        let WriteResponse::EventAppended { id } = response else {
            panic!("the writer answered `append_event` with another request's response");
        };
        drop(handle);
        writer.await.unwrap();

        let mut reader = open_writer_connection(&path).await.unwrap();
        let row = sqlx::query(
            "SELECT at, kind, subject_kind, subject_id, detail_json FROM event_log WHERE id = ?1",
        )
        .bind(id)
        .fetch_one(&mut reader)
        .await
        .unwrap();

        let kind: String = row.get("kind");
        let subject_kind: String = row.get("subject_kind");
        let subject_id: i64 = row.get("subject_id");
        assert_eq!(row.get::<i64, _>("at"), 1_700_000_000_000);
        assert_eq!(kind, "progress.watched");
        assert_eq!(subject_kind, "episode");
        assert_eq!(subject_id, 9);
        assert_eq!(
            row.get::<Option<String>, _>("detail_json").as_deref(),
            Some("{\"percent\":0.97}")
        );

        let parsed = Event::parse(
            &kind,
            SubjectKind::parse(&subject_kind).expect("the row holds a declared subject kind"),
            subject_id,
        );
        assert_eq!(
            parsed,
            Some(declared),
            "an appended event must read back as the event it was"
        );
    }
}
