//! Retention: the deletion policy of `DATA_MODEL` §6, its plan, and its application.
//!
//! `DATA_MODEL` §6 names what may be dropped and how long it lives. This module is that policy
//! as data — [`RETENTION_POLICY`] holds one entry per table the section names — because the
//! distinction the policy turns on is *which table a row is in*, and a scattering of cutoffs
//! through the code is exactly what makes that distinction unauditable.
//!
//! **User data is never eligible.** Every [`RetentionPolicyEntry::Prune`] entry is a cache, an
//! audit log, or a lifecycle record §6 gives a window: a cached provider response, a finished
//! journal row, an event-log row, a notification the user has read or dismissed, and a record
//! of why one release was chosen. A watch position, a library root, a media file, a title, a
//! subtitle track, and a watchlist entry are not in the table, and the generated predicate
//! cannot reach a table the table does not name. `watch_progress`, `image_cache`, and
//! `download_job` are listed as [`RetentionPolicyEntry::Elsewhere`] so a reader sees the rest
//! of §6's eligible set and who bounds it, rather than having to infer that an omission was
//! deliberate.
//!
//! §6 also names tables this schema does not have yet: `monitor`, `monitor_event`, `mcp_call`,
//! `approval_request`, `share`, `share_library`, `share_entry`, `share_device`,
//! `account_unlock_attempt`, `content_request`, and `account_content_rule`. None is in the
//! policy, because a predicate for a table that does not exist cannot run; the migration that
//! adds one adds its policy entry, in the same change.
//!
//! Planning and deleting are separate steps (`AGENTS` R22): [`plan`] counts what a run would
//! remove and returns it as a value, and [`apply`] removes what the plan enumerated. The plan
//! carries the instant it was taken at and `apply` re-runs the same predicates with that same
//! instant, so the plan is what executes rather than a fresh decision taken mid-run — and no
//! clock is read here or below (`AGENTS` R14): time arrives as a parameter.
//!
//! Deletion runs on the single write connection (`ARCHITECTURE` §5) in bounded statements: at
//! most [`PRUNE_BATCH_ROWS`] rows per statement, so a sweep of a hundred thousand rows does
//! not hold the write lock against a user's progress write for its whole length. It is
//! best-effort in the way [`crate::backup`]'s rotation is: a table whose statement fails is
//! logged, recorded in the outcome, and the remaining tables still run.

use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnection;
use sqlx::sqlite::SqliteQueryResult;
use tracing::{info, warn};

use crate::error::StorageError;
use crate::writer::{WriteRequest, WriteResponse, WriterHandle};

/// How many rows one pruning statement removes at most.
///
/// The bound is what keeps a sweep from monopolising the single writer: a caller's progress
/// write waits behind one statement, not behind the whole table. It is also the unit of
/// best-effort failure, because `SQLite` applies one `DELETE` atomically and a row inside it
/// cannot be skipped.
pub(crate) const PRUNE_BATCH_ROWS: u64 = 500;

/// One day, as every window in §6 is stated in days.
const DAY_MILLIS: i64 = 24 * 60 * 60 * 1_000;

/// The window §6 gives a finished job, a read notification, and a release decision.
const THIRTY_DAYS: i64 = 30 * DAY_MILLIS;

/// The window §6 gives the event log and a dismissed notification.
const NINETY_DAYS: i64 = 90 * DAY_MILLIS;

/// The row count §6 caps the event log at, whichever bound bites first.
const EVENT_LOG_ROWS: u64 = 10_000;

/// One instant column and the age at which it makes a row eligible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeWindow {
    /// The column holding the instant, in Unix milliseconds.
    pub column: &'static str,
    /// How far behind the run's instant the column must be.
    ///
    /// Zero means the column *is* the row's expiry: §6 gives `provider_cache` a TTL per
    /// resource class, that TTL was applied by whichever write stored the row, and the row is
    /// eligible the moment its own `expires_at` passes.
    pub millis: i64,
}

/// A column that has to hold one of `values` before a row is eligible at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Requirement {
    /// The column holding the value.
    pub column: &'static str,
    /// The accepted spellings, which are the ones the column's `CHECK` constraint allows.
    pub values: &'static [&'static str],
}

/// What the daily job does with one table `DATA_MODEL` §6 names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionRule {
    /// The table the eligible rows live in.
    pub table: &'static str,
    /// What a row is, in §6's words.
    pub describes: &'static str,
    /// Values a row must hold before it is considered. Every entry is required.
    pub requires: &'static [Requirement],
    /// Instants that make a row eligible: a row is eligible when *any* is past its window.
    pub windows: &'static [TimeWindow],
    /// Keep the newest this many rows by rowid, whatever their age.
    pub keep_newest: Option<u64>,
}

/// One entry of the retention policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionPolicyEntry {
    /// The job deletes the eligible rows.
    Prune(RetentionRule),
    /// §6 names the data, but its bound is not an age this job applies. The entry records
    /// where the bound is, so a reader can see that the omission is deliberate.
    Elsewhere {
        /// The table.
        table: &'static str,
        /// What a row is.
        describes: &'static str,
        /// What bounds it instead.
        bound: &'static str,
    },
}

/// The whole retention policy, `DATA_MODEL` §6 in schema order.
///
/// Adding a `Prune` entry to this table is the act of making user data deletable, which is why
/// it is one reviewed constant rather than a call site per table: `retention::tests` fails if
/// this list stops matching the tables that test seeds as eligible.
pub const RETENTION_POLICY: &[RetentionPolicyEntry] = &[
    RetentionPolicyEntry::Prune(RetentionRule {
        table: "provider_cache",
        describes: "a cached provider response",
        requires: &[],
        // §6's window is a TTL per resource class, applied when the row was written; the
        // remaining age here is therefore zero and the row's own expiry decides.
        windows: &[TimeWindow {
            column: "expires_at",
            millis: 0,
        }],
        keep_newest: None,
    }),
    RetentionPolicyEntry::Prune(RetentionRule {
        table: "job",
        describes: "a job-journal row that reached a terminal state",
        requires: &[Requirement {
            column: "state",
            values: &["succeeded", "failed", "canceled"],
        }],
        // A terminal row records when it finished. `created_at` is the second window for a
        // row that never did — one canceled before its first attempt — so age is decided by
        // whichever instant the row has, and a `NULL` `finished_at` cannot exempt it.
        windows: &[
            TimeWindow {
                column: "finished_at",
                millis: THIRTY_DAYS,
            },
            TimeWindow {
                column: "created_at",
                millis: THIRTY_DAYS,
            },
        ],
        keep_newest: None,
    }),
    RetentionPolicyEntry::Prune(RetentionRule {
        table: "event_log",
        describes: "an event-log row",
        requires: &[],
        windows: &[TimeWindow {
            column: "at",
            millis: NINETY_DAYS,
        }],
        keep_newest: Some(EVENT_LOG_ROWS),
    }),
    RetentionPolicyEntry::Prune(RetentionRule {
        table: "notification",
        describes: "a notification-centre row the user has read or dismissed",
        requires: &[],
        // An unread, undismissed row is the centre's content and is not eligible at any age;
        // a `NULL` column never satisfies the comparison, which is what keeps it.
        windows: &[
            TimeWindow {
                column: "read_at",
                millis: THIRTY_DAYS,
            },
            TimeWindow {
                column: "dismissed_at",
                millis: NINETY_DAYS,
            },
        ],
        keep_newest: None,
    }),
    RetentionPolicyEntry::Prune(RetentionRule {
        table: "release_decision",
        describes: "a record of why one release was or was not chosen",
        requires: &[],
        windows: &[TimeWindow {
            column: "created_at",
            millis: THIRTY_DAYS,
        }],
        keep_newest: None,
    }),
    RetentionPolicyEntry::Elsewhere {
        table: "image_cache",
        describes: "a cached artwork or thumbnail file",
        bound: "the configured byte cap, evicted after each write batch (PG-E6-09)",
    },
    RetentionPolicyEntry::Elsewhere {
        table: "download_job",
        describes: "a completed download job",
        bound: "kept until the user clears history, and while a row is seeding (PG-E13-10)",
    },
    RetentionPolicyEntry::Elsewhere {
        table: "watch_progress",
        describes: "what an account has watched and where it stopped",
        bound: "forever: this is user data, not a cache",
    },
];

/// One table's rows: what a plan would remove, or what a run did remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableRemoval {
    rule: &'static RetentionRule,
    rows: u64,
}

impl TableRemoval {
    /// The table the rows are in.
    #[must_use]
    pub const fn table(self) -> &'static str {
        self.rule.table
    }

    /// What a row is, in §6's words.
    #[must_use]
    pub const fn describes(self) -> &'static str {
        self.rule.describes
    }

    /// How many rows.
    #[must_use]
    pub const fn rows(self) -> u64 {
        self.rows
    }
}

/// What a retention run at one instant would remove, before anything is removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionPlan {
    at_millis: i64,
    removals: Vec<TableRemoval>,
}

impl RetentionPlan {
    /// The instant the plan was taken at, which is also the instant the applied plan prunes
    /// against.
    #[must_use]
    pub const fn at_millis(&self) -> i64 {
        self.at_millis
    }

    /// One entry per prunable table, in policy order, including tables that would lose no rows.
    ///
    /// Keeping the zero entries is what makes the plan an enumeration of the policy rather
    /// than a list of hits: a caller logging it can see every window that was evaluated.
    #[must_use]
    pub fn removals(&self) -> &[TableRemoval] {
        &self.removals
    }

    /// The total rows the plan would remove.
    #[must_use]
    pub fn rows(&self) -> u64 {
        self.removals.iter().map(|removal| removal.rows).sum()
    }

    /// Whether the plan would remove nothing at all.
    #[must_use]
    pub fn removes_nothing(&self) -> bool {
        self.removals.iter().all(|removal| removal.rows == 0)
    }
}

/// A table the run could not prune.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionFailure {
    /// The table.
    pub table: &'static str,
    /// What the database said, for the log and the caller's report.
    pub detail: String,
}

/// What a run removed, and what it could not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionOutcome {
    removed: Vec<TableRemoval>,
    failures: Vec<RetentionFailure>,
}

impl RetentionOutcome {
    /// One entry per prunable table, in policy order, with the rows actually removed.
    #[must_use]
    pub fn removed(&self) -> &[TableRemoval] {
        &self.removed
    }

    /// The tables the run could not prune, which the next run will try again.
    #[must_use]
    pub fn failures(&self) -> &[RetentionFailure] {
        &self.failures
    }

    /// The total rows removed.
    #[must_use]
    pub fn rows(&self) -> u64 {
        self.removed.iter().map(|removal| removal.rows).sum()
    }
}

/// Counts what a run at `at_millis` would remove, without removing anything.
///
/// The plan is strict rather than best-effort: a rule whose count cannot be taken fails the
/// whole plan, because a plan that silently omitted a table would under-report what the caller
/// is about to authorise (`AGENTS` R22).
pub(crate) async fn plan(pool: &SqlitePool, at_millis: i64) -> Result<RetentionPlan, StorageError> {
    let mut removals = Vec::new();
    for entry in RETENTION_POLICY {
        let RetentionPolicyEntry::Prune(rule) = entry else {
            continue;
        };
        let sql = format!(
            "SELECT COUNT(*) FROM \"{}\" WHERE {}",
            rule.table,
            eligibility(rule)
        );
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .bind(at_millis)
            .fetch_one(pool)
            .await
            .map_err(StorageError::from_sqlx)?;
        removals.push(TableRemoval {
            rule,
            rows: u64::try_from(count).unwrap_or_default(),
        });
    }
    Ok(RetentionPlan {
        at_millis,
        removals,
    })
}

/// Applies `plan` through the single writer, bounded per statement and best-effort per table.
///
/// Each table is pruned up to the count the plan enumerated, so a table that is somehow
/// refilled while it is being pruned cannot hold the run open.
pub(crate) async fn apply(
    writer: &WriterHandle,
    plan: &RetentionPlan,
) -> Result<RetentionOutcome, StorageError> {
    let mut removed = Vec::new();
    let mut failures = Vec::new();
    for removal in plan.removals() {
        let mut rows = 0_u64;
        while rows < removal.rows {
            let batch = (removal.rows - rows).min(PRUNE_BATCH_ROWS);
            let outcome = writer
                .send(WriteRequest::PruneRetention {
                    rule: removal.rule,
                    at_millis: plan.at_millis,
                    limit: batch,
                })
                .await;
            match outcome {
                Ok(WriteResponse::RetentionPruned { rows: pruned }) => {
                    rows += pruned;
                    if pruned < batch {
                        break;
                    }
                }
                Ok(other) => {
                    failures.push(RetentionFailure {
                        table: removal.rule.table,
                        detail: format!("the writer answered `prune_retention` with {other:?}"),
                    });
                    break;
                }
                // A stopped writer is not one table's failure: nothing else can be pruned
                // either, and a list of identical failures would hide that.
                Err(error @ StorageError::WriterStopped) => return Err(error),
                Err(error) => {
                    warn!(
                        table = removal.rule.table,
                        %error,
                        "retention could not prune a table"
                    );
                    failures.push(RetentionFailure {
                        table: removal.rule.table,
                        detail: error.to_string(),
                    });
                    break;
                }
            }
        }
        removed.push(TableRemoval {
            rule: removal.rule,
            rows,
        });
    }

    let pruned = removed.iter().map(|removal| removal.rows).sum::<u64>();
    if pruned > 0 || !failures.is_empty() {
        info!(
            rows = pruned,
            failures = failures.len(),
            "retention run finished"
        );
    }
    Ok(RetentionOutcome { removed, failures })
}

/// Removes at most `limit` of the rows `rule` makes eligible, and reports how many went.
///
/// The statement is assembled from the policy's own constants — a table name, column names,
/// and ages in milliseconds — so it carries no text a caller supplied, and the
/// [`sqlx::AssertSqlSafe`] at the call site records that audit.
pub(crate) async fn delete_batch(
    connection: &mut SqliteConnection,
    rule: &RetentionRule,
    at_millis: i64,
    limit: u64,
) -> Result<u64, sqlx::Error> {
    let sql = format!(
        "DELETE FROM \"{table}\" WHERE rowid IN \
         (SELECT rowid FROM \"{table}\" WHERE {eligibility} LIMIT ?2)",
        table = rule.table,
        eligibility = eligibility(rule)
    );
    let result: SqliteQueryResult = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(at_millis)
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .execute(&mut *connection)
        .await?;
    Ok(result.rows_affected())
}

/// The `WHERE` clause that selects the rows `rule` makes eligible, with `?1` the run's instant.
fn eligibility(rule: &RetentionRule) -> String {
    let mut terms: Vec<String> = rule
        .requires
        .iter()
        .map(|required| format!("{} IN ({})", required.column, quoted(required.values)))
        .collect();

    // A `NULL` column never satisfies a comparison, which is what keeps a row whose instant
    // has not been recorded — an unread notification, a job that never finished.
    let aged: Vec<String> = rule
        .windows
        .iter()
        .map(|window| format!("{} <= ?1 - {}", window.column, window.millis))
        .chain(rule.keep_newest.map(|keep| {
            format!(
                "rowid <= (SELECT MAX(rowid) FROM \"{}\") - {keep}",
                rule.table
            )
        }))
        .collect();
    if !aged.is_empty() {
        terms.push(format!("({})", aged.join(" OR ")));
    }
    terms.join(" AND ")
}

/// The values of a `CHECK`-constrained column as a SQL list, from a compile-time constant.
fn quoted(values: &[&str]) -> String {
    values
        .iter()
        .map(|value| format!("'{value}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests;
