//! Disk accounting: the bytes the database can account for, and the line where it stops.
//!
//! `DATA_MODEL` §7 lists the components a storage screen reports and §4 names the one
//! aggregate the schema is shaped for. This module answers both with the *database's* numbers
//! only: it never walks a directory, because the filesystem is the authority for what is on
//! disk and a query that pretended otherwise would hide the gap a support question turns on.
//!
//! The line, stated once:
//!
//! - **The database knows** the size it was told when a row was written
//!   (`provider_cache.size_bytes`, `image_cache.bytes`, `media_file.size_bytes`) and its own
//!   page geometry (`page_count * page_size`, which is the main database file).
//! - **The database cannot know** whether a file is still that size or still there, what else
//!   is in the same directory, how far the write-ahead log has grown since the last
//!   checkpoint, how large the logs are, or the bytes of a file the schema records only a path
//!   for (`subtitle_track.path`). Each of those is a `stat` or a directory walk.
//!
//! The tiers, and which side each number is measured from:
//!
//! | [`Tier`] | Database rows | [`MeasuredFrom`] |
//! | --- | --- | --- |
//! | `Database` | `page_count` | `PageGeometry`: the main file only, never the `-wal` |
//! | `Cache` | `provider_cache` | `Recorded("provider_cache.size_bytes")`: bytes stored *inside* the database file |
//! | `Artwork` | `image_cache` | `Recorded("image_cache.bytes")`: the size the writer stat'ed |
//! | `Sidecars` | `subtitle_track` with a path | `FilesystemOnly`: the schema holds no size |
//! | `Logs` | none | `NoRow`: the logging adapter owns those files |
//!
//! There is deliberately no total in [`DiskUsage`]. The `Database` figure contains the `Cache`
//! bytes, because `provider_cache` rows live inside the database file, so the tiers are not
//! disjoint and a sum would count those bytes twice. The only honest total is the filesystem's.
//!
//! ## Cost
//!
//! | Query | Plan |
//! | --- | --- |
//! | [`Database::disk_usage`] `Database` | two pragma reads of one row each; no table is read |
//! | [`Database::disk_usage`] `Cache` | a covering index scan of `provider_cache_size_idx`, so the compressed bodies are never read |
//! | [`Database::disk_usage`] `Artwork` | a full scan of `image_cache`; the table holds metadata only, so the pages are small |
//! | [`Database::disk_usage`] `Sidecars` | a full scan of `subtitle_track` for a `COUNT`; the rows are small and there is no index on `path` |
//! | [`Database::disk_usage`] `Logs` | no query |
//! | [`Database::library_storage_usage`] | a covering index scan of `media_file_usage_idx` grouped in index order, plus one primary-key lookup each into `media_item` and `title` for the scope predicate |
//!
//! The last one is the §4 "library storage usage" read: it is the one place where both sides of
//! the line describe the same bytes, because `media_file.size_bytes` was measured from the file
//! and the file is still the same one until a scan says otherwise.

use peregrine_domain::AccountScope;
use sqlx::SqlitePool;

use crate::error::StorageError;

/// One component of `DATA_MODEL` §7's disk-accounting model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// The `SQLite` database file.
    Database,
    /// `provider_cache`: cached provider responses, held as blobs inside the database.
    Cache,
    /// `image_cache`: artwork and thumbnail files on disk.
    Artwork,
    /// `subtitle_track`: subtitle sidecar files a row records a path for.
    Sidecars,
    /// The application's log files.
    Logs,
}

impl Tier {
    /// Every tier, in the order a report lists them.
    pub const ALL: [Self; 5] = [
        Self::Database,
        Self::Cache,
        Self::Artwork,
        Self::Sidecars,
        Self::Logs,
    ];
}

/// Which side of the database/filesystem line a tier's bytes were measured from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasuredFrom {
    /// `SQLite`'s own page geometry for the main database file: `page_count * page_size`.
    ///
    /// It cannot see the write-ahead log, the shared-memory file, or any pre-migration copy
    /// beside the database.
    PageGeometry,
    /// A byte column a writer recorded when it stored the row.
    ///
    /// The number is as accurate as that writer was, and nothing keeps it in step with a file
    /// changed behind the database's back.
    Recorded(&'static str),
    /// The database records a path per row and no size, so only a `stat` of that path knows.
    FilesystemOnly(&'static str),
    /// The database keeps no row for this tier at all.
    NoRow,
}

/// One tier: the rows the database holds, and the bytes it can account for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TierUsage {
    /// Which tier.
    pub tier: Tier,
    /// How many rows the database holds for the tier. Always a database count.
    pub rows: u64,
    /// The bytes the database accounts for, or `None` when it holds no size for the tier.
    pub bytes: Option<u64>,
    /// Which side of the line `bytes` came from.
    pub measured_from: MeasuredFrom,
}

/// The per-tier answer, from the database's side of the line only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskUsage {
    tiers: Vec<TierUsage>,
}

impl DiskUsage {
    /// Every tier, in [`Tier::ALL`] order.
    #[must_use]
    pub fn tiers(&self) -> &[TierUsage] {
        &self.tiers
    }

    /// One tier's entry.
    #[must_use]
    pub fn tier(&self, tier: Tier) -> Option<&TierUsage> {
        self.tiers.iter().find(|usage| usage.tier == tier)
    }
}

/// One `(root, quality)` group of the library's stored bytes (`DATA_MODEL` §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryUsageRow {
    /// The library root the files live under, or `None` when the scan attributed none.
    pub root_id: Option<i64>,
    /// The quality label the scanner recorded, or `None` when it recorded none.
    pub quality_label: Option<String>,
    /// How many files the group holds.
    pub files: u64,
    /// The sum of their recorded sizes.
    pub bytes: u64,
}

/// The cache tier's sum, as one statement so a test can explain its plan.
pub(crate) const CACHE_SUM_SQL: &str =
    "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM provider_cache";

/// The §4 library-usage sum, as one statement so a test can explain its plan.
pub(crate) const LIBRARY_USAGE_SQL: &str = "SELECT f.root_id AS root_id, f.quality_label AS quality_label, \
     COUNT(f.id) AS files, COALESCE(SUM(f.size_bytes), 0) AS bytes \
     FROM media_file f \
     JOIN media_item m ON m.id = f.media_item_id \
     JOIN title t ON t.id = m.title_id \
     WHERE f.missing_since IS NULL \
       AND (?1 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL)) \
     GROUP BY f.root_id, f.quality_label \
     ORDER BY f.root_id, f.quality_label";

/// Reads every tier's rows and bytes.
pub(crate) async fn disk_usage(pool: &SqlitePool) -> Result<DiskUsage, StorageError> {
    // One row: the two pragmas are single-row table-valued functions, and the page geometry is
    // what `SQLite` itself would write for the file's size.
    let (page_count, page_size): (i64, i64) =
        sqlx::query_as("SELECT page_count, page_size FROM pragma_page_count(), pragma_page_size()")
            .fetch_one(pool)
            .await
            .map_err(StorageError::from_sqlx)?;
    let pages = u64::try_from(page_count).unwrap_or_default();
    let bytes = pages * u64::try_from(page_size).unwrap_or_default();

    let (cache_rows, cache_bytes): (i64, i64) = sqlx::query_as(CACHE_SUM_SQL)
        .fetch_one(pool)
        .await
        .map_err(StorageError::from_sqlx)?;
    // One query, shared with the size cap's check: the artwork tier reports both numbers and the
    // cap compares one of them, so neither owns the `SELECT`.
    let artwork = crate::image_cache::totals(pool).await?;
    // An embedded track lives inside its media file and is not a sidecar; only a row that
    // records a path has bytes of its own.
    let sidecar_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM subtitle_track WHERE path IS NOT NULL")
            .fetch_one(pool)
            .await
            .map_err(StorageError::from_sqlx)?;

    Ok(DiskUsage {
        tiers: vec![
            TierUsage {
                tier: Tier::Database,
                rows: pages,
                bytes: Some(bytes),
                measured_from: MeasuredFrom::PageGeometry,
            },
            TierUsage {
                tier: Tier::Cache,
                rows: unsigned(cache_rows),
                bytes: Some(unsigned(cache_bytes)),
                measured_from: MeasuredFrom::Recorded("provider_cache.size_bytes"),
            },
            TierUsage {
                tier: Tier::Artwork,
                rows: unsigned(artwork.rows),
                bytes: Some(unsigned(artwork.bytes)),
                measured_from: MeasuredFrom::Recorded("image_cache.bytes"),
            },
            TierUsage {
                tier: Tier::Sidecars,
                rows: unsigned(sidecar_rows),
                bytes: None,
                measured_from: MeasuredFrom::FilesystemOnly("subtitle_track.path"),
            },
            TierUsage {
                tier: Tier::Logs,
                rows: 0,
                bytes: None,
                measured_from: MeasuredFrom::NoRow,
            },
        ],
    })
}

/// Sums the library's recorded bytes by root and quality label, when `scope` may see them.
///
/// A file recorded missing is left out: its bytes are not on disk, and the row is kept so watch
/// progress survives (I11). No [`AccountScope`] is asked for the tier totals above, which count
/// components rather than content; this one reads the library, so it is scoped and the predicate
/// is applied in SQL (I25).
pub(crate) async fn library_storage_usage(
    pool: &SqlitePool,
    scope: AccountScope,
) -> Result<Vec<LibraryUsageRow>, StorageError> {
    let rows = sqlx::query_as::<_, UsageRow>(LIBRARY_USAGE_SQL)
        .bind(crate::library::hidden_by_scope(scope))
        .fetch_all(pool)
        .await
        .map_err(StorageError::from_sqlx)?;

    rows.into_iter()
        .map(|row| {
            Ok(LibraryUsageRow {
                root_id: row.root_id,
                quality_label: row.quality_label,
                files: unsigned(row.files),
                bytes: unsigned(row.bytes),
            })
        })
        .collect()
}

/// The columns the library-usage statement reads.
#[derive(sqlx::FromRow)]
struct UsageRow {
    root_id: Option<i64>,
    quality_label: Option<String>,
    files: i64,
    bytes: i64,
}

/// A count or a sum as the unsigned value it holds.
fn unsigned(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

#[cfg(test)]
mod tests;
