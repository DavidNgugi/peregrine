//! Maintenance tests: the acceptance is that a run declines and writes nothing while playback
//! is active, and that the assertion is not vacuous.
//!
//! `maintenance_declines_and_writes_nothing_while_playback_is_active` is that acceptance, and it
//! is built to fail from either side. The first arm runs a fully due plan with the predicate
//! saying a playback is live and asserts the database is byte-identical afterwards; the second
//! arm runs the *same plan* on the same untouched database with the predicate saying nothing is
//! playing and asserts every step ran and left its own evidence. A build that ignored the
//! predicate would do that work in the first arm and fail the identity comparison; a fixture
//! with no work in it would make the first arm pass for the wrong reason, which the plan's own
//! step list is asserted against before either arm runs.
//!
//! `playback_that_starts_mid_run_stops_the_remaining_steps` is the other half of "yields to
//! user-initiated work": the predicate is consulted before *every* step, not once at the door.
//!
//! The `VACUUM` assertions are the measurements this module's documentation records: a
//! `VACUUM` on its own leaves the file at its old size in WAL mode, and only the checkpoint
//! that follows it makes the reclaimed bytes real.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use peregrine_domain::{AccountId, AccountScope};
use sqlx::Connection as _;
use tempfile::TempDir;

use super::{
    MaintenanceRequest, MaintenanceStep, StepOutcome, free_page_percent, vacuum, vacuum_due,
};
use crate::database::open_writer_connection;
use crate::page::PageRequest;
use crate::retention::{RETENTION_POLICY, RetentionPolicyEntry};
use crate::test_support::{
    NOW, count, database, days_before, path_of, seed, snapshot, user_data_rows,
};

/// The tables a maintenance run is allowed to change, and which step changes each one.
///
/// It is written out rather than derived, for the reason retention's `ELIGIBLE` is: a guard that
/// asked the code what it may change would agree with any code. A step that starts writing a
/// table this list does not name fails
/// `maintenance_changes_only_the_tables_this_suite_names`, so extending the job is a deliberate
/// edit to the guard in the same change.
const MAY_CHANGE: [&str; 11] = [
    // The retention policy's five (`DATA_MODEL` §6): the `Retention` step.
    "event_log",
    "job",
    "notification",
    "provider_cache",
    "release_decision",
    // `ANALYZE`, which writes both statistics tables this build has.
    "sqlite_stat1",
    "sqlite_stat4",
    // The search index's shadow tables: `RebuildSearchIndex` and `OptimizeSearchIndex`.
    "title_fts_config",
    "title_fts_data",
    "title_fts_docsize",
    "title_fts_idx",
];

/// Nothing is playing.
fn idle() -> impl Fn() -> bool {
    || false
}

/// The request that asks for nothing beyond what every run does: retention and the index
/// `optimize`.
fn daily() -> MaintenanceRequest {
    MaintenanceRequest {
        at_millis: NOW,
        analyze_due: false,
        vacuum_interval_due: false,
        reindex_requested: false,
    }
}

/// The request a scheduler makes when the weekly `ANALYZE` is due and nothing else is asked for.
fn weekly() -> MaintenanceRequest {
    MaintenanceRequest {
        at_millis: NOW,
        analyze_due: true,
        vacuum_interval_due: false,
        reindex_requested: false,
    }
}

/// The request that leaves nothing to measurement: every interval has elapsed and a rebuild has
/// been asked for.
fn everything_due() -> MaintenanceRequest {
    MaintenanceRequest {
        at_millis: NOW,
        analyze_due: true,
        vacuum_interval_due: true,
        reindex_requested: true,
    }
}

/// A thousand expired cached responses of 4 KB each: retention work, and enough bytes that
/// removing them frees far more than §6's 20% of the file.
fn expired_cache_rows() -> Vec<String> {
    vec![format!(
        "INSERT INTO provider_cache (key, provider, resource, fetched_at, expires_at, \
         schema_version, body, size_bytes) \
         SELECT printf('cache.%04d', i), 'tmdb', 'movie', {}, {}, 1, randomblob(4000), 4000 FROM \
         (WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 1000) \
          SELECT i FROM n)",
        days_before(40),
        days_before(1)
    )]
}

/// The same rows with all but a hundred already deleted, so the file has free pages for `VACUUM`
/// to reclaim before any maintenance runs at all.
fn freed_cache_rows() -> Vec<String> {
    let mut rows = expired_cache_rows();
    rows.push("DELETE FROM provider_cache WHERE key > 'cache.0100'".to_owned());
    rows
}

/// A notification the user read a month and a half ago, which §6 makes retention-eligible.
fn expired_notification() -> String {
    format!(
        "INSERT INTO notification (event, title, read_at, dismissed_at, created_at) VALUES \
         ('download.completed', 'maintenance.test', {}, NULL, {})",
        days_before(40),
        days_before(400)
    )
}

/// What the database occupies on disk and what it says about itself.
///
/// All three numbers are needed together, because in WAL mode they disagree and the
/// disagreement is the point: a database that has never been checkpointed keeps one page in its
/// main file and everything else in the log, and a `VACUUM`'s compaction lives in the log until
/// the checkpoint that follows it. Measuring only the main file would read as *growth* after a
/// vacuum.
struct Footprint {
    /// The main database file's bytes.
    main: u64,
    /// The write-ahead log's bytes, or zero when there is none.
    wal: u64,
    /// The database's own `page_count * page_size`.
    geometry: u64,
}

impl Footprint {
    /// The bytes the database and its log occupy together.
    const fn total(&self) -> u64 {
        self.main + self.wal
    }
}

/// Reads the three numbers above.
async fn footprint(path: &Path) -> Footprint {
    let mut log = path.as_os_str().to_owned();
    log.push("-wal");
    let main = tokio::fs::metadata(path)
        .await
        .map_or(0, |metadata| metadata.len());
    let wal = tokio::fs::metadata(Path::new(&log))
        .await
        .map_or(0, |metadata| metadata.len());
    let mut connection = open_writer_connection(path).await.unwrap();
    let (pages, size): (i64, i64) =
        sqlx::query_as("SELECT page_count, page_size FROM pragma_page_count(), pragma_page_size()")
            .fetch_one(&mut connection)
            .await
            .unwrap();
    connection.close().await.unwrap();
    Footprint {
        main,
        wal,
        geometry: u64::try_from(pages).unwrap_or_default()
            * u64::try_from(size).unwrap_or_default(),
    }
}

/// A scope that hides nothing, as the search read needs.
fn any_account() -> AccountScope {
    AccountScope::unrestricted(AccountId::new(1).expect("account one exists"))
}

/// The tables that differ between two snapshots, in name order.
fn changed(before: &BTreeMap<String, String>, after: &BTreeMap<String, String>) -> Vec<String> {
    let mut changed: Vec<String> = before
        .keys()
        .chain(after.keys())
        .filter(|table| before.get(*table) != after.get(*table))
        .cloned()
        .collect();
    changed.sort_unstable();
    changed.dedup();
    changed
}

/// The acceptance: a run with playback active declines every step and writes nothing.
///
/// The second arm is what makes the first assertion worth anything: the same plan, on the same
/// database, does the full run when nothing is playing.
#[tokio::test]
async fn maintenance_declines_and_writes_nothing_while_playback_is_active() {
    let directory = TempDir::new().unwrap();
    let database = database(&directory).await;
    let path = path_of(&directory);
    let mut statements = user_data_rows();
    statements.extend(freed_cache_rows());
    seed(&path, &statements).await;

    let plan = database.plan_maintenance(everything_due()).await.unwrap();
    assert_eq!(
        plan.steps(),
        &[
            MaintenanceStep::Retention,
            MaintenanceStep::Vacuum,
            MaintenanceStep::Analyze,
            MaintenanceStep::RebuildSearchIndex,
            MaintenanceStep::OptimizeSearchIndex,
        ],
        "the fixture must give every step something to do, or the refusal below proves nothing"
    );
    let before = snapshot(&path, &[]).await;
    let unfootprinted = footprint(&path).await;

    let outcome = database.apply_maintenance(&plan, || true).await.unwrap();

    assert!(
        outcome.did_nothing(),
        "a playback must leave every step unrun: {:?}",
        outcome.steps()
    );
    assert_eq!(outcome.declined(), plan.steps().len());
    assert_eq!(outcome.failed(), 0, "{:?}", outcome.steps());
    assert!(
        outcome
            .steps()
            .iter()
            .all(|record| record.outcome() == &StepOutcome::Declined),
        "{:?}",
        outcome.steps()
    );
    assert_eq!(
        snapshot(&path, &[]).await,
        before,
        "a refused run must leave every table byte-identical"
    );
    let after = footprint(&path).await;
    assert_eq!(
        (after.main, after.wal, after.geometry),
        (
            unfootprinted.main,
            unfootprinted.wal,
            unfootprinted.geometry
        ),
        "a refused run must not move a page"
    );

    // The other arm: the same plan, the same database, nothing playing. Every step runs, and
    // each one leaves its own evidence.
    let outcome = database.apply_maintenance(&plan, idle()).await.unwrap();

    assert_eq!(outcome.ran(), plan.steps().len(), "{:?}", outcome.steps());
    assert_eq!(outcome.failed(), 0, "{:?}", outcome.steps());
    assert_eq!(
        count(&path, "SELECT COUNT(*) FROM provider_cache").await,
        0,
        "retention took the expired rows"
    );
    assert_eq!(
        count(
            &path,
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'sqlite_stat1'"
        )
        .await,
        1,
        "ANALYZE wrote statistics"
    );
    let hits = database
        .search(any_account(), "A Show", PageRequest::first(10))
        .await
        .unwrap();
    assert_eq!(hits.len(), 1, "the rebuild put the title back in the index");
    let after = footprint(&path).await;
    assert!(
        after.total() < unfootprinted.total() && after.geometry < unfootprinted.geometry,
        "the vacuum must have reclaimed the freed pages: {} bytes / {} of geometry became {} / {}",
        unfootprinted.total(),
        unfootprinted.geometry,
        after.total(),
        after.geometry
    );
}

/// Yielding is not a door check: playback that starts while a run is in progress stops the run
/// where it is.
#[tokio::test]
async fn playback_that_starts_mid_run_stops_the_remaining_steps() {
    let directory = TempDir::new().unwrap();
    let database = database(&directory).await;
    let path = path_of(&directory);
    seed(&path, &expired_cache_rows()).await;

    let plan = database.plan_maintenance(weekly()).await.unwrap();
    assert_eq!(
        plan.steps(),
        &[
            MaintenanceStep::Retention,
            MaintenanceStep::Analyze,
            MaintenanceStep::OptimizeSearchIndex,
        ],
        "the premise: a run with no free pages and no rebuild asked for has three steps"
    );

    // The predicate reports that nothing is playing once — for the first step — and a playback
    // from the second step on, which is a playback starting between two steps of one run.
    let asked = AtomicUsize::new(0);
    let outcome = database
        .apply_maintenance(&plan, || asked.fetch_add(1, Ordering::SeqCst) > 0)
        .await
        .unwrap();

    assert_eq!(
        outcome.steps()[0].outcome(),
        &StepOutcome::Ran,
        "the first step ran before the playback started: {:?}",
        outcome.steps()
    );
    for record in &outcome.steps()[1..] {
        assert_eq!(
            record.outcome(),
            &StepOutcome::Declined,
            "every step after the playback started must be declined: {:?}",
            outcome.steps()
        );
    }
    assert_eq!(
        count(&path, "SELECT COUNT(*) FROM provider_cache").await,
        0,
        "the step that ran did its work"
    );
    assert_eq!(
        count(
            &path,
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'sqlite_stat1'"
        )
        .await,
        0,
        "the step that was declined did nothing"
    );
}

/// The empty case: a database with no rows, no free pages and a due `ANALYZE` runs exactly the
/// steps that are due, fails nothing, and reports no free share.
#[tokio::test]
async fn an_empty_database_runs_only_the_steps_that_are_due() {
    let directory = TempDir::new().unwrap();
    let database = database(&directory).await;
    let path = path_of(&directory);

    let plan = database.plan_maintenance(weekly()).await.unwrap();

    assert_eq!(plan.free_page_percent(), 0, "nothing has been deleted yet");
    assert_eq!(
        plan.steps(),
        &[
            MaintenanceStep::Retention,
            MaintenanceStep::Analyze,
            MaintenanceStep::OptimizeSearchIndex,
        ],
        "an empty database has no interval due and no rebuild asked for"
    );
    assert!(plan.retention().removes_nothing());
    assert_eq!(plan.at_millis(), NOW);

    let outcome = database.apply_maintenance(&plan, idle()).await.unwrap();

    assert_eq!(outcome.ran(), 3, "{:?}", outcome.steps());
    assert_eq!(outcome.failed(), 0, "{:?}", outcome.steps());
    assert_eq!(
        count(
            &path,
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'sqlite_stat1'"
        )
        .await,
        1,
        "ANALYZE ran on an empty schema too"
    );
}

/// The plan is an enumeration, not a list of hits: what each trigger makes due appears, and
/// nothing appears that is not due.
#[tokio::test]
async fn the_plan_lists_every_step_that_is_due_and_nothing_else() {
    let directory = TempDir::new().unwrap();
    let database = database(&directory).await;
    let path = path_of(&directory);
    seed(&path, &user_data_rows()).await;
    let before = snapshot(&path, &[]).await;

    let quiet = database.plan_maintenance(daily()).await.unwrap();

    assert_eq!(
        quiet.steps(),
        &[
            MaintenanceStep::Retention,
            MaintenanceStep::OptimizeSearchIndex
        ],
        "retention and the index optimize are due on every run; the intervals are the caller's"
    );
    assert_eq!(
        quiet.free_page_percent(),
        0,
        "the plan reports the measurement the VACUUM decision turned on"
    );
    assert_eq!(
        snapshot(&path, &[]).await,
        before,
        "planning is not doing: it writes nothing"
    );

    let all = database.plan_maintenance(everything_due()).await.unwrap();

    assert_eq!(
        all.steps(),
        &[
            MaintenanceStep::Retention,
            MaintenanceStep::Vacuum,
            MaintenanceStep::Analyze,
            MaintenanceStep::RebuildSearchIndex,
            MaintenanceStep::OptimizeSearchIndex,
        ],
        "the interval alone schedules VACUUM, with no free pages to measure"
    );
    assert_eq!(all.at_millis(), NOW);
    assert_eq!(all.retention().at_millis(), NOW);
}

/// The second run of the same request changes nothing, which is what makes the steps idempotent
/// rather than merely repeatable: `ANALYZE` rewrites statistics the planner already has, and
/// `optimize` has one segment left to merge into.
#[tokio::test]
async fn a_second_run_leaves_the_database_byte_identical() {
    let directory = TempDir::new().unwrap();
    let database = database(&directory).await;
    let path = path_of(&directory);
    seed(&path, &user_data_rows()).await;

    let first = database.plan_maintenance(weekly()).await.unwrap();
    let outcome = database.apply_maintenance(&first, idle()).await.unwrap();
    assert_eq!(outcome.failed(), 0, "{:?}", outcome.steps());
    let before = snapshot(&path, &[]).await;

    let second = database.plan_maintenance(weekly()).await.unwrap();
    assert_eq!(second.steps(), first.steps());
    let outcome = database.apply_maintenance(&second, idle()).await.unwrap();

    assert_eq!(outcome.ran(), second.steps().len(), "{:?}", outcome.steps());
    assert_eq!(outcome.failed(), 0, "{:?}", outcome.steps());
    let after = snapshot(&path, &[]).await;
    assert_eq!(
        after,
        before,
        "the second run must not change a byte: {:?}",
        changed(&before, &after)
    );
}

/// A step that fails is reported and the run continues: no step here is another's precondition.
///
/// The failure is made real by dropping the table the retention plan counted, between planning
/// and applying — which is also what proves the plan, not a fresh decision, is what runs.
#[tokio::test]
async fn one_step_failing_does_not_stop_the_others() {
    let directory = TempDir::new().unwrap();
    let database = database(&directory).await;
    let path = path_of(&directory);
    let mut statements = user_data_rows();
    statements.push(expired_notification());
    statements.extend(freed_cache_rows());
    seed(&path, &statements).await;

    let plan = database.plan_maintenance(everything_due()).await.unwrap();
    assert!(
        plan.retention()
            .removals()
            .iter()
            .any(|removal| removal.table() == "notification" && removal.rows() == 1),
        "the plan must have counted the notification row: {:?}",
        plan.retention().removals()
    );
    let before = footprint(&path).await;
    seed(&path, &["DROP TABLE notification".to_owned()]).await;

    let outcome = database.apply_maintenance(&plan, idle()).await.unwrap();

    let failure = outcome
        .steps()
        .iter()
        .find(|record| matches!(record.outcome(), StepOutcome::Failed { .. }))
        .expect("retention cannot prune a table that is gone");
    assert_eq!(failure.step(), MaintenanceStep::Retention);
    let StepOutcome::Failed { detail } = failure.outcome() else {
        unreachable!("the record was found by its failure")
    };
    assert!(
        detail.contains("notification"),
        "the failure must name the table: {detail}"
    );
    assert_eq!(
        outcome.ran(),
        plan.steps().len() - 1,
        "every other step still ran: {:?}",
        outcome.steps()
    );
    assert_eq!(
        count(
            &path,
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'sqlite_stat1'"
        )
        .await,
        1,
        "the failed step did not stop ANALYZE"
    );
    let hits = database
        .search(any_account(), "A Show", PageRequest::first(10))
        .await
        .unwrap();
    assert_eq!(hits.len(), 1, "nor the rebuild");
    let after = footprint(&path).await;
    assert!(
        after.total() < before.total(),
        "nor the vacuum: {} bytes became {}",
        before.total(),
        after.total()
    );
}

/// The acceptance's other half: a full maintenance run changes the tables its own steps are
/// named for and nothing else. Every user-data table is seeded with rows old enough that a
/// sweep by age alone would take them, so a step that reached one shows up as a table this
/// suite does not name.
#[tokio::test]
async fn maintenance_changes_only_the_tables_this_suite_names() {
    let directory = TempDir::new().unwrap();
    let database = database(&directory).await;
    let path = path_of(&directory);
    let mut statements = user_data_rows();
    statements.extend(freed_cache_rows());
    seed(&path, &statements).await;
    let before = snapshot(&path, &[]).await;

    let plan = database.plan_maintenance(everything_due()).await.unwrap();
    assert_eq!(plan.steps().len(), 5, "{:?}", plan.steps());
    let outcome = database.apply_maintenance(&plan, idle()).await.unwrap();
    assert_eq!(outcome.failed(), 0, "{:?}", outcome.steps());

    let after = snapshot(&path, &[]).await;
    let touched = changed(&before, &after);

    assert!(
        !touched.is_empty(),
        "the guard must cover a run that did something"
    );
    for table in &touched {
        assert!(
            MAY_CHANGE.contains(&table.as_str()),
            "maintenance changed `{table}`, which this suite does not name as a table a step \
             may write; the guard and the job must change together"
        );
    }

    // Every retention-eligible table is on that list, so a policy that grew a sixth entry
    // without this suite noticing fails here.
    for entry in RETENTION_POLICY {
        if let RetentionPolicyEntry::Prune(rule) = entry {
            assert!(
                MAY_CHANGE.contains(&rule.table),
                "`{}` is retention-eligible and not named here",
                rule.table
            );
        }
    }

    // The user data is there and byte-identical. These are the tables the fixture wrote rows
    // into that no step is allowed near.
    for table in [
        "title",
        "title_alias",
        "watch_progress",
        "watchlist_entry",
        "media_file",
        "media_item",
        "library_root",
        "subtitle_track",
        "settings",
        "secret_ref",
        "image_cache",
        "dataset_state",
    ] {
        assert!(
            before.contains_key(table),
            "the fixture must have seeded `{table}`"
        );
        assert_eq!(
            before.get(table),
            after.get(table),
            "`{table}` is user data and must not move"
        );
    }
}

/// §6's fragmentation trigger, at the boundary: "exceeding 20%" is more than a fifth of the
/// file's pages, so exactly a fifth is not enough.
#[test]
fn the_free_page_threshold_is_more_than_twenty_percent_of_the_file() {
    assert_eq!(free_page_percent(0, 0), 0, "no pages is not a share");
    assert_eq!(free_page_percent(0, 100), 0);
    assert_eq!(free_page_percent(19, 100), 19);
    assert_eq!(free_page_percent(20, 100), 20);
    assert_eq!(free_page_percent(21, 100), 21);
    // The rendering rounds down; the decision does not.
    assert_eq!(free_page_percent(205, 1000), 20);

    assert!(!vacuum_due(0, 0, false));
    assert!(!vacuum_due(19, 100, false));
    assert!(
        !vacuum_due(20, 100, false),
        "exactly 20% is not exceeding it"
    );
    assert!(vacuum_due(21, 100, false));
    assert!(vacuum_due(1, 4, false), "a quarter is over a fifth");
    assert!(vacuum_due(205, 1000, false), "and so is 20.5%");
    assert!(vacuum_due(0, 100, true), "the interval needs no free pages");
}

/// The pages a run's own retention pass frees are reclaimed by the next run, which is what the
/// plan measuring its share at plan time means in practice: a plan taken before the deletes
/// cannot know about them.
///
/// It is also the acceptance for `VACUUM` in this architecture: the compaction is in the log
/// until the checkpoint, so the assertion is on both the file's bytes and the page geometry.
#[tokio::test]
async fn the_pages_a_prune_frees_are_reclaimed_by_the_next_run() {
    let directory = TempDir::new().unwrap();
    let database = database(&directory).await;
    let path = path_of(&directory);
    seed(&path, &expired_cache_rows()).await;

    let first = database.plan_maintenance(weekly()).await.unwrap();
    assert!(
        !first.steps().contains(&MaintenanceStep::Vacuum),
        "there is nothing to reclaim before the prune: {}%",
        first.free_page_percent()
    );
    let outcome = database.apply_maintenance(&first, idle()).await.unwrap();
    assert_eq!(outcome.failed(), 0, "{:?}", outcome.steps());
    assert_eq!(
        count(&path, "SELECT COUNT(*) FROM provider_cache").await,
        0,
        "retention took every expired row"
    );

    let second = database.plan_maintenance(weekly()).await.unwrap();
    assert!(
        second.steps().contains(&MaintenanceStep::Vacuum),
        "the prune freed most of the file: {}%",
        second.free_page_percent()
    );
    let before = footprint(&path).await;
    let outcome = database.apply_maintenance(&second, idle()).await.unwrap();
    assert_eq!(outcome.failed(), 0, "{:?}", outcome.steps());
    let after = footprint(&path).await;

    assert!(
        after.geometry < before.geometry / 2,
        "the database's own geometry must shrink: {} became {}",
        before.geometry,
        after.geometry
    );
    assert!(
        after.total() < before.total() / 2,
        "and the bytes it occupies with it, which is what the checkpoint after the vacuum is \
         for: {} became {}",
        before.total(),
        after.total()
    );
    // The measured relationship, pinned: the checkpoint truncates the main file to exactly the
    // page geometry, so the compacted database is in the file rather than the log. The log is
    // not empty afterwards — the `optimize` step after the vacuum writes its own frames — but
    // what it holds are pages the compacted database produced, not the pages it rewrote.
    assert_eq!(
        after.main, after.geometry,
        "the checkpoint must leave the main file at exactly the page geometry"
    );
    assert!(
        after.wal < before.wal,
        "and the log must have shrunk from {} bytes to {}",
        before.wal,
        after.wal
    );
}

/// The premise the `Vacuum` request is built on, pinned so a future reader sees why the writer
/// dispatches it on the connection in autocommit and never wraps it in a transaction.
#[tokio::test]
async fn vacuum_refuses_to_run_inside_a_transaction() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("peregrine.sqlite");
    let mut connection = open_writer_connection(&path).await.unwrap();
    crate::migrations::apply(&mut connection, &path)
        .await
        .unwrap();

    let mut transaction = connection.begin().await.unwrap();
    let error = vacuum(&mut transaction)
        .await
        .expect_err("SQLite refuses VACUUM from within a transaction");
    transaction.rollback().await.unwrap();

    assert!(
        error
            .to_string()
            .contains("cannot VACUUM from within a transaction"),
        "the reason the writer does not open one: {error}"
    );
}
