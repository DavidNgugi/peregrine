//! Disk accounting tests: the database's numbers against real files on disk.
//!
//! `PG-E3-17`'s acceptance is that a diagnostics total matches a filesystem measurement within
//! 2%, and the one tier where the two sides describe the same bytes is the library: a
//! `media_file` row records the size of a file the scanner `stat`ed, so a fresh walk of the
//! same files must agree with the sum of the recorded sizes. That is what
//! `the_library_totals_match_the_filesystem_within_two_percent` asserts, on files this test
//! writes into a temporary directory rather than on numbers it also invented.
//!
//! The cache tiers cannot make that promise, and pretending they could would be the bug: an
//! `image_cache` row describes a file something else may have replaced, and a `subtitle_track`
//! row records a path and no size at all. Those tests assert what is actually true — the query
//! matches the files the database owns and is a lower bound on the directory — and
//! `the_library_total_stops_matching_when_a_file_changes_behind_the_database` proves that the
//! matching test is a comparison and not a tautology.

use std::path::{Path, PathBuf};

use peregrine_domain::{AccountId, AccountScope};
use sqlx::Connection as _;
use tempfile::TempDir;

use crate::database::open_writer_connection;
use crate::test_support::{NOW, database, path_of, seed};
use crate::usage::{MeasuredFrom, Tier};

/// The acceptance's tolerance, in percent, and the only place it is written down.
const TOLERANCE_PERCENT: u64 = 2;

/// The scope the fixtures are read with: an account with no ceiling and no curation.
fn any_account() -> AccountScope {
    AccountScope::unrestricted(AccountId::new(1).expect("account one exists"))
}

/// A scope that hides adult-flagged and unrated content.
fn restricted() -> AccountScope {
    AccountScope::restricted(AccountId::new(2).expect("account two exists"))
}

/// Writes a file of `bytes` bytes and returns the size the filesystem reports for it.
async fn write_file(path: &Path, byte: u8, bytes: usize) -> u64 {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.unwrap();
    }
    tokio::fs::write(path, vec![byte; bytes]).await.unwrap();
    tokio::fs::metadata(path).await.unwrap().len()
}

/// The number of regular files under `directory` and the sum of their sizes, as the filesystem
/// reports them. This is the "filesystem measurement" side of the acceptance.
async fn measured(directory: &Path) -> (u64, u64) {
    let mut files = 0_u64;
    let mut bytes = 0_u64;
    let mut pending = vec![directory.to_path_buf()];
    while let Some(next) = pending.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&next).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let metadata = entry.metadata().await.unwrap();
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                files += 1;
                bytes += metadata.len();
            }
        }
    }
    (files, bytes)
}

/// Asserts the two sides of the line agree inside the acceptance's tolerance.
fn within_two_percent(database_bytes: u64, filesystem_bytes: u64, what: &str) {
    assert!(
        filesystem_bytes > 0,
        "{what}: the filesystem measurement is zero, so there is nothing to compare"
    );
    let difference = database_bytes.abs_diff(filesystem_bytes);
    let allowed = filesystem_bytes * TOLERANCE_PERCENT / 100;
    assert!(
        difference <= allowed,
        "{what}: the database says {database_bytes} bytes and the filesystem says \
         {filesystem_bytes}, which differ by {difference} — more than the {TOLERANCE_PERCENT}% \
         tolerance of {allowed}"
    );
}

/// One `library_root` row.
fn root_row(id: i64, path: &Path) -> String {
    format!(
        "INSERT INTO library_root (id, path, kind, created_at) VALUES ({id}, '{}', 'mixed', \
         {NOW})",
        path.display()
    )
}

/// One `title` row, certified with `rating` (`NULL` for none) and flagged adult or not.
fn title_row(id: i64, rating: &str, adult: i64) -> String {
    format!(
        "INSERT INTO title (id, kind, name, sort_name, content_rating, adult, metadata_state, \
         created_at, updated_at) VALUES ({id}, 'movie', 'Title {id}', 'Title {id}', {rating}, \
         {adult}, 'complete', {NOW}, {NOW})"
    )
}

/// One `media_item` row for `title_id`.
fn item_row(id: i64, title_id: i64, quality: &str) -> String {
    format!(
        "INSERT INTO media_item (id, title_id, state, origin, quality_label, created_at, \
         updated_at) VALUES ({id}, {title_id}, 'available', 'scan', '{quality}', {NOW}, {NOW})"
    )
}

/// One `media_file` row of `size` bytes, under `root_id` when there is one.
fn file_row(
    id: i64,
    item_id: i64,
    root_id: Option<i64>,
    path: &str,
    size: u64,
    quality: &str,
) -> String {
    let root = root_id.map_or_else(|| "NULL".to_owned(), |root| root.to_string());
    format!(
        "INSERT INTO media_file (id, media_item_id, path, root_id, size_bytes, mtime_ms, \
         added_at, quality_label, updated_at) VALUES ({id}, {item_id}, '{path}', {root}, {size}, \
         {NOW}, {NOW}, '{quality}', {NOW})"
    )
}

/// The three rows for one scanned, certified file, where every id equals `id`.
fn scanned(root_id: i64, id: i64, path: &str, size: u64, quality: &str) -> Vec<String> {
    vec![
        title_row(id, "'PG'", 0),
        item_row(id, id, quality),
        file_row(id, id, Some(root_id), path, size, quality),
    ]
}

/// PG-E3-17's acceptance: three real files, recorded at their measured sizes, agree with a walk
/// of the directory they were written into.
#[tokio::test]
async fn the_library_totals_match_the_filesystem_within_two_percent() {
    let directory = TempDir::new().unwrap();
    let database = database(&directory).await;
    let path = path_of(&directory);
    let root = directory.path().join("media");
    let first = write_file(&root.join("film.mkv"), 0x11, 4_096).await;
    let second = write_file(&root.join("show/s01e01.mkv"), 0x22, 81_920).await;
    let third = write_file(&root.join("show/s01e02.mkv"), 0x33, 1_048_576).await;

    let mut statements = vec![root_row(1, &root)];
    statements.extend(scanned(
        1,
        1,
        &root.join("film.mkv").display().to_string(),
        first,
        "1080p",
    ));
    statements.extend(scanned(
        1,
        2,
        &root.join("show/s01e01.mkv").display().to_string(),
        second,
        "1080p",
    ));
    statements.extend(scanned(
        1,
        3,
        &root.join("show/s01e02.mkv").display().to_string(),
        third,
        "2160p",
    ));
    seed(&path, &statements).await;

    let usage = database.library_storage_usage(any_account()).await.unwrap();
    let recorded: u64 = usage.iter().map(|row| row.bytes).sum();
    let (files, bytes) = measured(&root).await;

    assert_eq!(
        usage.iter().map(|row| row.files).sum::<u64>(),
        files,
        "every measured file has a row"
    );
    assert_eq!(recorded, first + second + third);
    within_two_percent(recorded, bytes, "the library tier");
}

/// The matching test above is a comparison of two independent measurements: change the file
/// behind the database's back and the recorded total is exactly the bytes the database was
/// never told about, which is more than the tolerance. A query that returned the filesystem's
/// answer could not fail this.
#[tokio::test]
async fn the_library_total_stops_matching_when_a_file_changes_behind_the_database() {
    let directory = TempDir::new().unwrap();
    let database = database(&directory).await;
    let path = path_of(&directory);
    let root = directory.path().join("media");
    let file = root.join("film.mkv");
    let size = write_file(&file, 0x11, 65_536).await;
    let mut statements = vec![root_row(1, &root)];
    statements.extend(scanned(1, 1, &file.display().to_string(), size, "1080p"));
    seed(&path, &statements).await;

    let appended = write_file(&file, 0x22, 65_536 + 8_192).await - size;

    let usage = database.library_storage_usage(any_account()).await.unwrap();
    let recorded: u64 = usage.iter().map(|row| row.bytes).sum();
    let (_, bytes) = measured(&root).await;

    assert_eq!(recorded, size, "the query repeats what the scan recorded");
    assert_eq!(
        bytes - recorded,
        appended,
        "the gap is exactly what was written after the scan"
    );
    assert!(
        bytes - recorded > recorded * TOLERANCE_PERCENT / 100,
        "the difference must exceed the tolerance, or the accepting test proves nothing"
    );
}

#[tokio::test]
async fn the_library_usage_is_grouped_by_root_and_quality() {
    let directory = TempDir::new().unwrap();
    let database = database(&directory).await;
    let path = path_of(&directory);
    let root = directory.path().join("media");
    let mut statements = vec![root_row(1, &root)];
    statements.extend(scanned(1, 1, "/media/a.mkv", 1_000, "1080p"));
    statements.extend(scanned(1, 2, "/media/b.mkv", 2_000, "1080p"));
    statements.extend(scanned(1, 3, "/media/c.mkv", 4_000, "2160p"));
    seed(&path, &statements).await;

    let usage = database.library_storage_usage(any_account()).await.unwrap();

    assert_eq!(
        usage.len(),
        2,
        "two quality labels under one root: {usage:?}"
    );
    assert_eq!(usage[0].root_id, Some(1));
    assert_eq!(usage[0].quality_label.as_deref(), Some("1080p"));
    assert_eq!(usage[0].files, 2);
    assert_eq!(usage[0].bytes, 3_000);
    assert_eq!(usage[1].quality_label.as_deref(), Some("2160p"));
    assert_eq!(usage[1].files, 1);
    assert_eq!(usage[1].bytes, 4_000);
}

/// R34/I25: the storage screen is a library read, so a scope that may not see a title does not
/// count its bytes either.
#[tokio::test]
async fn a_restricted_scope_does_not_count_bytes_it_cannot_see() {
    let directory = TempDir::new().unwrap();
    let database = database(&directory).await;
    let path = path_of(&directory);
    let mut statements = vec![
        title_row(1, "'PG'", 0),
        title_row(2, "'X'", 1),
        title_row(3, "NULL", 0),
    ];
    for (id, size) in [(1, 1_000), (2, 2_000), (3, 4_000)] {
        statements.push(item_row(id, id, "1080p"));
        statements.push(file_row(
            id,
            id,
            None,
            &format!("/media/{id}.mkv"),
            size,
            "1080p",
        ));
    }
    seed(&path, &statements).await;

    let all = database.library_storage_usage(any_account()).await.unwrap();
    let visible = database.library_storage_usage(restricted()).await.unwrap();

    assert_eq!(all.iter().map(|row| row.bytes).sum::<u64>(), 7_000);
    assert_eq!(
        visible.iter().map(|row| row.bytes).sum::<u64>(),
        1_000,
        "an adult title and an unrated one are invisible, bytes included"
    );
    assert_eq!(visible.len(), 1);
}

/// I11: the row stays so progress survives, but the bytes are not on disk.
#[tokio::test]
async fn a_missing_file_is_not_counted() {
    let directory = TempDir::new().unwrap();
    let database = database(&directory).await;
    let path = path_of(&directory);
    let mut statements = vec![root_row(1, &directory.path().join("media"))];
    statements.extend(scanned(1, 1, "/media/gone.mkv", 4_096, "1080p"));
    statements.push(format!(
        "UPDATE media_file SET missing_since = {NOW} WHERE id = 1"
    ));
    seed(&path, &statements).await;

    let usage = database.library_storage_usage(any_account()).await.unwrap();

    assert!(
        usage.is_empty(),
        "a file recorded missing has no bytes to report: {usage:?}"
    );
}

/// The artwork tier holds a recorded size per file, so it matches the files the database owns
/// exactly, and it is a lower bound on the directory — which is the whole distinction between a
/// database count and a filesystem measurement.
#[tokio::test]
async fn the_artwork_tier_matches_the_files_the_database_owns_and_is_a_lower_bound() {
    let directory = TempDir::new().unwrap();
    let database = database(&directory).await;
    let path = path_of(&directory);
    let artwork = directory.path().join("images");
    let first = write_file(&artwork.join("poster.webp"), 0x41, 26_214).await;
    let second = write_file(&artwork.join("thumb.webp"), 0x42, 8_192).await;
    seed(
        &path,
        &[
            format!(
                "INSERT INTO image_cache (key, provider, path, variant, disk_path, bytes, \
                 created_at, last_used_at) VALUES ('a', 'tmdb', '/p/a', 'poster', '{}', {first}, \
                 {NOW}, {NOW})",
                artwork.join("poster.webp").display()
            ),
            format!(
                "INSERT INTO image_cache (key, provider, path, variant, disk_path, bytes, \
                 created_at, last_used_at) VALUES ('b', 'tmdb', '/p/b', 'thumb', '{}', {second}, \
                 {NOW}, {NOW})",
                artwork.join("thumb.webp").display()
            ),
        ],
    )
    .await;

    let owned = database.disk_usage().await.unwrap();
    let (files, bytes) = measured(&artwork).await;
    let tier = owned.tier(Tier::Artwork).unwrap();
    assert_eq!(tier.rows, files);
    assert_eq!(tier.bytes, Some(bytes));
    assert_eq!(
        tier.measured_from,
        MeasuredFrom::Recorded("image_cache.bytes")
    );

    // A file written into the cache directory that no row owns — what a crashed write or another
    // tool leaves behind. The query cannot know about it, and must not claim to.
    let orphan = write_file(&artwork.join("orphan.webp"), 0x43, 4_096).await;
    let (_, after) = measured(&artwork).await;
    let owned = database.disk_usage().await.unwrap();
    let tier = owned.tier(Tier::Artwork).unwrap();
    assert_eq!(
        tier.bytes,
        Some(bytes),
        "the database's number does not move when the directory does"
    );
    assert_eq!(after - tier.bytes.unwrap(), orphan);
}

/// The database tier is `SQLite`'s own page geometry. While the write-ahead log holds pages that
/// no checkpoint has folded into the main file, the geometry is the logical size the two files
/// together cover; once the log is checkpointed, the geometry *is* the file, inside the stated
/// tolerance.
///
/// A comparison against the two files added together is deliberately not asserted to 2%: the log
/// keeps every version of a page written since the last checkpoint, not the latest one only, so
/// the filesystem's total can exceed the logical size by more than the tolerance. That is a
/// property of the log, not a defect in the query, and the number is documented as the database's
/// logical size rather than as "the bytes on disk".
#[tokio::test]
async fn the_database_tier_matches_its_file_once_the_log_is_checkpointed() {
    let directory = TempDir::new().unwrap();
    let database = database(&directory).await;
    let path = path_of(&directory);
    // Enough to leave pages in the log; the default autocheckpoint is 1000 pages, far above it.
    seed(
        &path,
        &[format!(
            "INSERT INTO provider_cache (key, provider, resource, fetched_at, expires_at, \
             schema_version, body, size_bytes) VALUES ('k', 'tmdb', 'movie', {NOW}, {NOW}, 1, \
             zeroblob(2000000), 2000000)"
        )],
    )
    .await;

    let usage = database.disk_usage().await.unwrap();
    let tier = usage.tier(Tier::Database).unwrap();
    let main = tokio::fs::metadata(&path).await.unwrap().len();
    let wal = file_size(&wal_path(&path)).await;

    assert!(tier.rows > 0, "a migrated database has pages");
    assert_eq!(tier.measured_from, MeasuredFrom::PageGeometry);
    assert!(
        wal > 0,
        "the fixture must leave pages in the write-ahead log for this test to mean anything"
    );
    assert!(
        tier.bytes.unwrap() >= main,
        "the geometry covers the main file and the pages still only in the log"
    );
    assert!(
        tier.bytes.unwrap() <= main + wal,
        "the write-ahead log is part of the database, not a second copy of it"
    );

    checkpoint(&path).await;
    let checkpointed = tokio::fs::metadata(&path).await.unwrap().len();
    let after = database.disk_usage().await.unwrap();
    within_two_percent(
        after.tier(Tier::Database).unwrap().bytes.unwrap(),
        checkpointed,
        "the database tier after a checkpoint",
    );
}

/// The cache tier's bytes are inside the database file, so there is no filesystem side to agree
/// with: the number is the length of the bodies as the database stores them.
#[tokio::test]
async fn the_cache_tier_sums_the_bodies_the_database_stores() {
    let directory = TempDir::new().unwrap();
    let database = database(&directory).await;
    let path = path_of(&directory);
    seed(
        &path,
        &[
            format!(
                "INSERT INTO provider_cache (key, provider, resource, fetched_at, expires_at, \
                 schema_version, body, size_bytes) VALUES ('a', 'tmdb', 'movie', {NOW}, {NOW}, 1, \
                 zeroblob(1_500), 1_500)"
            ),
            format!(
                "INSERT INTO provider_cache (key, provider, resource, fetched_at, expires_at, \
                 schema_version, body, size_bytes) VALUES ('b', 'tmdb', 'tv', {NOW}, {NOW}, 1, \
                 zeroblob(500), 500)"
            ),
        ],
    )
    .await;

    let usage = database.disk_usage().await.unwrap();
    let tier = usage.tier(Tier::Cache).unwrap();

    assert_eq!(tier.rows, 2);
    assert_eq!(tier.bytes, Some(2_000));
    assert_eq!(
        tier.measured_from,
        MeasuredFrom::Recorded("provider_cache.size_bytes")
    );
}

/// The sidecar tier is the honest gap: the schema holds a path, so the database can count the
/// files and cannot say how large they are. The 2% acceptance cannot be met for this tier from
/// SQL alone, and the entry says so rather than folding a `stat` into a query.
#[tokio::test]
async fn the_sidecar_tier_counts_rows_and_cannot_report_bytes() {
    let directory = TempDir::new().unwrap();
    let database = database(&directory).await;
    let path = path_of(&directory);
    let sidecar = directory.path().join("subtitles");
    let english = write_file(&sidecar.join("film.en.srt"), 0x53, 1_024).await;
    let french = write_file(&sidecar.join("film.fr.srt"), 0x54, 2_048).await;
    let mut statements = vec![root_row(1, &directory.path().join("media"))];
    statements.extend(scanned(1, 1, "/media/film.mkv", 4_096, "1080p"));
    statements.push(format!(
        "INSERT INTO subtitle_track (media_file_id, origin, format, language, path) \
         VALUES (1, 'external', 'srt', 'en', '{}')",
        sidecar.join("film.en.srt").display()
    ));
    statements.push(format!(
        "INSERT INTO subtitle_track (media_file_id, origin, format, language, path) \
         VALUES (1, 'external', 'srt', 'fr', '{}')",
        sidecar.join("film.fr.srt").display()
    ));
    // An embedded track is inside its media file and is not a sidecar.
    statements.push(
        "INSERT INTO subtitle_track (media_file_id, origin, format, language, stream_index) \
         VALUES (1, 'embedded', 'srt', 'de', 3)"
            .to_owned(),
    );
    seed(&path, &statements).await;

    let usage = database.disk_usage().await.unwrap();
    let tier = usage.tier(Tier::Sidecars).unwrap();
    let (files, bytes) = measured(&sidecar).await;

    assert_eq!(
        tier.rows, files,
        "only the rows that record a path are sidecars"
    );
    assert_eq!(tier.bytes, None, "the schema holds no sidecar size");
    assert_eq!(
        tier.measured_from,
        MeasuredFrom::FilesystemOnly("subtitle_track.path")
    );
    assert_eq!(
        bytes,
        english + french,
        "the filesystem is where those bytes are"
    );
}

#[tokio::test]
async fn every_tier_is_reported_once_and_an_empty_database_measures_zero() {
    let directory = TempDir::new().unwrap();
    let database = database(&directory).await;

    let usage = database.disk_usage().await.unwrap();

    assert_eq!(usage.tiers().len(), Tier::ALL.len());
    for tier in Tier::ALL {
        assert_eq!(
            usage
                .tiers()
                .iter()
                .filter(|entry| entry.tier == tier)
                .count(),
            1,
            "{tier:?} is reported once"
        );
    }
    assert_eq!(usage.tier(Tier::Cache).unwrap().bytes, Some(0));
    assert_eq!(usage.tier(Tier::Artwork).unwrap().bytes, Some(0));
    assert_eq!(usage.tier(Tier::Sidecars).unwrap().rows, 0);
    assert_eq!(usage.tier(Tier::Sidecars).unwrap().bytes, None);
    assert_eq!(
        usage.tier(Tier::Logs).unwrap().measured_from,
        MeasuredFrom::NoRow
    );
    assert!(usage.tier(Tier::Database).unwrap().bytes.unwrap() > 0);
}

/// The index migration 0011 adds is the query's plan, not a decoration: these two assertions are
/// what make a future `SQLite` that stops using them visible in a test rather than in a support
/// ticket.
#[tokio::test]
async fn the_accounting_queries_are_served_by_their_covering_indices() {
    let directory = TempDir::new().unwrap();
    let database = database(&directory).await;

    let library = explain(database.readers(), crate::usage::LIBRARY_USAGE_SQL).await;
    assert!(
        library
            .iter()
            .any(|line| line.contains("media_file_usage_idx")),
        "the library usage sum must go through its covering index: {library:?}"
    );
    assert!(
        !library.iter().any(|line| line.contains("SCAN media_file")),
        "the sum must not read the file rows, which carry the probe documents: {library:?}"
    );

    let cache = explain(database.readers(), crate::usage::CACHE_SUM_SQL).await;
    assert!(
        cache
            .iter()
            .any(|line| line.contains("provider_cache_size_idx")),
        "the cache sum must go through its covering index: {cache:?}"
    );
}

/// The file `SQLite` appends `-wal` to.
fn wal_path(path: &Path) -> PathBuf {
    let name = path.file_name().expect("the database has a file name");
    let mut wal = name.to_os_string();
    wal.push("-wal");
    path.with_file_name(wal)
}

/// A file's size, or zero when it is not there.
async fn file_size(path: &Path) -> u64 {
    tokio::fs::metadata(path)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or_default()
}

/// Folds the write-ahead log into the main file, so a `stat` of the database is the database.
async fn checkpoint(path: &Path) {
    let mut connection = open_writer_connection(path).await.unwrap();
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
}

/// The query plan `SQLite` reports for `sql`.
async fn explain(pool: &sqlx::SqlitePool, sql: &str) -> Vec<String> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!("EXPLAIN QUERY PLAN {sql}")))
        .fetch_all(pool)
        .await
        .unwrap();
    rows.iter()
        .map(|row| {
            use sqlx::Row as _;
            row.get::<String, _>("detail")
        })
        .collect()
}
