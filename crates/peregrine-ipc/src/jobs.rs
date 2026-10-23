//! The `jobs` namespace of the command surface (docs/ARCHITECTURE.md §7).
//!
//! Every command here is job-scoped, which is the pattern PG-E2-06 fixes: a start returns
//! an id and no command waits for the work. Progress arrives on `job://progress` and the
//! lifecycle on `job://state`, both through the coalescing bus, so a job that runs for a
//! minute cannot flood the webview and the UI never blocks on a job it started.

use peregrine_app::ProviderError;
use peregrine_app::runtime::RuntimeError;
use peregrine_app::{JobPhase, JobRecord as StoredJobRecord, StorageError};

use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;
use tauri::State;

use crate::error::{AppError, ErrorCode};
use crate::guard::guard_response;
use crate::runtime::RuntimeState;

/// A job the runtime accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct JobStarted {
    /// The scheduler's identity for the job; the key its events carry.
    pub job_id: String,
    /// The journal row it reports into.
    ///
    /// `Number` opts this `i64` into a TypeScript `number`, which specta otherwise refuses
    /// rather than silently losing precision. A row id and a Unix-millisecond timestamp
    /// both stay well inside the 2^53 a JavaScript number represents exactly.
    #[specta(type = Number)]
    pub journal_id: i64,
    /// Whether the runtime joined a job that was already running rather than starting one.
    ///
    /// A screen tells the user "a refresh is already under way" from this rather than decoding the
    /// nil job id, and it follows the *journal* row either way: every `job://state` payload carries
    /// both ids, and the journal one exists before the job runs.
    pub coalesced: bool,
}

/// The lifecycle phase, as the UI reads it.
///
/// A separate enum from the storage crate's, because the wire form is part of the IPC
/// contract and the storage spelling is part of the schema; `every_phase_matches_the_
/// schema` proves they cannot drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
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

impl From<JobPhase> for JobState {
    fn from(phase: JobPhase) -> Self {
        match phase {
            JobPhase::Queued => Self::Queued,
            JobPhase::Running => Self::Running,
            JobPhase::Deferred => Self::Deferred,
            JobPhase::Succeeded => Self::Succeeded,
            JobPhase::Failed => Self::Failed,
            JobPhase::Canceled => Self::Canceled,
        }
    }
}

/// One row of the job journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct JobRecord {
    /// The row's id, which is also the paging cursor.
    #[specta(type = Number)]
    pub id: i64,
    /// The job kind.
    pub kind: String,
    /// The stored phase.
    pub state: JobState,
    /// Scheduling priority.
    #[specta(type = Number)]
    pub priority: i64,
    /// Completion in permille, from 0 to 1000.
    #[specta(type = Number)]
    pub progress_permille: i64,
    /// How many attempts have been made.
    #[specta(type = Number)]
    pub attempts: i64,
    /// How many attempts are allowed.
    #[specta(type = Number)]
    pub max_attempts: i64,
    /// Unix milliseconds when the next attempt is due, while deferred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[specta(type = Option<Number>)]
    pub next_attempt_at: Option<i64>,
    /// The last failure's stable code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_code: Option<String>,
    /// The last failure's message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Unix milliseconds when the job was submitted.
    #[specta(type = Number)]
    pub created_at: i64,
    /// Unix milliseconds when an attempt first started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[specta(type = Option<Number>)]
    pub started_at: Option<i64>,
    /// Unix milliseconds when the job reached a terminal state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[specta(type = Option<Number>)]
    pub finished_at: Option<i64>,
    /// Whether cancellation has been requested.
    pub cancel_requested: bool,
}

impl From<StoredJobRecord> for JobRecord {
    fn from(row: StoredJobRecord) -> Self {
        Self {
            id: row.id,
            kind: row.kind,
            state: row.state.into(),
            priority: row.priority,
            progress_permille: row.progress_permille,
            attempts: row.attempts,
            max_attempts: row.max_attempts,
            next_attempt_at: row.next_attempt_at,
            last_error_code: row.last_error_code,
            last_error: row.last_error,
            created_at: row.created_at,
            started_at: row.started_at,
            finished_at: row.finished_at,
            cancel_requested: row.cancel_requested,
        }
    }
}

/// Starts the hello job.
///
/// # Errors
///
/// [`ErrorCode::InvalidInput`] when `steps` is outside the documented range,
/// [`ErrorCode::Internal`] when the scheduler is not running.
#[tauri::command]
#[specta::specta]
pub async fn start_hello(
    state: State<'_, RuntimeState>,
    steps: u32,
) -> Result<JobStarted, AppError> {
    let started = state.runtime()?.start_hello(steps).await?;
    Ok(guard_response(
        "jobs::start_hello",
        JobStarted::from(started),
    ))
}

/// A submission the runtime accepted, for the two commands that answer one.
///
/// The conversion lives here rather than at each call site because [`JobStarted`] is this crate's
/// wire type and a fourth field added to it would otherwise be a change in four places.
impl From<peregrine_app::StartedJob> for JobStarted {
    fn from(started: peregrine_app::StartedJob) -> Self {
        Self {
            job_id: started.job.to_string(),
            journal_id: started.journal_id,
            coalesced: started.coalesced,
        }
    }
}

/// Asks a running job to stop.
///
/// # Errors
///
/// [`ErrorCode::InvalidInput`] when `job_id` is not a job id,
/// [`ErrorCode::NotFound`] when no live job has it.
#[tauri::command]
#[specta::specta]
pub async fn cancel(state: State<'_, RuntimeState>, job_id: String) -> Result<(), AppError> {
    let job = peregrine_app::jobs::parse_job_id(&job_id).map_err(|error| {
        AppError::new(ErrorCode::InvalidInput).with_detail(format!("`{error}` is not a job id"))
    })?;
    state.runtime()?.cancel(job).await?;
    Ok(())
}

/// The newest page of the job journal.
///
/// The page size is capped here rather than trusted: an unbounded page would break the
/// 64 KB response budget in ARCHITECTURE §7 rule 1. Keyset paging below a cursor is a
/// repository concern and is tested there; this surface shows the newest work, which is
/// what a diagnostics screen needs.
///
/// # Errors
///
/// [`ErrorCode::StorageCorrupt`] when the journal cannot be read.
#[tauri::command]
#[specta::specta]
pub async fn list(
    state: State<'_, RuntimeState>,
    limit: Option<u32>,
) -> Result<Vec<JobRecord>, AppError> {
    const MAX_PAGE: u32 = 100;
    let limit = limit.unwrap_or(20).clamp(1, MAX_PAGE);
    let rows = state.runtime()?.jobs(None, limit).await?;
    Ok(guard_response(
        "jobs::list",
        rows.into_iter().map(JobRecord::from).collect::<Vec<_>>(),
    ))
}

impl From<RuntimeError> for AppError {
    fn from(error: RuntimeError) -> Self {
        match &error {
            RuntimeError::InvalidRequest { detail } => {
                AppError::new(ErrorCode::InvalidInput).with_detail(detail.clone())
            }
            RuntimeError::UnknownJob { job } => AppError::new(ErrorCode::NotFound)
                .with_detail("That job is no longer running.")
                .with_cause(format!("no live job {job}")),
            RuntimeError::Scheduler(inner) => AppError::new(ErrorCode::Internal)
                .with_detail("The job queue is not running.")
                .with_cause(inner.to_string()),
            // The user is told what to do about each of these rather than what went wrong inside:
            // a provider's own error carries no URL and no key, and saying "the provider refused"
            // is the difference between a user checking their API key and a user filing a bug.
            RuntimeError::EmptyQuery => {
                AppError::new(ErrorCode::InvalidInput).with_detail("Type something to search for.")
            }
            // Each of these says what the user can do about it. A provider error carries no URL and no
            // key, so nothing here can leak one — which is why the detail is written rather than passed
            // through.
            RuntimeError::Provider(ProviderError::RateLimited { .. }) => {
                AppError::new(ErrorCode::ProviderRateLimited)
                    .with_detail("The provider is asking us to slow down. Try again in a moment.")
                    .with_cause("the provider refused with 429")
            }
            RuntimeError::Provider(ProviderError::SchemaDrift { .. }) => {
                AppError::new(ErrorCode::ProviderSchemaDrift)
                    .with_detail("The provider's answer has changed shape and could not be read.")
                    .with_cause("the provider's payload did not match the documented shape")
            }
            RuntimeError::Provider(_) => AppError::new(ErrorCode::ProviderUnavailable)
                .with_detail(
                    "The metadata provider could not answer. Check the API key in Settings.",
                )
                .with_cause("a provider request failed"),
            RuntimeError::Sync(inner) => AppError::new(ErrorCode::Internal)
                .with_detail("The provider's answer could not be saved.")
                .with_cause(inner.to_string()),
            // Neither of these messages carries a filesystem path into the payload: the
            // rule is that `detail` and `cause` are safe to show, and the path is in the
            // log instead (ARCHITECTURE §7 rule 3).
            RuntimeError::DataDirectory { source, .. } => {
                AppError::new(ErrorCode::PermissionDenied)
                    .with_detail("Peregrine could not create its data folder.")
                    .with_cause(source.to_string())
            }
            RuntimeError::Logs { source, .. } => AppError::new(ErrorCode::Internal)
                .with_detail("The log file could not be read.")
                .with_cause(source.to_string()),
            RuntimeError::Storage(inner) => storage_error(inner),
            // A folder that is already a library is a conflict the screen can point at; a path the
            // service refuses is bad input; anything else is the database, and `storage_error`
            // decides what to say about that. The service's own message is not passed through: it
            // contains the absolute path the user picked, and rule 3 of ARCHITECTURE §7 keeps paths
            // out of the payload.
            RuntimeError::ScanAlreadyRunning { job } => AppError::new(ErrorCode::Conflict)
                .with_detail("A scan is already running.")
                .with_cause(format!("the running scan is {job}")),
            RuntimeError::Library(inner) => library_error(inner),
            // A hand-match fails for two reasons: the database refused, or a path cannot be stored
            // as text. The first is the storage codes; the second is the caller's input.
            RuntimeError::Index(inner) => match inner {
                peregrine_app::IndexError::Storage(storage) => storage_error(storage),
                peregrine_app::IndexError::NotText { .. } => AppError::new(ErrorCode::InvalidInput)
                    .with_detail("That file's path cannot be stored.")
                    .with_cause(inner.to_string()),
            },
            // Nothing is configured to ask: a setting rather than a failure, and the one answer a
            // user can act on is where to set it. `Unsupported` rather than `ProviderUnavailable`
            // because retrying changes nothing until a key exists.
            RuntimeError::NoProvider => AppError::new(ErrorCode::Unsupported)
                .with_detail("No metadata provider is configured. Add an API key in Settings.")
                .with_cause("no configured provider serves this read"),
            // **A playback refusal, and every one of them is something a person can act on** — which
            // is why the details are written here rather than passed through. A title that is not
            // here and one this account may not see are one answer on purpose (I37), and the file's
            // own path never reaches the payload: `FileOutsideRoots` says where the refusal is about
            // without naming where the file is (ARCHITECTURE §7 rule 3).
            RuntimeError::UnknownTitle => AppError::new(ErrorCode::NotFound)
                .with_detail("That title is not in your library.")
                .with_cause("no visible title has that id"),
            RuntimeError::NoPlayableFile => AppError::new(ErrorCode::NotFound)
                .with_detail("Your library has no copy of that title that is on this machine.")
                .with_cause("the title holds no file whose scan says it is present"),
            RuntimeError::MediaUnidentified => AppError::new(ErrorCode::Conflict)
                .with_detail(
                    "That download holds more than one episode, and none matches the one you asked for.",
                )
                .with_cause("no selected media entry's name matches the requested episode"),
            RuntimeError::NotReleased => AppError::new(ErrorCode::Conflict)
                .with_detail("That has not been released yet, so it cannot be marked watched.")
                .with_cause("the release or air date is in the future"),
            RuntimeError::FileOutsideRoots => AppError::new(ErrorCode::PermissionDenied)
                .with_detail("That file is outside the folders Peregrine is allowed to read.")
                .with_cause("no configured library root contains the media file's path"),
            // The two refusals that are about the file, which the runtime classified so that this
            // surface never has to name the stream crate's error type. Both are "there is nothing to
            // play here", and each says the one thing a person can do about it.
            RuntimeError::MediaMissing => AppError::new(ErrorCode::NotFound)
                .with_detail("That file is no longer where the last scan saw it.")
                .with_cause("the media file is not on disk"),
            RuntimeError::MediaNotAFile => AppError::new(ErrorCode::NotFound)
                .with_detail("That is not a file Peregrine can play.")
                .with_cause("the recorded path is not a regular file"),
            // Anything else is this process failing to serve a file it can see — a listener that
            // would not bind, a file that would not open — which is ours rather than the user's.
            RuntimeError::Playback(inner) => AppError::new(ErrorCode::Internal)
                .with_detail("Playback could not be started.")
                .with_cause(inner.to_string()),
            // The external player is a separate program: a build with no mpv cannot play this
            // container, and the sentence names the install rather than a failed spawn.
            RuntimeError::Player(inner) => AppError::new(ErrorCode::Unsupported)
                .with_detail("The external player could not be started. Install mpv and try again.")
                .with_cause(inner.to_string()),
            // The session did not start, so acquisition and streaming cannot answer. The request is
            // not what is wrong, so the sentence says what is unavailable rather than what broke.
            RuntimeError::TorrentEngineUnavailable => AppError::new(ErrorCode::Unsupported)
                .with_detail("Downloads are unavailable because the torrent engine did not start.")
                .with_cause("the torrent session was not composed at startup"),
            RuntimeError::PublicIpUnavailable => AppError::new(ErrorCode::Internal)
                .with_detail(
                    "The public address could not be read. Check the connection and try again.",
                )
                .with_cause("the public-address echo request failed"),
        }
    }
}

/// Maps a library refusal onto the codes the UI branches on.
fn library_error(error: &peregrine_app::LibraryError) -> AppError {
    use peregrine_app::LibraryError;

    match error {
        LibraryError::AlreadyARoot { .. } => AppError::new(ErrorCode::Conflict)
            .with_detail("That folder is already one of your libraries.")
            .with_cause(error.to_string()),
        LibraryError::Path(_) | LibraryError::NotText { .. } => {
            AppError::new(ErrorCode::InvalidInput)
                .with_detail("That folder cannot be used as a library.")
                .with_cause(error.to_string())
        }
        LibraryError::Storage(inner) => storage_error(inner),
    }
}

/// Maps a storage failure onto the codes the UI branches on.
///
/// The storage error's own message is not used: several of its variants name an absolute
/// path, and rule 3 of ARCHITECTURE §7 keeps paths out of the payload. What the user needs
/// is the code and what to do about it; what a maintainer needs is in the log.
fn storage_error(error: &StorageError) -> AppError {
    match error {
        StorageError::Busy { .. } => AppError::new(ErrorCode::StorageBusy),
        // The detail is written for a user by the repository that refused the write, and it
        // names the id rather than a path or a secret.
        StorageError::Conflict { detail } => {
            AppError::new(ErrorCode::Conflict).with_detail(detail.clone())
        }
        StorageError::Open { .. } => AppError::new(ErrorCode::PermissionDenied)
            .with_detail("The library database could not be opened.")
            .with_cause("the database file could not be opened"),
        StorageError::Migrate { .. } => AppError::new(ErrorCode::StorageCorrupt)
            .with_detail("The library database could not be upgraded.")
            .with_cause("the embedded migrations could not be applied"),
        // The database is untouched, so this is not corruption: the upgrade did not run and
        // the copy taken first is the way back. The version and the copy's path are in the
        // log, and neither belongs in the payload (rule 3 of ARCHITECTURE §7).
        StorageError::MigrationFailed {
            version,
            description,
            ..
        } => AppError::new(ErrorCode::StorageCorrupt)
            .with_detail("The library database could not be upgraded; it was left unchanged.")
            .with_cause(format!("migration {version:04}_{description} failed")),
        StorageError::Backup { .. } => AppError::new(ErrorCode::PermissionDenied)
            .with_detail("The library database could not be copied.")
            .with_cause("the copy could not be written"),
        // The user chose a file; the answer is theirs to act on, which is why this is input
        // rather than corruption. The path stays in the log (ARCHITECTURE §7 rule 3).
        StorageError::InvalidBackup { detail, .. } => AppError::new(ErrorCode::InvalidInput)
            .with_detail("That file is not a backup Peregrine can restore.")
            .with_cause(detail.clone()),
        StorageError::Quarantine { .. } => AppError::new(ErrorCode::StorageCorrupt)
            .with_detail(
                "The library database is damaged and could not be moved aside. Close any \
                 other copy of Peregrine and start again.",
            )
            .with_cause("the damaged database could not be moved aside"),
        StorageError::Pragma { pragma, .. } => AppError::new(ErrorCode::StorageCorrupt)
            .with_detail("The library database could not be configured safely.")
            .with_cause(format!("`{pragma}` is not in force")),
        StorageError::WriterStopped => AppError::new(ErrorCode::Internal)
            .with_detail("The library database is not accepting writes.")
            .with_cause("the single-writer task has stopped"),
        // The driver's sentence is what makes the next one diagnosable — a UNIQUE violation
        // names the index's columns, a missing column names itself — and it carries no path and
        // no secret. The detail a person reads stays generic, because "no such column" is not
        // something they can act on (ARCHITECTURE §7 rule 3).
        StorageError::Statement { source } => {
            let cause = source.to_string();
            tracing::warn!(error = %cause, "a library database statement failed");
            AppError::new(ErrorCode::Internal)
                .with_detail("The library database rejected a statement.")
                .with_cause(cause)
        }
        StorageError::UnexpectedValue { column, .. } => AppError::new(ErrorCode::StorageCorrupt)
            .with_detail("The library database holds a value this version cannot read.")
            .with_cause(format!("`{column}` is outside its allowed values")),
        // A JSON column this version wrote and cannot parse is the same class as the arm
        // above: the row is there and its content is unreadable, which is corruption rather
        // than a bad request. `column` and the parse's own message stay in the log — the
        // detail a user needs is that the file is damaged, not which field failed.
        StorageError::InvalidJsonField { column, .. } => AppError::new(ErrorCode::StorageCorrupt)
            .with_detail("The library database holds a value this version cannot read.")
            .with_cause(format!("`{column}` is not the JSON this version wrote")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire form of a phase is what the UI switches on, and the storage spelling is
    /// what the schema's `CHECK` accepts. They are two declarations of one vocabulary, so
    /// this is the test that keeps them one vocabulary.
    #[test]
    fn every_phase_matches_the_schema() {
        for (phase, state, wire) in [
            (JobPhase::Queued, JobState::Queued, "\"queued\""),
            (JobPhase::Running, JobState::Running, "\"running\""),
            (JobPhase::Deferred, JobState::Deferred, "\"deferred\""),
            (JobPhase::Succeeded, JobState::Succeeded, "\"succeeded\""),
            (JobPhase::Failed, JobState::Failed, "\"failed\""),
            (JobPhase::Canceled, JobState::Canceled, "\"canceled\""),
        ] {
            assert_eq!(JobState::from(phase), state);
            assert_eq!(serde_json::to_string(&state).unwrap(), wire);
            assert_eq!(phase.as_str(), state_str(state));
        }
    }

    /// The lowercase name of a state, so the test above compares the schema's spelling
    /// with the wire's without a second hand-written table in the assertion.
    fn state_str(state: JobState) -> &'static str {
        match state {
            JobState::Queued => "queued",
            JobState::Running => "running",
            JobState::Deferred => "deferred",
            JobState::Succeeded => "succeeded",
            JobState::Failed => "failed",
            JobState::Canceled => "canceled",
        }
    }

    #[test]
    fn a_stored_row_maps_field_for_field() {
        let stored = StoredJobRecord {
            id: 7,
            kind: "hello".to_owned(),
            state: JobPhase::Deferred,
            priority: 100,
            dedupe_key: Some("ignored-by-the-dto".to_owned()),
            progress_permille: 250,
            attempts: 1,
            max_attempts: 3,
            next_attempt_at: Some(1_700_000_001_000),
            last_error_code: Some("NETWORK".to_owned()),
            last_error: Some("the connection dropped".to_owned()),
            created_at: 1_700_000_000_000,
            started_at: Some(1_700_000_000_100),
            finished_at: None,
            cancel_requested: true,
        };

        let dto = JobRecord::from(stored);
        assert_eq!(dto.id, 7);
        assert_eq!(dto.state, JobState::Deferred);
        assert_eq!(dto.progress_permille, 250);
        assert_eq!(dto.next_attempt_at, Some(1_700_000_001_000));
        assert_eq!(dto.last_error_code.as_deref(), Some("NETWORK"));
        assert_eq!(dto.finished_at, None);
        assert!(dto.cancel_requested);

        // The wire shape omits absent optionals rather than sending nulls.
        let json = serde_json::to_value(&dto).unwrap();
        let object = json.as_object().unwrap();
        assert!(!object.contains_key("nextAttemptAt") || object["nextAttemptAt"].is_number());
        assert!(
            !object.contains_key("finishedAt"),
            "an absent timestamp must be omitted: {json}"
        );
        assert!(object.contains_key("progressPermille"));
    }

    /// Every mapped runtime failure, with the code and the retryability it must produce.
    ///
    /// The table is separate from the assertion below because it is most of the line count, and a
    /// test that trips `too_many_lines` hides the logic it exists to show.
    fn runtime_failure_cases() -> Vec<(RuntimeError, ErrorCode, bool)> {
        vec![
            (
                RuntimeError::InvalidRequest {
                    detail: "steps must be between 1 and 600".to_owned(),
                },
                ErrorCode::InvalidInput,
                false,
            ),
            (
                RuntimeError::UnknownJob {
                    job: peregrine_app::jobs::parse_job_id("01ARZ3NDEKTSV4RRFFQ69G5FAV")
                        .expect("a valid ULID"),
                },
                ErrorCode::NotFound,
                false,
            ),
            (
                RuntimeError::Storage(StorageError::Busy { busy_timeout_ms: 5 }),
                ErrorCode::StorageBusy,
                true,
            ),
            (
                RuntimeError::Storage(StorageError::WriterStopped),
                ErrorCode::Internal,
                false,
            ),
            (
                RuntimeError::Storage(StorageError::MigrationFailed {
                    version: 9,
                    description: "deliberately_failing".to_owned(),
                    backup: Some(std::path::PathBuf::from(
                        "/Users/someone/Library/Application Support/peregrine-leak-canary/peregrine.sqlite.backup-0008-01",
                    )),
                    source: "the migration failed".into(),
                }),
                ErrorCode::StorageCorrupt,
                false,
            ),
            (
                RuntimeError::Storage(StorageError::Backup {
                    path: std::path::PathBuf::from(
                        "C:\\Users\\someone\\AppData\\Roaming\\peregrine-leak-canary",
                    ),
                    source: "the disk is full".into(),
                }),
                ErrorCode::PermissionDenied,
                false,
            ),
            (
                RuntimeError::Storage(StorageError::InvalidBackup {
                    path: std::path::PathBuf::from(
                        "/home/someone/.local/share/peregrine-leak-canary",
                    ),
                    detail: "it was written by a newer version of Peregrine".to_owned(),
                }),
                ErrorCode::InvalidInput,
                false,
            ),
            (
                RuntimeError::Storage(StorageError::Quarantine {
                    path: std::path::PathBuf::from(
                        "/home/someone/.local/share/peregrine-leak-canary",
                    ),
                    source: "permission denied".into(),
                }),
                ErrorCode::StorageCorrupt,
                false,
            ),
            (
                RuntimeError::Storage(StorageError::Statement {
                    source: "a statement failed".into(),
                }),
                ErrorCode::Internal,
                false,
            ),
            (
                RuntimeError::DataDirectory {
                    path: std::path::PathBuf::from(
                        "/Users/someone/Library/Application Support/peregrine-leak-canary",
                    ),
                    source: std::io::Error::new(
                        std::io::ErrorKind::NotADirectory,
                        "not a directory",
                    ),
                },
                ErrorCode::PermissionDenied,
                false,
            ),
            (RuntimeError::NoProvider, ErrorCode::Unsupported, false),
        ]
    }

    /// The error mapping, pinned per variant: a retryable condition must not become a permanent
    /// one and a path must never reach the payload.
    #[test]
    fn runtime_failures_map_to_stable_codes() {
        for (error, code, retryable) in runtime_failure_cases() {
            let mapped = AppError::from(error);
            assert_eq!(mapped.code, code, "{}", mapped.message);
            assert_eq!(mapped.retryable, retryable, "{}", mapped.message);
            for text in [mapped.detail.as_deref(), mapped.cause.as_deref()]
                .into_iter()
                .flatten()
            {
                crate::redaction::assert_no_path("a mapped runtime failure", text);
            }
        }
    }

    /// A statement failure keeps its fixed sentence for the person and carries the driver's own
    /// text as the cause, which is the diagnostic line the next one of these is read from.
    ///
    /// The live failure this comes from was `UNIQUE constraint failed: job.kind, job.dedupe_key`;
    /// the screen said only "The library database rejected a statement." and nothing anywhere said
    /// which statement or why.
    #[test]
    fn a_statement_failure_carries_the_drivers_sentence_as_its_cause() {
        let mapped = AppError::from(RuntimeError::Storage(StorageError::Statement {
            source: "UNIQUE constraint failed: job.kind, job.dedupe_key".into(),
        }));
        assert_eq!(mapped.code, ErrorCode::Internal);
        assert_eq!(
            mapped.detail.as_deref(),
            Some("The library database rejected a statement."),
            "the sentence a person reads does not change"
        );
        assert_eq!(
            mapped.cause.as_deref(),
            Some("UNIQUE constraint failed: job.kind, job.dedupe_key"),
            "the driver's text is what makes the failure diagnosable"
        );
        crate::redaction::assert_no_path("a statement cause", mapped.cause.as_deref().unwrap());
    }
}

#[cfg(test)]
mod command_tests {
    use super::*;
    use peregrine_app::events::{Event, EventSink, EventSinkError};
    use peregrine_app::{Observability, Runtime, RuntimeConfig};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tauri::Manager as _;
    use tempfile::TempDir;

    /// A sink that discards every event: these tests are about the command surface, and
    /// the bus's own delivery is tested in the crate that owns it.
    struct SilentSink;

    impl EventSink for SilentSink {
        fn emit(&self, _event: &Event) -> Result<(), EventSinkError> {
            Ok(())
        }
    }

    /// Starts a runtime into a mock app and returns the app, so a command can be called
    /// the way Tauri calls it: with managed state injected.
    async fn served(directory: &TempDir) -> tauri::App<tauri::test::MockRuntime> {
        let app = tauri::test::mock_app();
        let config = RuntimeConfig::new(
            directory.path(),
            directory.path().join("artwork"),
            Arc::new(SilentSink),
        )
        // A command test never reaches the developer's keychain (`AGENTS R14`): a real read from a
        // freshly built binary raises a macOS access prompt and waits on a person.
        .with_secrets(Arc::new(crate::test_support::NoCredentials));
        let (subscriber, logging) = Observability::build(&config.log_config()).unwrap();
        drop(subscriber);
        let runtime = Runtime::start(config, logging).await.unwrap();
        app.manage(RuntimeState::for_test(Arc::new(runtime)));
        app
    }

    /// Waits for the journal row to reach a terminal phase, through the command surface.
    async fn await_terminal(state: &State<'_, RuntimeState>, journal_id: i64) -> JobRecord {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let rows = list(state.clone(), None).await.unwrap();
                let row = rows
                    .into_iter()
                    .find(|row| row.id == journal_id)
                    .expect("the row exists");
                if matches!(
                    row.state,
                    JobState::Succeeded | JobState::Failed | JobState::Canceled
                ) {
                    return row;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the job must reach a terminal state")
    }

    /// PG-E0-19's end-to-end path through the actual command functions: start a job over
    /// IPC, watch its journal reach a terminal state, and read it back through `list`.
    #[tokio::test]
    async fn start_then_list_reports_a_finished_job() {
        let directory = TempDir::new().unwrap();
        let app = served(&directory).await;
        let state = app.state::<RuntimeState>();

        let started = start_hello(state.clone(), 3).await.unwrap();
        assert_eq!(started.job_id.len(), 26, "a ULID as a string");
        assert!(started.journal_id > 0);

        let row = await_terminal(&state, started.journal_id).await;
        assert_eq!(row.state, JobState::Succeeded);
        assert_eq!(row.kind, "hello");
        assert_eq!(row.progress_permille, 1000);
        assert_eq!(row.attempts, 1);

        // The page below the cursor holds what came before, and the newest row is first.
        let first_page = list(state.clone(), Some(1)).await.unwrap();
        assert_eq!(first_page.len(), 1);
        assert_eq!(first_page[0].id, row.id);
        assert_eq!(
            list(state.clone(), Some(0)).await.unwrap().len(),
            1,
            "a zero limit is clamped to one row rather than refused"
        );

        crate::runtime::shutdown(app.handle()).await;
    }

    /// Cancellation over IPC, including the second attempt that has nothing to cancel.
    #[tokio::test]
    async fn cancel_stops_a_running_job_and_then_reports_it_unknown() {
        let directory = TempDir::new().unwrap();
        let app = served(&directory).await;
        let state = app.state::<RuntimeState>();

        let started = start_hello(state.clone(), 600).await.unwrap();
        cancel(state.clone(), started.job_id.clone()).await.unwrap();

        let row = await_terminal(&state, started.journal_id).await;
        assert_eq!(row.state, JobState::Canceled);
        assert!(row.cancel_requested);

        let error = cancel(state.clone(), started.job_id)
            .await
            .expect_err("a finished job cannot be cancelled");
        assert_eq!(error.code, ErrorCode::NotFound);
        assert_eq!(error.message_key(), "error.not_found");

        crate::runtime::shutdown(app.handle()).await;
    }

    /// A malformed id and an out-of-range step count are refused as bad input, not as
    /// internal failures, and neither leaves a journal row behind.
    #[tokio::test]
    async fn bad_input_is_reported_as_bad_input() {
        let directory = TempDir::new().unwrap();
        let app = served(&directory).await;
        let state = app.state::<RuntimeState>();

        let error = start_hello(state.clone(), 0).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert!(
            error
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("600")),
            "the message must name the bound: {error:?}"
        );

        let error = cancel(state.clone(), "not-a-job".to_owned())
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);

        assert!(
            list(state.clone(), None).await.unwrap().is_empty(),
            "a refused request must not journal anything"
        );

        crate::runtime::shutdown(app.handle()).await;
    }

    /// Health reports the running process's own numbers, and an idle runtime has no work.
    #[tokio::test]
    async fn health_reports_the_running_process() {
        let directory = TempDir::new().unwrap();
        let app = served(&directory).await;
        let state = app.state::<RuntimeState>();

        let health = crate::system::health(state.clone()).await.unwrap();
        assert!(health.startup_millis < 60_000);
        assert_eq!(health.scheduler.live, 0);
        assert_eq!(health.scheduler.inflight, 0);
        assert!(!health.events.degraded);

        crate::runtime::shutdown(app.handle()).await;
    }

    /// PG-E2-06's acceptance: no command blocks for longer than 100 ms, measured while
    /// the scheduler is busy with real work.
    ///
    /// The budget is the documented one and not a tighter number, so the test fails on a
    /// regression rather than on a slow machine. This is the one test in the suite with a
    /// wall-clock assertion in it; ARCHITECTURE §7 rule 2 and R10 are why it exists, and
    /// the alternative — reviewing every command by eye — is what the rule was written to
    /// replace.
    #[tokio::test]
    async fn no_command_blocks_for_longer_than_the_budget() {
        const BUDGET: Duration = Duration::from_millis(100);
        const ROUNDS: u32 = 20;
        const BUSY_JOBS: u32 = 8;

        let directory = TempDir::new().unwrap();
        let app = served(&directory).await;
        let state = app.state::<RuntimeState>();

        // The load: eight jobs of fifty steps each (about half a second of work apiece),
        // so the scheduler and the single database writer are contended for the whole
        // measurement rather than idle while it runs.
        for _ in 0..BUSY_JOBS {
            let _ = start_hello(state.clone(), 50).await.unwrap();
        }

        let mut worst: Vec<(&'static str, Duration)> = Vec::new();
        let measured = tokio::time::timeout(Duration::from_secs(60), async {
            for _ in 0..ROUNDS {
                let started = Instant::now();
                let _ = crate::system::build_info(app.handle().clone()).unwrap();
                worst.push(("system::build_info", started.elapsed()));

                let started = Instant::now();
                let _ = crate::system::health(state.clone()).await.unwrap();
                worst.push(("system::health", started.elapsed()));

                let started = Instant::now();
                let _ = crate::system::log_tail(state.clone(), Some(10))
                    .await
                    .unwrap();
                worst.push(("system::log_tail", started.elapsed()));

                let started = Instant::now();
                let _ = list(state.clone(), Some(5)).await.unwrap();
                worst.push(("jobs::list", started.elapsed()));

                let started = Instant::now();
                let job = start_hello(state.clone(), 20).await.unwrap();
                worst.push(("jobs::start_hello", started.elapsed()));

                let started = Instant::now();
                cancel(state.clone(), job.job_id).await.unwrap();
                worst.push(("jobs::cancel", started.elapsed()));
            }
        })
        .await;
        assert!(
            measured.is_ok(),
            "a command did not answer at all; the surface is blocking, not slow"
        );

        // The evidence, so a failure says which command and by how much, and a pass is
        // inspectable with `--nocapture` rather than taken on faith.
        let mut slowest = ("none", Duration::ZERO);
        for (name, elapsed) in &worst {
            if *elapsed > slowest.1 {
                slowest = (name, *elapsed);
            }
        }
        for name in [
            "system::build_info",
            "system::health",
            "system::log_tail",
            "jobs::list",
            "jobs::start_hello",
            "jobs::cancel",
        ] {
            let command_worst = worst
                .iter()
                .filter(|(candidate, _)| *candidate == name)
                .map(|(_, elapsed)| *elapsed)
                .max()
                .unwrap_or_default();
            println!("{name}: worst of {ROUNDS} rounds {command_worst:?}");
        }
        assert!(
            slowest.1 < BUDGET,
            "`{}` took {:?}, over the {BUDGET:?} budget in ARCHITECTURE §7 rule 2",
            slowest.0,
            slowest.1
        );

        crate::runtime::shutdown(app.handle()).await;
    }

    /// A runtime that never started answers every command with a structured failure
    /// rather than reaching for state that was not installed, and the failure carries no
    /// filesystem path.
    #[tokio::test]
    async fn a_failed_startup_is_reported_by_every_command() {
        let app = tauri::test::mock_app();
        app.manage(RuntimeState::failed(
            "the library database could not be opened",
        ));
        let state = app.state::<RuntimeState>();

        let error = start_hello(state.clone(), 3).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::Internal);
        assert_eq!(
            error.cause.as_deref(),
            Some("the library database could not be opened")
        );
        assert!(
            error
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("did not finish starting")),
            "the message must say what to do: {error:?}"
        );

        let error = crate::system::health(state.clone()).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::Internal);
        let error = crate::system::log_tail(state.clone(), Some(10))
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Internal);
    }

    /// The log tail command answers with the shape the UI expects.
    #[tokio::test]
    async fn log_tail_answers_with_a_list() {
        let directory = TempDir::new().unwrap();
        let app = served(&directory).await;
        let state = app.state::<RuntimeState>();

        let lines = crate::system::log_tail(state.clone(), Some(10))
            .await
            .unwrap();
        assert!(lines.is_empty(), "nothing has been written: {lines:?}");

        crate::runtime::shutdown(app.handle()).await;
    }
}
