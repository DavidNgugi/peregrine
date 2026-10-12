//! The maintenance job: `VACUUM`, `ANALYZE`, the search index's `optimize`, and the retention
//! pass, under `DATA_MODEL` §6's one rule that outranks them all — never during playback.
//!
//! §6 gives a maintenance job three operations and one interval each:
//!
//! | Operation | §6's trigger |
//! | --- | --- |
//! | Retention (PG-E3-15) | daily |
//! | `VACUUM` | after bulk deletes exceeding 20% of the file, or monthly |
//! | `ANALYZE` | weekly, and after large imports |
//! | FTS index rebuild | on dataset reimport, or a `probe_version` schema change |
//!
//! and then the rule that decides whether any of them may run: "Automatic maintenance never
//! runs while playback is active, never while a download is latency-sensitive and the user has
//! enabled 'don't disturb downloads', and always yields to user-initiated work."
//!
//! ## The playback answer is the caller's, and it is asked before every step
//!
//! This crate may not name `peregrine-playback` (AGENTS R6), so it never asks the player
//! anything: [`Database::apply_maintenance`] takes a predicate, and whether it is true is the
//! playback session's own state. The answer is owned by the caller that may see both crates —
//! the maintenance job in `peregrine-app`, which is the only place the player and this adapter
//! meet. It is a predicate rather than a flag read once, because it is consulted before *each*
//! step: playback that starts while a run is in progress stops the run where it is instead of
//! being noticed only at the door.
//!
//! ## Planning and applying are separate, as they are for retention
//!
//! [`Database::plan_maintenance`] answers what §6 makes due at one instant and returns it as a
//! value — including the measurement the `VACUUM` decision turns on, the share of the file's
//! pages that are free, and the retention plan that step will run. [`Database::apply_maintenance`]
//! runs that plan rather than deciding again, so the plan a caller logged is the work that
//! happens. Planning writes nothing, which is why it is allowed while playback is active; the
//! gate belongs to the run.
//!
//! ## Time and the intervals arrive as values
//!
//! No clock is read here or below (AGENTS R14). The schema records no "last run" instant, and
//! nothing in the shipping binary schedules this job yet (PG-E3-16), so the weekly `ANALYZE`
//! and monthly `VACUUM` intervals are [`MaintenanceRequest`]'s, supplied by whoever schedules
//! the run. The fragmentation half of the `VACUUM` trigger needs no such answer: free pages are
//! a fact about the file, measured when the plan is taken.
//!
//! ## What was verified about `VACUUM`, rather than assumed
//!
//! * **It cannot run inside a transaction.** `SQLite` refuses it with "cannot VACUUM from
//!   within a transaction", so the writer dispatches it on the connection in autocommit, and
//!   no step of this job wraps it.
//! * **By itself it reclaims nothing on disk in WAL mode.** Measured on a file of 1120 pages
//!   with 995 of them free: after `VACUUM`, `page_count * page_size` was 1.0 MB while the main
//!   file was still 9.2 MB and the `-wal` still 9.8 MB. `PRAGMA wal_checkpoint(TRUNCATE)` is
//!   what then truncated the main file to 1.0 MB and the log to zero. So the step checkpoints
//!   after vacuuming; a checkpoint that cannot complete is logged and reported, and does not
//!   fail the run, because the compaction itself did happen.
//! * **Its temporary copy is held in memory.** `VACUUM` attaches a temporary database
//!   (`sqlite3RunVacuum`), this build has `SQLITE_TEMP_STORE 1`, and §5's PRAGMA set requires
//!   `temp_store = MEMORY`, so `sqlite3TempInMemory` is true and the copy is not written to a
//!   file. Measured: a 150 MB database grew the process by 75 MB and a 300 MB one by 96 MB,
//!   against 1–2 MB with `temp_store = FILE`. The cost is bounded by the database's live size,
//!   which is the reason `VACUUM` is scheduled only when there are pages to reclaim or the
//!   month has elapsed rather than on every run.
//! * **Other connections do not stop it.** A reader holding a snapshot on another connection
//!   did not block it, and the write path and the reader pool both kept working afterwards;
//!   both are asserted in `tests`.
//!
//! `VACUUM INTO` — the mechanism behind the pre-migration copy ([`crate::backup`]) — is not
//! used here. It writes a second file of the database's size, and replacing the live database
//! with it would need every connection closed and the file renamed underneath the open writer,
//! which is a lifecycle change this adapter does not have.
//!
//! ## What §6 names and this job does not do
//!
//! The second half of the playback rule — "never while a download is latency-sensitive and the
//! user has enabled 'don't disturb downloads'" — is not implemented: that setting is PG-E20-01's
//! and no download state reaches this adapter. §6's rebuild trigger is an event, not an
//! interval: a dataset reimport (PG-E6-08) or a change to what the index reads is what sets
//! [`MaintenanceRequest::reindex_requested`], and nothing in the shipping binary raises it yet.

use sqlx::Row as _;
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnection;
use tracing::{info, warn};

use crate::error::StorageError;
use crate::retention::{self, RetentionPlan};
use crate::writer::{WriteRequest, WriteResponse, WriterHandle};

/// The share of the file's pages that must be free before §6's `VACUUM` is worth its cost.
///
/// §6's word is "exceeding", so a file with exactly this share free is not vacuumed; one with
/// more than it is.
const VACUUM_FREE_PAGE_PERCENT: u64 = 20;

/// One operation a maintenance run performs, in the order a run performs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintenanceStep {
    /// The retention pass: `Database::plan_retention` and `Database::apply_retention`.
    ///
    /// It runs first because it is what frees the pages `VACUUM` reclaims, and because §6 gives
    /// it the shortest interval of anything here.
    Retention,
    /// Compact the database file and checkpoint the log, so the freed pages become bytes.
    Vacuum,
    /// `ANALYZE`, so the query planner's statistics describe the data that is now there.
    ///
    /// It runs after `VACUUM` rather than before: the file `VACUUM` rewrites is the one the
    /// statistics are about.
    Analyze,
    /// Rewrite the search index from its source tables, discarding whatever the index held.
    RebuildSearchIndex,
    /// Merge the search index's b-tree segments, which is `FTS5`'s `optimize`.
    ///
    /// It runs after a rebuild so the rebuilt index is left in one segment, and on every run
    /// because it is cheap and idempotent.
    OptimizeSearchIndex,
}

/// What a maintenance run was asked to consider, beyond what the database can measure itself.
///
/// Two of §6's triggers are intervals — weekly `ANALYZE`, monthly `VACUUM` — and the schema
/// records nothing about when this job last ran, so they arrive as values (AGENTS R14). The
/// caller is the scheduler, which does not exist yet (PG-E3-16); nothing here reads a clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaintenanceRequest {
    /// The instant the run is taken at, in Unix milliseconds. It is the instant the retention
    /// plan prunes against, and it is carried by the plan so applying it cannot drift.
    pub at_millis: i64,
    /// Whether §6's weekly `ANALYZE` interval has elapsed.
    pub analyze_due: bool,
    /// Whether §6's monthly `VACUUM` interval has elapsed.
    ///
    /// The other half of the trigger is not a caller's to supply: free pages are measured from
    /// the file when the plan is taken.
    pub vacuum_interval_due: bool,
    /// Whether the search index has to be rebuilt, because a dataset was reimported or the
    /// projection changed (§6; nothing in the shipping binary raises it yet).
    pub reindex_requested: bool,
}

/// What a maintenance run at one instant would do, before any of it happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaintenancePlan {
    at_millis: i64,
    free_page_percent: u64,
    retention: RetentionPlan,
    steps: Vec<MaintenanceStep>,
}

impl MaintenancePlan {
    /// The instant the plan was taken at, which is also the instant the retention step prunes
    /// against.
    #[must_use]
    pub const fn at_millis(&self) -> i64 {
        self.at_millis
    }

    /// The share of the file's pages that were free when the plan was taken, which is the
    /// measurement §6's "exceeding 20% of the file" turns on.
    ///
    /// It is reported rather than kept private because it is the plan's evidence: a caller
    /// reading a plan that does not include [`MaintenanceStep::Vacuum`] can see how close the
    /// file was. It is measured at plan time, so the pages a run's own retention pass frees are
    /// reclaimed by the next run's `VACUUM`; deciding it mid-run instead would make the plan a
    /// forecast rather than the thing that runs.
    #[must_use]
    pub const fn free_page_percent(&self) -> u64 {
        self.free_page_percent
    }

    /// The retention plan the retention step will apply.
    #[must_use]
    pub const fn retention(&self) -> &RetentionPlan {
        &self.retention
    }

    /// Every step the run will attempt, in the order it will attempt them.
    ///
    /// It always names retention and the index `optimize`, because both are due on every run;
    /// the rest appear only when their own trigger fired.
    #[must_use]
    pub fn steps(&self) -> &[MaintenanceStep] {
        &self.steps
    }
}

/// What a run did with one step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepOutcome {
    /// The step ran to completion.
    Ran,
    /// The step was not attempted, because playback was active when its turn came.
    Declined,
    /// The step failed; what the database said, and the run continued with the next step.
    Failed {
        /// The detail, for the log and the caller's report.
        detail: String,
    },
}

/// One step of a plan and what became of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRecord {
    step: MaintenanceStep,
    outcome: StepOutcome,
}

impl StepRecord {
    /// The step.
    #[must_use]
    pub const fn step(&self) -> MaintenanceStep {
        self.step
    }

    /// What became of it.
    #[must_use]
    pub const fn outcome(&self) -> &StepOutcome {
        &self.outcome
    }
}

/// What a run did: one record per step of the plan, in plan order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaintenanceOutcome {
    steps: Vec<StepRecord>,
}

impl MaintenanceOutcome {
    /// Every step the run considered, in plan order.
    #[must_use]
    pub fn steps(&self) -> &[StepRecord] {
        &self.steps
    }

    /// How many steps ran.
    #[must_use]
    pub fn ran(&self) -> usize {
        self.count(&StepOutcome::Ran)
    }

    /// How many steps were declined because playback was active.
    #[must_use]
    pub fn declined(&self) -> usize {
        self.count(&StepOutcome::Declined)
    }

    /// How many steps failed.
    #[must_use]
    pub fn failed(&self) -> usize {
        self.steps
            .iter()
            .filter(|record| matches!(record.outcome, StepOutcome::Failed { .. }))
            .count()
    }

    /// Whether the run performed no step at all, which is what a playback refusal looks like.
    #[must_use]
    pub fn did_nothing(&self) -> bool {
        self.ran() == 0
    }

    fn count(&self, outcome: &StepOutcome) -> usize {
        self.steps
            .iter()
            .filter(|record| &record.outcome == outcome)
            .count()
    }
}

/// Counts what a run at `request.at_millis` would do, without doing any of it.
///
/// It reads one row of page geometry and the retention counts, so it writes nothing and is
/// safe to take while playback is active.
pub(crate) async fn plan(
    pool: &SqlitePool,
    request: MaintenanceRequest,
) -> Result<MaintenancePlan, StorageError> {
    let (free_pages, page_count): (i64, i64) = sqlx::query_as(
        "SELECT freelist_count, page_count FROM pragma_freelist_count(), pragma_page_count()",
    )
    .fetch_one(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    let free_pages = u64::try_from(free_pages).unwrap_or_default();
    let page_count = u64::try_from(page_count).unwrap_or_default();

    let retention = retention::plan(pool, request.at_millis).await?;

    let mut steps = vec![MaintenanceStep::Retention];
    if vacuum_due(free_pages, page_count, request.vacuum_interval_due) {
        steps.push(MaintenanceStep::Vacuum);
    }
    if request.analyze_due {
        steps.push(MaintenanceStep::Analyze);
    }
    if request.reindex_requested {
        steps.push(MaintenanceStep::RebuildSearchIndex);
    }
    steps.push(MaintenanceStep::OptimizeSearchIndex);

    Ok(MaintenancePlan {
        at_millis: request.at_millis,
        free_page_percent: free_page_percent(free_pages, page_count),
        retention,
        steps,
    })
}

/// Applies `plan` through the single writer, asking `is_playing` before every step.
///
/// A step whose request fails is recorded and the run continues: no step of this job is a
/// precondition for another, so a `VACUUM` that could not complete must not lose the `ANALYZE`
/// behind it (the retention module's pattern). The one error returned is a stopped writer,
/// because then nothing can run at all.
pub(crate) async fn apply<F>(
    writer: &WriterHandle,
    plan: &MaintenancePlan,
    is_playing: &F,
) -> Result<MaintenanceOutcome, StorageError>
where
    F: Fn() -> bool + ?Sized,
{
    let mut steps = Vec::with_capacity(plan.steps.len());
    for &step in &plan.steps {
        let outcome = if is_playing() {
            StepOutcome::Declined
        } else {
            match step {
                MaintenanceStep::Retention => run_retention(writer, &plan.retention).await?,
                MaintenanceStep::Vacuum => run_request(writer, step, WriteRequest::Vacuum).await?,
                MaintenanceStep::Analyze => {
                    run_request(writer, step, WriteRequest::Analyze).await?
                }
                MaintenanceStep::RebuildSearchIndex => {
                    run_request(writer, step, WriteRequest::RebuildSearchIndex).await?
                }
                MaintenanceStep::OptimizeSearchIndex => {
                    run_request(writer, step, WriteRequest::OptimizeSearchIndex).await?
                }
            }
        };
        steps.push(StepRecord { step, outcome });
    }

    let outcome = MaintenanceOutcome { steps };
    info!(
        at_millis = plan.at_millis,
        free_page_percent = plan.free_page_percent,
        ran = outcome.ran(),
        declined = outcome.declined(),
        failed = outcome.failed(),
        "maintenance finished"
    );
    Ok(outcome)
}

/// Runs the retention step and reports it as a failure when a table could not be pruned.
///
/// The per-table counts stay in the retention module's own log line; what this adds is that a
/// partial sweep is visible in the outcome rather than only in the log.
async fn run_retention(
    writer: &WriterHandle,
    plan: &RetentionPlan,
) -> Result<StepOutcome, StorageError> {
    let outcome = retention::apply(writer, plan).await?;
    let failures = outcome.failures();
    if failures.is_empty() {
        return Ok(StepOutcome::Ran);
    }
    let tables = failures
        .iter()
        .map(|failure| failure.table)
        .collect::<Vec<_>>()
        .join(", ");
    Ok(StepOutcome::Failed {
        detail: format!(
            "retention could not prune {} of its tables: {tables}",
            failures.len()
        ),
    })
}

/// Sends one statement request and turns its answer into a step outcome.
async fn run_request(
    writer: &WriterHandle,
    step: MaintenanceStep,
    request: WriteRequest,
) -> Result<StepOutcome, StorageError> {
    match writer.send(request).await {
        Ok(WriteResponse::Applied) => Ok(StepOutcome::Ran),
        Ok(WriteResponse::SearchIndexRebuilt { indexed }) => {
            info!(indexed, "the search index was rebuilt");
            Ok(StepOutcome::Ran)
        }
        Ok(WriteResponse::Vacuumed { checkpointed }) => {
            if !checkpointed {
                warn!(
                    "the write-ahead log could not be checkpointed, so the compacted database \
                     is not yet smaller on disk"
                );
            }
            Ok(StepOutcome::Ran)
        }
        Ok(other) => Ok(StepOutcome::Failed {
            detail: format!("the writer answered `{step:?}` with {other:?}"),
        }),
        // A stopped writer is not one step's failure: nothing else can run either, and a list
        // of identical failures would hide that.
        Err(error @ StorageError::WriterStopped) => Err(error),
        Err(error) => {
            warn!(step = ?step, %error, "a maintenance step failed");
            Ok(StepOutcome::Failed {
                detail: error.to_string(),
            })
        }
    }
}

/// Compacts the database and checkpoints the log, reporting whether the log was truncated.
///
/// It runs on the writer's connection, which is in autocommit here: `VACUUM` refuses to run
/// inside a transaction, and the checkpoint must not be in one either. `SQLite` also refuses
/// `VACUUM` while another statement is in progress on the connection, which the single writer
/// guarantees cannot happen.
///
/// # Errors
///
/// The driver's error. A checkpoint that reports itself busy is not an error: it returns
/// `false`, and the compaction is still there to be checkpointed by the next one.
pub(crate) async fn vacuum(connection: &mut SqliteConnection) -> Result<bool, sqlx::Error> {
    sqlx::query("VACUUM").execute(&mut *connection).await?;
    let checkpoint = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .fetch_one(&mut *connection)
        .await?;
    Ok(checkpoint.try_get::<i64, _>(0)? == 0)
}

/// The share of `page_count` pages that are free, as a whole percentage, for a report.
///
/// A database with no pages yet has no free share rather than an undefined one. The decision
/// itself does not round: [`vacuum_due`] compares by cross-multiplication, so a share a fraction
/// over the threshold is not truncated down into a decision not to vacuum.
fn free_page_percent(free_pages: u64, page_count: u64) -> u64 {
    if page_count == 0 {
        return 0;
    }
    free_pages.saturating_mul(100) / page_count
}

/// Whether §6 makes `VACUUM` due: more than [`VACUUM_FREE_PAGE_PERCENT`] of the file is free,
/// or the monthly interval has elapsed.
fn vacuum_due(free_pages: u64, page_count: u64, interval_due: bool) -> bool {
    interval_due
        || free_pages.saturating_mul(100) > VACUUM_FREE_PAGE_PERCENT.saturating_mul(page_count)
}

#[cfg(test)]
mod tests;
