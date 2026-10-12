//! Retention tests: the acceptance is that the eligible rows shrink and nothing else moves.
//!
//! `retention_prunes_the_eligible_rows_and_leaves_every_other_table_byte_identical` is that
//! acceptance. It seeds every eligible table with a row inside its window and a row past it,
//! seeds the user-data tables with rows old enough that any age-based sweep would take them,
//! and then compares a rendering of every other table in the schema before and after a run.
//!
//! The test is built to fail from either side of the line. Deleting a table from `ELIGIBLE`
//! makes the snapshot include a table the run pruned, so the identity assertion fails; adding a
//! table to `ELIGIBLE` fails `the_policy_prunes_exactly_the_tables_this_suite_calls_eligible`,
//! which ties the two lists together. Both mutations were run by hand while this was written,
//! and the failure each produced is in the commit body.

use std::collections::BTreeMap;
use std::path::Path;

use sqlx::Connection as _;
use sqlx::Row as _;
use sqlx::sqlite::SqliteConnection;
use tempfile::TempDir;

use super::{
    PRUNE_BATCH_ROWS, RETENTION_POLICY, Requirement, RetentionPolicyEntry, RetentionRule,
    TimeWindow, eligibility, quoted,
};
use crate::database::open_writer_connection;
use crate::jobs::JobPhase;
use crate::test_support::{NOW, database, path_of, seed};

/// The tables this suite treats as eligible, which is exactly what the policy must prune.
///
/// It is written out rather than derived from [`RETENTION_POLICY`] on purpose: a test that read
/// the policy to decide what may be deleted would agree with any policy, including one that
/// deleted the user's watch history.
const ELIGIBLE: [&str; 5] = [
    "event_log",
    "job",
    "notification",
    "provider_cache",
    "release_decision",
];

/// One day, as the fixtures count ages.
const DAY: i64 = 24 * 60 * 60 * 1_000;

/// An instant `days` before the run's instant; negative values are in the future.
const fn days_before(days: i64) -> i64 {
    NOW - days * DAY
}

/// A row in every eligible table: one past its window, one inside it, and — where the policy's
/// shape allows it — one that a naive age sweep would take and the policy must not.
fn eligible_rows() -> Vec<String> {
    vec![
        format!(
            "INSERT INTO provider_cache (key, provider, resource, fetched_at, expires_at, \
             schema_version, body, size_bytes) VALUES ('cache.expired', 'tmdb', 'movie', {}, \
             {}, 1, x'00', 1)",
            days_before(40),
            days_before(1)
        ),
        format!(
            "INSERT INTO provider_cache (key, provider, resource, fetched_at, expires_at, \
             schema_version, body, size_bytes) VALUES ('cache.live', 'tmdb', 'movie', {}, {}, 1, \
             x'00', 1)",
            NOW,
            days_before(-1)
        ),
        format!(
            "INSERT INTO job (kind, state, payload_json, created_at, finished_at) VALUES \
             ('job.expired', 'succeeded', '{{}}', {}, {})",
            days_before(400),
            days_before(40)
        ),
        // Cancelled before its first attempt: terminal, and no `finished_at` to judge it by.
        format!(
            "INSERT INTO job (kind, state, payload_json, created_at, finished_at) VALUES \
             ('job.canceled-before-it-started', 'canceled', '{{}}', {}, NULL)",
            days_before(400)
        ),
        format!(
            "INSERT INTO job (kind, state, payload_json, created_at, finished_at) VALUES \
             ('job.fresh', 'failed', '{{}}', {}, {})",
            days_before(1),
            days_before(1)
        ),
        // Not terminal, so §6 keeps it whatever its age.
        format!(
            "INSERT INTO job (kind, state, payload_json, created_at, finished_at) VALUES \
             ('job.running', 'running', '{{}}', {}, NULL)",
            days_before(400)
        ),
        format!(
            "INSERT INTO event_log (at, kind) VALUES ({}, 'event.expired')",
            days_before(100)
        ),
        // Exactly at the 90-day window: the comparison is `<=`, so this is eligible.
        format!(
            "INSERT INTO event_log (at, kind) VALUES ({}, 'event.boundary')",
            days_before(90)
        ),
        format!(
            "INSERT INTO event_log (at, kind) VALUES ({}, 'event.recent')",
            days_before(89)
        ),
        notification(
            "notification.read.expired",
            &days_before(40).to_string(),
            "NULL",
            days_before(400),
        ),
        notification(
            "notification.read.fresh",
            &days_before(1).to_string(),
            "NULL",
            days_before(400),
        ),
        notification(
            "notification.dismissed.expired",
            "NULL",
            &days_before(100).to_string(),
            days_before(400),
        ),
        // Dismissed inside the 90-day window, so it stays.
        notification(
            "notification.dismissed.fresh",
            "NULL",
            &days_before(80).to_string(),
            days_before(400),
        ),
        // Never read and never dismissed: the centre's content, kept at any age.
        notification("notification.unread", "NULL", "NULL", days_before(400)),
        format!(
            "INSERT INTO release_decision (title_id, release_title, score, rank_position, \
             reasons_json, created_at) VALUES (1, 'release.expired', 1.0, 1, '[]', {})",
            days_before(40)
        ),
        format!(
            "INSERT INTO release_decision (title_id, release_title, score, rank_position, \
             reasons_json, created_at) VALUES (1, 'release.recent', 1.0, 1, '[]', {})",
            days_before(1)
        ),
    ]
}

/// One `notification` row whose identity is its title.
///
/// `read_at` and `dismissed_at` are SQL expressions so a call site can leave one `NULL`, which is
/// the state §6 keeps at any age.
fn notification(title: &str, read_at: &str, dismissed_at: &str, created_at: i64) -> String {
    format!(
        "INSERT INTO notification (event, title, read_at, dismissed_at, created_at) VALUES \
         ('download.completed', '{title}', {read_at}, {dismissed_at}, {created_at})"
    )
}

/// Rows in the tables the policy must not touch, every one of them old enough that a sweep by
/// age alone would remove it.
fn user_data_rows() -> Vec<String> {
    let mut rows = catalogue_rows();
    rows.extend(library_rows());
    rows
}

/// The catalogue half of [`user_data_rows`]: identity, titles, and their children.
fn catalogue_rows() -> Vec<String> {
    let old = days_before(400);
    vec![
        format!(
            "INSERT INTO account (id, name, kind, is_default, created_at) VALUES (1, 'Adult', \
             'adult', 1, {old})"
        ),
        format!(
            "INSERT INTO title (id, kind, name, sort_name, content_rating, metadata_state, \
             created_at, updated_at) VALUES (1, 'show', 'A Show', 'A Show', 'TV-14', 'complete', \
             {old}, {old})"
        ),
        "INSERT INTO title_alias (title_id, alias) VALUES (1, 'An Alias')".to_owned(),
        format!(
            "INSERT INTO external_id (title_id, provider, value, fetched_at) VALUES (1, 'tmdb', \
             '1', {old})"
        ),
        "INSERT INTO genre (id, name) VALUES (1, 'Drama')".to_owned(),
        "INSERT INTO title_genre (title_id, genre_id) VALUES (1, 1)".to_owned(),
        "INSERT INTO keyword (id, name) VALUES (1, 'slow burn')".to_owned(),
        "INSERT INTO title_keyword (title_id, keyword_id) VALUES (1, 1)".to_owned(),
        "INSERT INTO company (id, name) VALUES (1, 'A Studio')".to_owned(),
        "INSERT INTO title_company (title_id, company_id, role) VALUES (1, 1, 'studio')".to_owned(),
        format!(
            "INSERT INTO person (id, name, sort_name, updated_at) VALUES (1, 'A Person', \
             'Person, A', {old})"
        ),
        "INSERT INTO credit (title_id, person_id, kind, order_index) VALUES (1, 1, 'cast', 0)"
            .to_owned(),
        format!(
            "INSERT INTO season (id, title_id, season_number, updated_at) VALUES (1, 1, 1, {old})"
        ),
        format!(
            "INSERT INTO episode (id, title_id, season_id, season_number, episode_number, \
             updated_at) VALUES (1, 1, 1, 1, 1, {old})"
        ),
        "INSERT INTO episode_ordering (episode_id, scheme, season_number, episode_number) \
         VALUES (1, 'aired', 1, 1)"
            .to_owned(),
    ]
}

/// The library, watch-state, acquisition, cache, and platform half of [`user_data_rows`].
fn library_rows() -> Vec<String> {
    let old = days_before(400);
    vec![
        format!(
            "INSERT INTO library_root (id, path, kind, created_at) VALUES (1, '/media', 'shows', \
             {old})"
        ),
        format!(
            "INSERT INTO media_item (id, title_id, state, origin, created_at, updated_at) VALUES \
             (1, 1, 'available', 'scan', {old}, {old})"
        ),
        format!(
            "INSERT INTO media_file (id, media_item_id, path, root_id, size_bytes, mtime_ms, \
             added_at, updated_at) VALUES (1, 1, '/media/s01e01.mkv', 1, 4096, {old}, {old}, \
             {old})"
        ),
        format!(
            "INSERT INTO subtitle_track (id, media_file_id, origin, format, language, path, \
             downloaded_at, last_used_at) VALUES (1, 1, 'external', 'srt', 'en', \
             '/media/s01e01.en.srt', {old}, {old})"
        ),
        format!(
            "INSERT INTO subtitle_download (id, media_file_id, provider, provider_subtitle_id, \
             language, score, created_at) VALUES (1, 1, 'opensubtitles', 'x', 'en', 1.0, {old})"
        ),
        format!(
            "INSERT INTO watch_progress (account_id, title_id, episode_id, position_ms, percent, \
             updated_at, watched_at) VALUES (1, 1, 1, 1000, 0.5, {old}, {old})"
        ),
        format!(
            "INSERT INTO watchlist_entry (account_id, title_id, added_at) VALUES (1, 1, {old})"
        ),
        format!(
            "INSERT INTO smart_collection (id, name, query_json, sort, created_at) VALUES (1, \
             'Unwatched', '{{}}', 'name', {old})"
        ),
        format!(
            "INSERT INTO quality_profile (id, name, rules_json, cutoff_rank, updated_at) VALUES \
             (1, 'Default', '{{}}', 1, {old})"
        ),
        format!(
            "INSERT INTO indexer (id, kind, name, last_check_at) VALUES (1, 'builtin', 'Built \
             in', {old})"
        ),
        format!(
            "INSERT INTO download_job (id, title_id, state, release_title, save_path, added_at, \
             completed_at) VALUES (1, 1, 'completed', 'A.Release', '/downloads', {old}, {old})"
        ),
        "INSERT INTO download_file (id, download_job_id, torrent_file_index, path, size_bytes) \
         VALUES (1, 1, 0, '/downloads/a.mkv', 4096)"
            .to_owned(),
        format!(
            "INSERT INTO image_cache (key, provider, path, variant, disk_path, bytes, created_at, \
             last_used_at) VALUES ('image.old', 'tmdb', '/p', 'poster', '/cache/p.webp', 4096, \
             {old}, {old})"
        ),
        "INSERT INTO image_ref (owner_kind, owner_id, role, provider, path, language) VALUES \
         ('title', 1, 'poster', 'tmdb', '/p', '')"
            .to_owned(),
        format!(
            "INSERT INTO dataset_state (name, rows, size_bytes, last_checked_at, last_updated_at) \
             VALUES ('imdb.ratings', 100, 60000000, {old}, {old})"
        ),
        format!(
            "INSERT INTO unmatched_file (id, path, size_bytes, mtime_ms, first_seen_at) VALUES \
             (1, '/media/orphan.mkv', 4096, {old}, {old})"
        ),
        format!(
            "INSERT INTO device (id, protocol, device_uid, name, last_seen_at) VALUES (1, \
             'chromecast', 'uid', 'Living Room', {old})"
        ),
        format!(
            "INSERT INTO cast_session (id, device_id, state, delivery, position_ms, started_at, \
             ended_at) VALUES (1, 1, 'ended', 'direct', 0, {old}, {old})"
        ),
        format!(
            "INSERT INTO settings (key, value_json, updated_at) VALUES ('ui.theme', '{{}}', {old})"
        ),
        format!(
            "INSERT INTO secret_ref (key, keyring_service, keyring_account, created_at) VALUES \
             ('tmdb.api_key', 'peregrine', 'tmdb', {old})"
        ),
    ]
}

/// The acceptance: the eligible rows past their window go, the eligible rows inside it stay,
/// and every other table in the schema is byte-identical.
#[tokio::test]
async fn retention_prunes_the_eligible_rows_and_leaves_every_other_table_byte_identical() {
    let directory = TempDir::new().unwrap();
    let database = database(&directory).await;
    let path = path_of(&directory);
    let mut statements = user_data_rows();
    statements.extend(eligible_rows());
    seed(&path, &statements).await;
    let before = snapshot(&path).await;

    let plan = database.plan_retention(NOW).await.unwrap();
    assert_eq!(
        plan.rows(),
        // Eight: one expired cache entry, the finished job, the cancelled job with no finish
        // instant, the expired and the boundary event rows, the read and the dismissed
        // notifications, and the expired release decision.
        8,
        "the plan enumerates what the run would take: {:?}",
        plan.removals()
    );
    let outcome = database.apply_retention(&plan).await.unwrap();

    assert!(
        outcome.failures().is_empty(),
        "no table should fail here: {:?}",
        outcome.failures()
    );
    assert_eq!(outcome.rows(), 8, "{:?}", outcome.removed());
    assert_untouched(&path, &before).await;
    assert_rows(&path, &eligible_expectations()).await;
}

/// Planning is not deleting: the plan is a value, and the database is untouched until it is
/// applied (AGENTS R22).
#[tokio::test]
async fn the_plan_enumerates_what_would_be_removed_without_removing_it() {
    let directory = TempDir::new().unwrap();
    let database = database(&directory).await;
    let path = path_of(&directory);
    let mut statements = user_data_rows();
    statements.extend(eligible_rows());
    seed(&path, &statements).await;
    let before = snapshot(&path).await;

    let plan = database.plan_retention(NOW).await.unwrap();

    assert_eq!(
        plan.removals().len(),
        ELIGIBLE.len(),
        "the plan enumerates every prunable table, including the ones with nothing to take"
    );
    assert_eq!(plan.at_millis(), NOW);
    for removal in plan.removals() {
        assert!(
            !removal.describes().is_empty(),
            "every entry says what it would remove"
        );
    }
    assert!(!plan.removes_nothing());
    for table in ELIGIBLE {
        let counted = plan
            .removals()
            .iter()
            .find(|removal| removal.table() == table)
            .expect("every eligible table is in the plan");
        assert!(counted.rows() > 0, "`{table}` has an old row to take");
    }
    assert_untouched(&path, &before).await;
}

/// Running the same plan twice removes nothing the second time: the policy is a predicate, not
/// a queue.
#[tokio::test]
async fn a_second_run_removes_nothing() {
    let directory = TempDir::new().unwrap();
    let database = database(&directory).await;
    let path = path_of(&directory);
    let mut statements = user_data_rows();
    statements.extend(eligible_rows());
    seed(&path, &statements).await;

    let first = database.plan_retention(NOW).await.unwrap();
    database.apply_retention(&first).await.unwrap();
    let second = database.plan_retention(NOW).await.unwrap();
    let outcome = database.apply_retention(&second).await.unwrap();

    assert!(second.removes_nothing(), "{:?}", second.removals());
    assert_eq!(outcome.rows(), 0);
}

/// §6's second bound on the event log: 10 000 rows, whichever bites first.
#[tokio::test]
async fn the_event_log_cap_prunes_the_oldest_beyond_it() {
    let directory = TempDir::new().unwrap();
    let database = database(&directory).await;
    let path = path_of(&directory);
    let values = (1..=10_007)
        .map(|id| format!("({id}, {}, 'event.recent')", days_before(1)))
        .collect::<Vec<_>>()
        .join(", ");
    seed(
        &path,
        &[format!(
            "INSERT INTO event_log (id, at, kind) VALUES {values}"
        )],
    )
    .await;

    let plan = database.plan_retention(NOW).await.unwrap();
    let event_log = plan
        .removals()
        .iter()
        .find(|removal| removal.table() == "event_log")
        .unwrap();
    assert_eq!(
        event_log.rows(),
        7,
        "the log is capped at 10 000 rows, newest kept"
    );

    database.apply_retention(&plan).await.unwrap();
    let remaining = count(&path, "SELECT COUNT(*) FROM event_log").await;
    assert_eq!(remaining, 10_000);
}

/// Best-effort: a table the run cannot prune is reported and the others still run. Dropping the
/// table is how a statement that cannot run is made real without corrupting a page.
#[tokio::test]
async fn a_table_the_run_cannot_prune_is_reported_and_the_others_still_run() {
    let directory = TempDir::new().unwrap();
    let database = database(&directory).await;
    let path = path_of(&directory);
    let mut statements = user_data_rows();
    statements.extend(eligible_rows());
    seed(&path, &statements).await;
    let mut before = snapshot(&path).await;
    before.remove("notification");

    // The plan is taken while the table is there and applied after it is gone, which is also
    // what makes the plan — not a fresh decision — the thing that runs.
    let plan = database.plan_retention(NOW).await.unwrap();
    seed(&path, &["DROP TABLE notification".to_owned()]).await;
    let outcome = database.apply_retention(&plan).await.unwrap();

    assert_eq!(outcome.failures().len(), 1, "{:?}", outcome.failures());
    assert_eq!(outcome.failures()[0].table, "notification");
    assert!(
        !outcome.failures()[0].detail.is_empty(),
        "a failure says what the database said"
    );
    assert_eq!(
        outcome.rows(),
        6,
        "the eight minus the two notification rows still go: {:?}",
        outcome.removed()
    );
    for table in ["provider_cache", "event_log", "job", "release_decision"] {
        let removal = outcome
            .removed()
            .iter()
            .find(|removal| removal.table() == table)
            .unwrap();
        assert!(removal.rows() > 0, "`{table}` still ran");
    }
    assert_untouched(&path, &before).await;
}

/// The policy and this suite's eligible list are one decision written twice: adding a table to
/// the deletion policy without adding it here (or the reverse) fails.
#[test]
fn the_policy_prunes_exactly_the_tables_this_suite_calls_eligible() {
    let mut pruned: Vec<&str> = RETENTION_POLICY
        .iter()
        .filter_map(|entry| match entry {
            RetentionPolicyEntry::Prune(rule) => Some(rule.table),
            RetentionPolicyEntry::Elsewhere { .. } => None,
        })
        .collect();
    pruned.sort_unstable();
    let mut expected = ELIGIBLE.to_vec();
    expected.sort_unstable();

    assert_eq!(
        pruned, expected,
        "the deletion policy changed; the acceptance's user-data guard must change with it"
    );
}

/// §6's retention-eligible data that this job does not prune is recorded, with the reason and
/// without a predicate: a reader sees that `image_cache` is bounded by the artwork cap, that
/// `download_job` history is the user's to clear, and that watch progress is never eligible.
#[test]
fn the_policy_records_the_eligible_tables_it_does_not_prune() {
    for (table, bound) in [
        ("image_cache", "PG-E6-09"),
        ("download_job", "PG-E13-10"),
        ("watch_progress", "forever"),
    ] {
        let entry = RETENTION_POLICY
            .iter()
            .find(|entry| match entry {
                RetentionPolicyEntry::Elsewhere { table: named, .. } => *named == table,
                RetentionPolicyEntry::Prune(_) => false,
            })
            .unwrap_or_else(|| panic!("`{table}` must be in the policy"));
        let RetentionPolicyEntry::Elsewhere {
            describes,
            bound: why,
            ..
        } = entry
        else {
            unreachable!("the entry was found by its `Elsewhere` variant")
        };
        assert!(!describes.is_empty(), "`{table}` says what a row is");
        assert!(
            why.contains(bound),
            "`{table}` must record what bounds it: {why}"
        );
    }
}

/// Every rule's columns exist, every rule selects something, the predicate runs against the
/// real schema, and the one filter the policy carries names terminal phases the domain declares.
#[tokio::test]
async fn every_rule_names_a_column_that_exists_and_selects_something() {
    let directory = TempDir::new().unwrap();
    let _database = database(&directory).await;
    let path = path_of(&directory);
    let mut connection = open_writer_connection(&path).await.unwrap();

    let mut rules = 0_usize;
    for entry in RETENTION_POLICY {
        let RetentionPolicyEntry::Prune(rule) = entry else {
            continue;
        };
        rules += 1;
        assert!(
            !rule.windows.is_empty() || rule.keep_newest.is_some(),
            "`{}` selects nothing, so it would never prune",
            rule.table
        );
        let columns = columns(&mut connection, rule.table).await;
        for window in rule.windows {
            assert!(
                columns.contains(&window.column.to_owned()),
                "`{}` has no column `{}`",
                rule.table,
                window.column
            );
        }
        for required in rule.requires {
            assert!(
                columns.contains(&required.column.to_owned()),
                "`{}` has no column `{}`",
                rule.table,
                required.column
            );
        }
        // Running the predicate is the check: a column that does not exist, or a clause that
        // is not valid SQL, fails here rather than inside a maintenance run.
        let _predicate_runs: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM \"{}\" WHERE {}",
            rule.table,
            eligibility(rule)
        )))
        .bind(NOW)
        .fetch_one(&mut connection)
        .await
        .unwrap_or_else(|error| panic!("`{}`'s predicate does not run: {error}", rule.table));
    }
    assert_eq!(rules, ELIGIBLE.len());

    let job = RETENTION_POLICY
        .iter()
        .find_map(|entry| match entry {
            RetentionPolicyEntry::Prune(rule) if rule.table == "job" => Some(rule),
            _ => None,
        })
        .expect("the job rule exists");
    for required in job.requires {
        for value in required.values {
            let phase =
                JobPhase::parse(value).unwrap_or_else(|| panic!("`{value}` is not a job phase"));
            assert!(phase.is_terminal(), "`{value}` must be a terminal phase");
        }
    }
}

/// The clause builder: the filter is `AND`ed with the aged terms, and a cap is one of them.
#[test]
fn the_eligibility_clause_ands_the_filter_with_the_aged_terms() {
    let clause = eligibility(&RetentionRule {
        table: "job",
        describes: "a terminal row",
        requires: &[Requirement {
            column: "state",
            values: &["succeeded", "failed"],
        }],
        windows: &[TimeWindow {
            column: "finished_at",
            millis: 0,
        }],
        keep_newest: Some(5),
    });

    assert_eq!(
        clause,
        "state IN ('succeeded', 'failed') AND (finished_at <= ?1 - 0 OR \
         rowid <= (SELECT MAX(rowid) FROM \"job\") - 5)"
    );
    assert_eq!(quoted(&["a", "b"]), "'a', 'b'");
    assert_eq!(PRUNE_BATCH_ROWS, 500);
}

/// A row present or absent, by the key the fixture gives it.
struct Expectation {
    table: &'static str,
    key_column: &'static str,
    key: &'static str,
    present: bool,
}

/// What the seeded eligible rows must look like after the run.
fn eligible_expectations() -> Vec<Expectation> {
    vec![
        Expectation {
            table: "provider_cache",
            key_column: "key",
            key: "cache.expired",
            present: false,
        },
        Expectation {
            table: "provider_cache",
            key_column: "key",
            key: "cache.live",
            present: true,
        },
        Expectation {
            table: "job",
            key_column: "kind",
            key: "job.expired",
            present: false,
        },
        Expectation {
            table: "job",
            key_column: "kind",
            key: "job.canceled-before-it-started",
            present: false,
        },
        Expectation {
            table: "job",
            key_column: "kind",
            key: "job.fresh",
            present: true,
        },
        Expectation {
            table: "job",
            key_column: "kind",
            key: "job.running",
            present: true,
        },
        Expectation {
            table: "event_log",
            key_column: "kind",
            key: "event.expired",
            present: false,
        },
        Expectation {
            table: "event_log",
            key_column: "kind",
            key: "event.boundary",
            present: false,
        },
        Expectation {
            table: "event_log",
            key_column: "kind",
            key: "event.recent",
            present: true,
        },
        Expectation {
            table: "notification",
            key_column: "title",
            key: "notification.read.expired",
            present: false,
        },
        Expectation {
            table: "notification",
            key_column: "title",
            key: "notification.read.fresh",
            present: true,
        },
        Expectation {
            table: "notification",
            key_column: "title",
            key: "notification.dismissed.expired",
            present: false,
        },
        Expectation {
            table: "notification",
            key_column: "title",
            key: "notification.dismissed.fresh",
            present: true,
        },
        Expectation {
            table: "notification",
            key_column: "title",
            key: "notification.unread",
            present: true,
        },
        Expectation {
            table: "release_decision",
            key_column: "release_title",
            key: "release.expired",
            present: false,
        },
        Expectation {
            table: "release_decision",
            key_column: "release_title",
            key: "release.recent",
            present: true,
        },
    ]
}

/// The single number a query returns.
async fn count(path: &Path, sql: &str) -> i64 {
    let mut connection = open_writer_connection(path).await.unwrap();
    let count = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .fetch_one(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    count
}

/// Asserts each keyed row is there or gone.
async fn assert_rows(path: &Path, expectations: &[Expectation]) {
    let mut connection = open_writer_connection(path).await.unwrap();
    for expected in expectations {
        let sql = format!(
            "SELECT COUNT(*) FROM \"{}\" WHERE \"{}\" = ?1",
            expected.table, expected.key_column
        );
        let found: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .bind(expected.key)
            .fetch_one(&mut connection)
            .await
            .unwrap();
        assert_eq!(
            found > 0,
            expected.present,
            "`{}.{}` = `{}` must be {} after the run",
            expected.table,
            expected.key_column,
            expected.key,
            expected.present
        );
    }
    connection.close().await.unwrap();
}

/// Every table outside [`ELIGIBLE`], rendered by `SQLite` itself.
async fn snapshot(path: &Path) -> BTreeMap<String, String> {
    let mut connection = open_writer_connection(path).await.unwrap();
    let mut snapshot = BTreeMap::new();
    for table in tables(&mut connection).await {
        if ELIGIBLE.contains(&table.as_str()) {
            continue;
        }
        snapshot.insert(table.clone(), dump(&mut connection, &table).await);
    }
    connection.close().await.unwrap();
    snapshot
}

/// Fails if any table outside [`ELIGIBLE`] changed at all.
async fn assert_untouched(path: &Path, before: &BTreeMap<String, String>) {
    let after = snapshot(path).await;
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>(),
        "the schema's table set changed during the run"
    );
    for (table, rows) in before {
        assert_eq!(
            rows,
            after.get(table).expect("the table set is the same"),
            "`{table}` is not retention-eligible and changed; the run must not touch it"
        );
    }
    assert!(
        before.len() > 20,
        "the guard must cover the schema, not a corner of it: {} tables",
        before.len()
    );
}

/// The tables the identity check compares.
///
/// The migration runner's bookkeeping is not user data, and the virtual search index cannot be
/// read column by column, but its shadow tables are ordinary tables and are included, so damage
/// to the index would still show.
async fn tables(connection: &mut SqliteConnection) -> Vec<String> {
    let rows = sqlx::query(
        "SELECT name, sql FROM sqlite_master WHERE type = 'table' \
         AND name NOT LIKE 'sqlite_%' AND name <> '_sqlx_migrations' ORDER BY name",
    )
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    rows.iter()
        .filter(|row| {
            !row.get::<Option<String>, _>("sql")
                .unwrap_or_default()
                .starts_with("CREATE VIRTUAL TABLE")
        })
        .map(|row| row.get::<String, _>("name"))
        .collect()
}

/// Every row of `table` as text, rendered and sorted by `SQLite`.
///
/// `quote()` gives each value its SQL literal — `NULL`, a number, a quoted string, or `X'..'`
/// for a blob — so the comparison is over the stored bytes rather than over a lossy display.
/// Sorting the rendered lines makes it a comparison of the rows themselves: the order SQLite
/// happens to walk a table in is not user data, and one of these tables is `WITHOUT ROWID`.
async fn dump(connection: &mut SqliteConnection, table: &str) -> String {
    let columns = columns(connection, table).await;
    let rendered = columns
        .iter()
        .map(|column| format!("quote(\"{column}\")"))
        .collect::<Vec<_>>()
        .join(" || '|' || ");
    let sql = format!(
        "SELECT COALESCE(group_concat(line, char(10)), '') FROM \
         (SELECT {rendered} AS line FROM \"{table}\" ORDER BY line)"
    );
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .fetch_one(&mut *connection)
        .await
        .unwrap()
}

/// The columns of `table`, in declaration order.
async fn columns(connection: &mut SqliteConnection, table: &str) -> Vec<String> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "PRAGMA table_info(\"{table}\")"
    )))
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    rows.iter()
        .map(|row| row.get::<String, _>("name"))
        .collect()
}
