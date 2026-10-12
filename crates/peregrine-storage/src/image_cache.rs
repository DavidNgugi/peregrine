//! The `image_cache` rows: what the artwork tier holds, and what has been read.
//!
//! The bytes themselves are `peregrine-sync`'s (`PG-E6-09`): a content-addressed blob store whose
//! key is `sha256(provider|path|variant)`, written atomically so a crash cannot leave a partial
//! poster behind. This module is the *accounting* beside those blobs — one row per stored image,
//! saying where it is, how big it is, and when it was last read — which is what an eviction needs
//! to decide what to throw away (`PG-E6-10`) and what the storage screen reports as the artwork
//! tier (`DATA_MODEL` §7).
//!
//! The two halves are written by different layers and neither can be reached from the other, so the
//! honest statement of their relationship is this: **a row without a file is a row an eviction has
//! to tolerate** (the file was swept by hand or lost with a volume), and **a file without a row is a
//! leak** that only the cache wipe can reap (`PG-E6-18`), because eviction walks rows and a
//! directory walk is the only thing that finds what the rows do not know about. That is why the
//! row is written *after* the bytes are published: a crash between the two loses a file rather than
//! recording bytes that are not there.
//!
//! ## Cost
//!
//! | Query | Plan |
//! | --- | --- |
//! | [`Database::image_cache_entry`] | one row by primary key |
//! | [`Database::image_cache_totals`] | a full scan of `image_cache` for one `COUNT` and one `SUM`; the table holds metadata only — no blobs — so its pages are small, and there is no index on `bytes` because a sum is not a range read |
//! | [`Database::record_image`] | one upsert by primary key |
//! | [`Database::touch_image`] | one update by primary key |
//! | [`Database::remove_image`] | one delete by primary key |
//!
//! The least-recently-used *ordering* an eviction needs is deliberately not here. `PG-E6-10` owns
//! that query and therefore the index that serves it (AGENTS §5): adding one now would be an index
//! for a read nobody has written.

use crate::error::StorageError;

/// One row of `image_cache`: a stored image, and what is known about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageCacheEntry {
    /// `sha256(provider|path|variant)`, lowercase hex — the same key `PG-E6-09` names the blob with.
    pub key: String,
    /// The provider the image came from.
    pub provider: String,
    /// The provider's own path for it.
    pub path: String,
    /// The size variant, as the provider spells it (`w342`, `w780`).
    pub variant: String,
    /// Where the bytes are, which the asset protocol is scoped to serve.
    pub disk_path: String,
    /// How many bytes the blob holds.
    pub bytes: i64,
    /// The image's width, when the provider or the probe reported one.
    pub width: Option<i64>,
    /// The image's height, when the provider or the probe reported one.
    pub height: Option<i64>,
    /// When the row was first written, in Unix milliseconds.
    pub created_at: i64,
    /// When the image was last read, in Unix milliseconds. What an eviction orders by.
    pub last_used_at: i64,
    /// How many times it has been read. Reported rather than acted on: a second eviction policy
    /// could use it to keep a much-read image, which is a decision for `PG-E6-10` to make.
    pub hits: i64,
}

/// Inserts one row, or refreshes the columns a re-discovery can change.
///
/// The upsert updates the file's location, size, and dimensions and the instant it was last used,
/// and deliberately keeps `created_at` and `hits`: a row that already exists is the same image
/// rediscovered — after a row loss or a restored backup — not a new one, and not a read. A read is
/// [`touch`], which is the caller's to report because only the caller knows whether the file it
/// found was the one it was looking for.
pub(crate) async fn record(
    connection: &mut sqlx::SqliteConnection,
    entry: &ImageCacheEntry,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO image_cache (key, provider, path, variant, disk_path, bytes, width, height, \
         created_at, last_used_at, hits) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
         ON CONFLICT (key) DO UPDATE SET \
             disk_path = excluded.disk_path, bytes = excluded.bytes, \
             width = excluded.width, height = excluded.height, \
             last_used_at = excluded.last_used_at",
    )
    .bind(&entry.key)
    .bind(&entry.provider)
    .bind(&entry.path)
    .bind(&entry.variant)
    .bind(&entry.disk_path)
    .bind(entry.bytes)
    .bind(entry.width)
    .bind(entry.height)
    .bind(entry.created_at)
    .bind(entry.last_used_at)
    .bind(entry.hits)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// Reads one row by key.
pub(crate) async fn entry(
    pool: &sqlx::SqlitePool,
    key: &str,
) -> Result<Option<ImageCacheEntry>, StorageError> {
    let row = sqlx::query_as::<_, EntryRow>(
        "SELECT key, provider, path, variant, disk_path, bytes, width, height, created_at, \
         last_used_at, hits FROM image_cache WHERE key = ?1",
    )
    .bind(key)
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(row.map(Into::into))
}

/// How many images are cached, and how many bytes they account for.
///
/// One query, because two callers want two different halves of it: the storage screen reports both
/// as the artwork tier (`DATA_MODEL` §7) and a size cap compares the bytes (`PG-E6-10`). Two
/// functions would be two copies of one `SELECT`, and the copy that drifted would be the one
/// nobody tested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Totals {
    /// How many rows the cache holds.
    pub rows: i64,
    /// What their recorded sizes add up to. Zero for an empty cache, never `NULL`.
    pub bytes: i64,
}

/// Reads the totals.
pub(crate) async fn totals(pool: &sqlx::SqlitePool) -> Result<Totals, StorageError> {
    let row: (i64, i64) =
        sqlx::query_as("SELECT COUNT(*), COALESCE(SUM(bytes), 0) FROM image_cache")
            .fetch_one(pool)
            .await
            .map_err(StorageError::from_sqlx)?;
    Ok(Totals {
        rows: row.0,
        bytes: row.1,
    })
}

/// Reports that the image was read at `at`, answering whether the row was there.
pub(crate) async fn touch(
    connection: &mut sqlx::SqliteConnection,
    key: &str,
    at: i64,
) -> Result<bool, sqlx::Error> {
    let result =
        sqlx::query("UPDATE image_cache SET last_used_at = ?1, hits = hits + 1 WHERE key = ?2")
            .bind(at)
            .bind(key)
            .execute(&mut *connection)
            .await?;
    Ok(result.rows_affected() > 0)
}

/// Removes one row, answering whether it was there.
///
/// The blob is not touched, because this layer does not know where it is beyond the string in the
/// row it is deleting. `PG-E6-10`'s eviction removes the file first and the row second, for the
/// same reason the record is written after the publish: a row that outlives its file is a row an
/// eviction tolerates, and a file that outlives its row is a leak.
pub(crate) async fn remove(
    connection: &mut sqlx::SqliteConnection,
    key: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM image_cache WHERE key = ?1")
        .bind(key)
        .execute(&mut *connection)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// The columns [`ImageCacheEntry`] needs.
#[derive(sqlx::FromRow)]
struct EntryRow {
    key: String,
    provider: String,
    path: String,
    variant: String,
    disk_path: String,
    bytes: i64,
    width: Option<i64>,
    height: Option<i64>,
    created_at: i64,
    last_used_at: i64,
    hits: i64,
}

impl From<EntryRow> for ImageCacheEntry {
    fn from(row: EntryRow) -> Self {
        Self {
            key: row.key,
            provider: row.provider,
            path: row.path,
            variant: row.variant,
            disk_path: row.disk_path,
            bytes: row.bytes,
            width: row.width,
            height: row.height,
            created_at: row.created_at,
            last_used_at: row.last_used_at,
            hits: row.hits,
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use crate::Database;

    use super::*;

    const NOW: i64 = 1_700_000_000_000;

    /// A row as the fetch path will write it.
    fn entry(key: &str, bytes: i64) -> ImageCacheEntry {
        ImageCacheEntry {
            key: key.to_owned(),
            provider: "tmdb".to_owned(),
            path: format!("/{key}.jpg"),
            variant: "w342".to_owned(),
            disk_path: format!("/cache/{key}"),
            bytes,
            width: Some(342),
            height: Some(513),
            created_at: NOW,
            last_used_at: NOW,
            hits: 0,
        }
    }

    async fn database(directory: &TempDir) -> Database {
        Database::open(directory.path().join("peregrine.sqlite"))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_recorded_image_round_trips() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;

        database.record_image(entry("a", 63_663)).await.unwrap();

        let stored = database
            .image_cache_entry("a")
            .await
            .unwrap()
            .expect("the row was just written");
        assert_eq!(stored, entry("a", 63_663));
    }

    #[tokio::test]
    async fn an_unknown_key_is_absent_rather_than_an_error() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;

        assert_eq!(database.image_cache_entry("nobody").await.unwrap(), None);
        assert!(!database.touch_image("nobody", NOW).await.unwrap());
        assert!(!database.remove_image("nobody").await.unwrap());
    }

    #[tokio::test]
    async fn recording_the_same_key_twice_keeps_when_it_was_first_seen_and_how_often_it_was_read() {
        // A re-record is the same image rediscovered — after a row loss or a restored backup — not
        // a new file and not a read. `hits` is what a policy keeps a popular image with, and
        // resetting it because a scan found the file again would throw that away.
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        database.record_image(entry("a", 63_663)).await.unwrap();
        database.touch_image("a", NOW + 1_000).await.unwrap();

        let mut again = entry("a", 64_000);
        again.created_at = NOW + 5_000;
        again.last_used_at = NOW + 5_000;
        again.disk_path = "/cache/moved".to_owned();
        database.record_image(again).await.unwrap();

        let stored = database.image_cache_entry("a").await.unwrap().unwrap();
        assert_eq!(stored.bytes, 64_000, "the size is refreshed");
        assert_eq!(stored.disk_path, "/cache/moved", "and so is where it is");
        assert_eq!(stored.created_at, NOW, "when it was first seen is not");
        assert_eq!(stored.hits, 1, "and a re-record is not a read");
        assert_eq!(stored.last_used_at, NOW + 5_000);
    }

    #[tokio::test]
    async fn touching_an_image_counts_the_read_and_moves_the_instant() {
        // What an eviction orders by: an image read a moment ago must not look like one read last
        // week, or the policy throws away what the user is looking at.
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        database.record_image(entry("a", 63_663)).await.unwrap();

        assert!(database.touch_image("a", NOW + 60_000).await.unwrap());
        assert!(database.touch_image("a", NOW + 120_000).await.unwrap());

        let stored = database.image_cache_entry("a").await.unwrap().unwrap();
        assert_eq!(stored.hits, 2);
        assert_eq!(stored.last_used_at, NOW + 120_000);
    }

    #[tokio::test]
    async fn the_byte_total_is_zero_for_an_empty_cache_and_sums_every_row() {
        // The number `PG-E6-10` compares against the user's cap. Zero rather than `NULL`: a cache
        // with nothing in it has used no bytes, and a caller that had to handle `NULL` would treat
        // "empty" as a case rather than as the number it is.
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        assert_eq!(
            database.image_cache_totals().await.unwrap(),
            Totals::default()
        );

        database.record_image(entry("a", 63_663)).await.unwrap();
        database.record_image(entry("b", 291_246)).await.unwrap();
        assert_eq!(
            database.image_cache_totals().await.unwrap(),
            Totals {
                rows: 2,
                bytes: 354_909,
            }
        );

        database.remove_image("a").await.unwrap();
        assert_eq!(
            database.image_cache_totals().await.unwrap(),
            Totals {
                rows: 1,
                bytes: 291_246,
            }
        );
    }

    #[tokio::test]
    async fn removing_an_image_reports_whether_it_was_there() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        database.record_image(entry("a", 63_663)).await.unwrap();

        assert!(database.remove_image("a").await.unwrap());
        assert!(!database.remove_image("a").await.unwrap());
        assert_eq!(database.image_cache_entry("a").await.unwrap(), None);
    }
}
