//! The job journal: the `job` table from `DATA_MODEL` §2.8.
//!
//! The scheduler is the authority while a job is alive; this table is what survives the
//! process. Every transition the runtime reports is written through the single writer, so
//! a job's state and its progress are recorded in the same order the application observed
//! them, and the reads here are keyset-paginated because a journal grows downward and a
//! UI page is "the next twenty rows below this id".

use crate::error::StorageError;
use crate::page::PageRequest;

/// The lifecycle phase stored in `job.state`.
///
/// The database's `CHECK` constraint lists exactly these spellings, including
/// `canceled` with one `l`; the enum is what stops a caller from inventing another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobPhase {
    /// Accepted and waiting for a worker slot.
    Queued,
    /// An attempt is executing.
    Running,
    /// Waiting for a retry deadline.
    Deferred,
    /// Terminal: the last attempt succeeded.
    Succeeded,
    /// Terminal: the last attempt failed, or panicked.
    Failed,
    /// Terminal: cancellation was observed, or the job never started.
    Canceled,
}

impl JobPhase {
    /// The spelling the `job.state` constraint accepts.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Deferred => "deferred",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }

    /// Whether the phase is terminal, which is also what the dedupe index keys on.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Canceled)
    }

    /// Parses the stored spelling, or `None` when the column holds something else.
    ///
    /// A `CHECK` constraint makes an unknown value impossible, so this is a shape guard
    /// against a database written by another version rather than a validation step.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "deferred" => Some(Self::Deferred),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "canceled" => Some(Self::Canceled),
            _ => None,
        }
    }
}

/// A job to add to the journal.
#[derive(Debug, Clone)]
pub struct NewJob {
    /// The job kind, for example `hello`.
    pub kind: String,
    /// Scheduling priority; **higher runs first**.
    ///
    /// The scheduler's ready heap pops the largest value and breaks ties by submission order
    /// (`peregrine_sync`'s `heap.rs`, pinned by its own test), so the user-asked work in this tree sits
    /// at 100 and a screen-driven refresh at 10. The doc here said "lower runs first" until PG-E5-20
    /// gave a settings field a priority to choose and made the contradiction matter.
    pub priority: i32,
    /// The key that coalesces duplicate work, when the job has one.
    pub dedupe_key: Option<String>,
    /// The job's inputs, already serialized; the scheduler's `JobSpec` owns their shape.
    pub payload_json: String,
    /// How many attempts the job may make.
    pub max_attempts: i64,
    /// Unix milliseconds when the job was submitted.
    pub created_at: i64,
}

/// One reported change to a journal row.
#[derive(Debug, Clone)]
pub enum JobChange {
    /// An attempt started.
    Started {
        /// Unix milliseconds.
        at: i64,
        /// Which attempt, counted from one.
        attempt: i64,
    },
    /// Progress within the current attempt.
    Progress {
        /// Completion in permille, from 0 to 1000.
        permille: i64,
    },
    /// The attempt deferred and a retry is scheduled.
    Deferred {
        /// Which attempt failed, counted from one.
        attempt: i64,
        /// Unix milliseconds when the retry is due.
        next_attempt_at: i64,
        /// The failure's stable code, when it has one.
        error_code: Option<String>,
        /// The failure's message, already redacted.
        error: Option<String>,
    },
    /// The job reached a terminal state.
    Finished {
        /// Unix milliseconds.
        at: i64,
        /// The terminal phase.
        phase: JobPhase,
        /// The failure's stable code, when the job failed.
        error_code: Option<String>,
        /// The failure's message, already redacted.
        error: Option<String>,
    },
    /// A cancellation was requested, before the job has observed it.
    CancelRequested,
}

/// One row of the job journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRecord {
    /// The row's id, which is also the paging cursor.
    pub id: i64,
    /// The job kind.
    pub kind: String,
    /// The title named by acquisition jobs, when the payload carries one.
    pub related_title_id: Option<i64>,
    /// The stored phase.
    pub state: JobPhase,
    /// Scheduling priority.
    pub priority: i64,
    /// The coalescing key, when the job has one.
    pub dedupe_key: Option<String>,
    /// Completion in permille.
    pub progress_permille: i64,
    /// How many attempts have been made.
    pub attempts: i64,
    /// How many attempts are allowed.
    pub max_attempts: i64,
    /// Unix milliseconds when the next attempt is due, while deferred.
    pub next_attempt_at: Option<i64>,
    /// The last failure's stable code.
    pub last_error_code: Option<String>,
    /// The last failure's message.
    pub last_error: Option<String>,
    /// Unix milliseconds when the job was submitted.
    pub created_at: i64,
    /// Unix milliseconds when an attempt first started.
    pub started_at: Option<i64>,
    /// Unix milliseconds when the job reached a terminal state.
    pub finished_at: Option<i64>,
    /// Whether cancellation has been requested.
    pub cancel_requested: bool,
}

/// Inserts one row and reports the id `SQLite` assigned.
pub(crate) async fn insert(
    connection: &mut sqlx::SqliteConnection,
    job: &NewJob,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO job (kind, state, priority, dedupe_key, payload_json, max_attempts, created_at) \
         VALUES (?1, 'queued', ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(&job.kind)
    .bind(job.priority)
    .bind(job.dedupe_key.as_deref())
    .bind(&job.payload_json)
    .bind(job.max_attempts)
    .bind(job.created_at)
    .execute(&mut *connection)
    .await?;
    Ok(result.last_insert_rowid())
}

/// Applies one change, reporting whether a row was there to change.
///
/// Every statement is guarded on the phase the change is legal from, so a job that has
/// already finished cannot be moved back to running by a late report: the row is left
/// alone and the caller is told nothing changed.
pub(crate) async fn update(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
    change: &JobChange,
) -> Result<bool, sqlx::Error> {
    let result = match change {
        JobChange::Started { at, attempt } => {
            sqlx::query(
                "UPDATE job SET state = 'running', attempts = ?1, \
                 started_at = COALESCE(started_at, ?2) \
                 WHERE id = ?3 AND state IN ('queued','deferred','running')",
            )
            .bind(attempt)
            .bind(at)
            .bind(id)
            .execute(&mut *connection)
            .await?
        }
        JobChange::Progress { permille } => {
            sqlx::query(
                "UPDATE job SET progress_permille = ?1 \
                 WHERE id = ?2 AND state IN ('queued','running','deferred')",
            )
            .bind(permille)
            .bind(id)
            .execute(&mut *connection)
            .await?
        }
        JobChange::Deferred {
            attempt,
            next_attempt_at,
            error_code,
            error,
        } => {
            sqlx::query(
                "UPDATE job SET state = 'deferred', attempts = ?1, next_attempt_at = ?2, \
                 last_error_code = ?3, last_error = ?4 \
                 WHERE id = ?5 AND state IN ('running','deferred')",
            )
            .bind(attempt)
            .bind(next_attempt_at)
            .bind(error_code.as_deref())
            .bind(error.as_deref())
            .bind(id)
            .execute(&mut *connection)
            .await?
        }
        JobChange::Finished {
            at,
            phase,
            error_code,
            error,
        } => {
            sqlx::query(
                "UPDATE job SET state = ?1, finished_at = ?2, next_attempt_at = NULL, \
                 last_error_code = ?3, last_error = ?4 \
                 WHERE id = ?5 AND state IN ('queued','running','deferred')",
            )
            .bind(phase.as_str())
            .bind(at)
            .bind(error_code.as_deref())
            .bind(error.as_deref())
            .bind(id)
            .execute(&mut *connection)
            .await?
        }
        JobChange::CancelRequested => {
            sqlx::query(
                "UPDATE job SET cancel_requested = 1 \
                 WHERE id = ?1 AND state IN ('queued','running','deferred')",
            )
            .bind(id)
            .execute(&mut *connection)
            .await?
        }
    };
    Ok(result.rows_affected() > 0)
}

/// Closes out the journal rows a previous process left unfinished.
///
/// **A job cannot outlive the process that was running it, and the journal cannot tell the rows of a
/// process that died from the rows of one that is still working** — there is no owner column, so every
/// non-terminal row at open time belongs to a process that is gone. What that costs without this is
/// not cosmetic: `job_dedupe_uq` is a partial unique index over the *non-terminal* rows, so a
/// `queued` row holds its dedupe key for ever and nothing with that key can ever be submitted again.
/// `Runtime::start_discover_rails`, which joins a live row rather than writing a second one, then
/// joins a job that will never run — the screen fills nothing, every launch, and pressing Refresh
/// goes through the same join.
///
/// Reported rather than deleted: the row keeps its attempts and gains the reason it stopped.
/// `finished_at` is deliberately left alone, because the instant it stopped is exactly what nobody
/// knows; `last_error` carries the reason, and the retention policy already reads `created_at` as the
/// second window for a row that never recorded a finish.
pub(crate) async fn fail_unfinished(
    connection: &mut sqlx::SqliteConnection,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE job SET state = 'failed', next_attempt_at = NULL, \
         last_error_code = 'INTERRUPTED', last_error = ?1 \
         WHERE state IN ('queued','running','deferred')",
    )
    .bind("The process running this job ended before it finished.")
    .execute(connection)
    .await?;
    Ok(result.rows_affected())
}

/// Removes one terminal acquisition row that never admitted a download.
///
/// The Downloads screen projects failed acquisition rows as retryable preflights. Dismissing one is
/// the only way to clear it without retrying; a download row that references the journal entry is
/// left alone, because that job is no longer just a preflight.
pub(crate) async fn delete_failed_acquisition(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
) -> Result<bool, StorageError> {
    let result = sqlx::query(
        "DELETE FROM job WHERE id = ?1 AND kind = 'acquisition.acquire' \
         AND state IN ('failed','canceled') \
         AND NOT EXISTS (SELECT 1 FROM download_job WHERE download_job.job_id = job.id)",
    )
    .bind(id)
    .execute(connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(result.rows_affected() > 0)
}

/// Reads one row.
pub(crate) async fn get(
    pool: &sqlx::SqlitePool,
    id: i64,
) -> Result<Option<JobRecord>, StorageError> {
    let row = sqlx::query_as::<_, JobRow>(
        "SELECT id, kind, json_extract(payload_json, '$.titleId') AS related_title_id, state, priority, dedupe_key, progress_permille, attempts, \
         max_attempts, next_attempt_at, last_error_code, last_error, created_at, \
         started_at, finished_at, cancel_requested FROM job WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    row.map(TryInto::try_into).transpose()
}

/// Reads a page of rows, newest first.
///
/// Keyset pagination, per AGENTS §5: the cursor is the id of the previous page's last row,
/// so a job submitted between two page reads cannot shift the second one. The statement is
/// written once for both cases, because a `NULL` cursor satisfies `?1 IS NULL`.
pub(crate) async fn list(
    pool: &sqlx::SqlitePool,
    request: PageRequest,
) -> Result<Vec<JobRecord>, StorageError> {
    let rows = sqlx::query_as::<_, JobRow>(
        "SELECT id, kind, json_extract(payload_json, '$.titleId') AS related_title_id, state, priority, dedupe_key, progress_permille, attempts, \
         max_attempts, next_attempt_at, last_error_code, last_error, created_at, \
         started_at, finished_at, cancel_requested FROM job \
         WHERE (?1 IS NULL OR id < ?1) ORDER BY id DESC LIMIT ?2",
    )
    .bind(request.bindable_cursor())
    .bind(request.bindable_limit())
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    rows.into_iter().map(TryInto::try_into).collect()
}

/// The raw row, so the column list stays in one place.
#[derive(sqlx::FromRow)]
struct JobRow {
    id: i64,
    kind: String,
    related_title_id: Option<i64>,
    state: String,
    priority: i64,
    dedupe_key: Option<String>,
    progress_permille: i64,
    attempts: i64,
    max_attempts: i64,
    next_attempt_at: Option<i64>,
    last_error_code: Option<String>,
    last_error: Option<String>,
    created_at: i64,
    started_at: Option<i64>,
    finished_at: Option<i64>,
    cancel_requested: i64,
}

impl TryFrom<JobRow> for JobRecord {
    type Error = StorageError;

    fn try_from(row: JobRow) -> Result<Self, Self::Error> {
        let state = JobPhase::parse(&row.state).ok_or_else(|| StorageError::UnexpectedValue {
            column: "job.state",
            found: row.state.clone(),
        })?;
        Ok(Self {
            id: row.id,
            kind: row.kind,
            related_title_id: row.related_title_id,
            state,
            priority: row.priority,
            dedupe_key: row.dedupe_key,
            progress_permille: row.progress_permille,
            attempts: row.attempts,
            max_attempts: row.max_attempts,
            next_attempt_at: row.next_attempt_at,
            last_error_code: row.last_error_code,
            last_error: row.last_error,
            created_at: row.created_at,
            started_at: row.started_at,
            finished_at: row.finished_at,
            cancel_requested: row.cancel_requested != 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use crate::page::Cursor;
    use tempfile::TempDir;

    async fn database(directory: &TempDir) -> Database {
        Database::open(directory.path().join("peregrine.sqlite"))
            .await
            .unwrap()
    }

    fn new_job(kind: &str) -> NewJob {
        NewJob {
            kind: kind.to_owned(),
            priority: 100,
            dedupe_key: None,
            payload_json: "{\"steps\":3}".to_owned(),
            max_attempts: 3,
            // Injected, never the wall clock (AGENTS R14).
            created_at: 1_700_000_000_000,
        }
    }

    /// The six spellings are the database's `CHECK` constraint, so the enum and the
    /// schema cannot drift apart without this failing.
    #[test]
    fn every_phase_spelling_matches_the_schema_constraint() {
        let documented = [
            (JobPhase::Queued, "queued", false),
            (JobPhase::Running, "running", false),
            (JobPhase::Deferred, "deferred", false),
            (JobPhase::Succeeded, "succeeded", true),
            (JobPhase::Failed, "failed", true),
            (JobPhase::Canceled, "canceled", true),
        ];
        for (phase, spelling, terminal) in documented {
            assert_eq!(phase.as_str(), spelling);
            assert_eq!(JobPhase::parse(spelling), Some(phase));
            assert_eq!(phase.is_terminal(), terminal, "{spelling}");
        }
        assert_eq!(JobPhase::parse("cancelled"), None);
        assert_eq!(JobPhase::parse(""), None);
    }

    #[tokio::test]
    async fn a_job_round_trips_through_the_journal() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;

        let id = database.insert_job(new_job("hello")).await.unwrap();
        let row = database
            .job(id)
            .await
            .unwrap()
            .expect("the row was inserted");

        assert_eq!(row.kind, "hello");
        assert_eq!(row.state, JobPhase::Queued);
        assert_eq!(row.progress_permille, 0);
        assert_eq!(row.attempts, 0);
        assert_eq!(row.max_attempts, 3);
        assert_eq!(row.created_at, 1_700_000_000_000);
        assert_eq!(row.started_at, None);
        assert_eq!(row.finished_at, None);
        assert!(!row.cancel_requested);
        assert_eq!(row.dedupe_key, None);

        assert_eq!(database.job(id + 1).await.unwrap(), None);
        assert_eq!(
            database.list_jobs(PageRequest::first(10)).await.unwrap(),
            vec![row]
        );
    }

    #[tokio::test]
    async fn only_a_failed_acquisition_can_be_dismissed() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        let id = database
            .insert_job(new_job("acquisition.acquire"))
            .await
            .unwrap();

        assert!(
            !database.dismiss_failed_acquisition(id).await.unwrap(),
            "a queued job is not a failed preflight"
        );
        database
            .update_job(
                id,
                JobChange::Finished {
                    at: 1_700_000_000_300,
                    phase: JobPhase::Failed,
                    error_code: Some("INTERRUPTED".to_owned()),
                    error: Some("The process ended.".to_owned()),
                },
            )
            .await
            .unwrap();
        assert!(database.dismiss_failed_acquisition(id).await.unwrap());
        assert_eq!(database.job(id).await.unwrap(), None);

        let other = database.insert_job(new_job("hello")).await.unwrap();
        database
            .update_job(
                other,
                JobChange::Finished {
                    at: 1_700_000_000_400,
                    phase: JobPhase::Failed,
                    error_code: None,
                    error: None,
                },
            )
            .await
            .unwrap();
        assert!(
            !database.dismiss_failed_acquisition(other).await.unwrap(),
            "a failed job of another kind is not a preflight"
        );
    }

    #[tokio::test]
    async fn an_attempt_records_its_start_and_its_progress() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        let id = database.insert_job(new_job("hello")).await.unwrap();

        assert!(
            database
                .update_job(
                    id,
                    JobChange::Started {
                        at: 1_700_000_000_100,
                        attempt: 1,
                    },
                )
                .await
                .unwrap()
        );
        assert!(
            database
                .update_job(id, JobChange::Progress { permille: 400 })
                .await
                .unwrap()
        );

        let row = database.job(id).await.unwrap().unwrap();
        assert_eq!(row.state, JobPhase::Running);
        assert_eq!(row.attempts, 1);
        assert_eq!(row.started_at, Some(1_700_000_000_100));
        assert_eq!(row.progress_permille, 400);
    }

    /// The guard that matters: a progress report that arrives after the job finished must
    /// not move the row out of its terminal state.
    #[tokio::test]
    async fn a_late_report_cannot_resurrect_a_finished_job() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        let id = database.insert_job(new_job("hello")).await.unwrap();
        database
            .update_job(
                id,
                JobChange::Started {
                    at: 1_700_000_000_100,
                    attempt: 1,
                },
            )
            .await
            .unwrap();
        database
            .update_job(id, JobChange::Progress { permille: 500 })
            .await
            .unwrap();
        database
            .update_job(
                id,
                JobChange::Finished {
                    at: 1_700_000_000_200,
                    phase: JobPhase::Succeeded,
                    error_code: None,
                    error: None,
                },
            )
            .await
            .unwrap();

        for late in [
            JobChange::Progress { permille: 900 },
            JobChange::Started {
                at: 1_700_000_000_300,
                attempt: 2,
            },
            JobChange::CancelRequested,
            JobChange::Deferred {
                attempt: 2,
                next_attempt_at: 1_700_000_000_400,
                error_code: None,
                error: None,
            },
        ] {
            assert!(
                !database.update_job(id, late.clone()).await.unwrap(),
                "{late:?} must be refused after the job reached a terminal state"
            );
        }

        let row = database.job(id).await.unwrap().unwrap();
        assert_eq!(row.state, JobPhase::Succeeded);
        assert_eq!(row.progress_permille, 500);
        assert_eq!(row.attempts, 1);
        assert_eq!(row.finished_at, Some(1_700_000_000_200));
    }

    #[tokio::test]
    async fn a_deferral_records_the_deadline_and_the_failure() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        let id = database.insert_job(new_job("hello")).await.unwrap();
        database
            .update_job(
                id,
                JobChange::Started {
                    at: 1_700_000_000_100,
                    attempt: 1,
                },
            )
            .await
            .unwrap();

        database
            .update_job(
                id,
                JobChange::Deferred {
                    attempt: 1,
                    next_attempt_at: 1_700_000_000_500,
                    error_code: Some("NETWORK".to_owned()),
                    error: Some("the connection dropped".to_owned()),
                },
            )
            .await
            .unwrap();

        let row = database.job(id).await.unwrap().unwrap();
        assert_eq!(row.state, JobPhase::Deferred);
        assert_eq!(row.next_attempt_at, Some(1_700_000_000_500));
        assert_eq!(row.last_error_code.as_deref(), Some("NETWORK"));
        assert_eq!(row.attempts, 1);

        // A retry that succeeds clears the deadline, so the UI cannot show a job as
        // waiting for a retry it already had.
        database
            .update_job(
                id,
                JobChange::Started {
                    at: 1_700_000_000_500,
                    attempt: 2,
                },
            )
            .await
            .unwrap();
        database
            .update_job(
                id,
                JobChange::Finished {
                    at: 1_700_000_000_600,
                    phase: JobPhase::Succeeded,
                    error_code: None,
                    error: None,
                },
            )
            .await
            .unwrap();

        let row = database.job(id).await.unwrap().unwrap();
        assert_eq!(row.state, JobPhase::Succeeded);
        assert_eq!(row.next_attempt_at, None);
        assert_eq!(row.finished_at, Some(1_700_000_000_600));
    }

    #[tokio::test]
    async fn a_cancellation_is_recorded_before_it_is_observed() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        let id = database.insert_job(new_job("hello")).await.unwrap();

        assert!(
            database
                .update_job(id, JobChange::CancelRequested)
                .await
                .unwrap()
        );
        let row = database.job(id).await.unwrap().unwrap();
        assert!(row.cancel_requested, "the request must be visible at once");
        assert_eq!(row.state, JobPhase::Queued, "asking is not yet stopping");

        database
            .update_job(
                id,
                JobChange::Finished {
                    at: 1_700_000_000_200,
                    phase: JobPhase::Canceled,
                    error_code: None,
                    error: None,
                },
            )
            .await
            .unwrap();

        let row = database.job(id).await.unwrap().unwrap();
        assert_eq!(row.state, JobPhase::Canceled);
        assert!(
            row.cancel_requested,
            "the audit keeps the request alongside the outcome"
        );
    }

    /// A page below a cursor is not shifted by a row written after the first page was
    /// read, which is the whole reason the cursor is an id rather than an offset.
    #[tokio::test]
    async fn pages_are_keyset_paged_and_unaffected_by_new_rows() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        let mut ids = Vec::new();
        for index in 0..5 {
            ids.push(
                database
                    .insert_job(new_job(&format!("job-{index}")))
                    .await
                    .unwrap(),
            );
        }

        let first = database.list_jobs(PageRequest::first(2)).await.unwrap();
        assert_eq!(
            first.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![ids[4], ids[3]]
        );

        // A row arriving between the two reads must not appear in the second page, nor
        // push anything out of it.
        database.insert_job(new_job("job-5")).await.unwrap();
        let cursor = Cursor::new(first.last().unwrap().id);
        let second = database
            .list_jobs(PageRequest::after(cursor, 2))
            .await
            .unwrap();
        assert_eq!(
            second.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![ids[2], ids[1]]
        );

        let third = database
            .list_jobs(PageRequest::after(
                Cursor::new(second.last().unwrap().id),
                2,
            ))
            .await
            .unwrap();
        assert_eq!(
            third.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![ids[0]]
        );
        assert!(
            database
                .list_jobs(PageRequest::after(Cursor::new(ids[0]), 2))
                .await
                .unwrap()
                .is_empty(),
            "the first row is the end of the journal"
        );
    }

    /// The empty case: paging an empty journal is an empty page, not an error.
    #[tokio::test]
    async fn an_empty_journal_lists_nothing() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        assert!(
            database
                .list_jobs(PageRequest::first(10))
                .await
                .unwrap()
                .is_empty(),
            "a fresh database has no jobs"
        );
        assert!(
            database
                .list_jobs(PageRequest::after(Cursor::new(1), 10))
                .await
                .unwrap()
                .is_empty(),
            "a cursor below every row has nothing below it"
        );
    }
}
