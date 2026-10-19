//! The releases a search found that the preferred quality excluded.
//!
//! They are kept so a person can pick one without searching again. The release's *source* lives
//! here and only metadata crosses IPC: the chooser sees a title, resolution, size and seeders,
//! and the source is looked up by id at the moment the choice is made.

use sqlx::{SqliteConnection, SqlitePool};

use crate::error::StorageError;

/// One offer as the core holds it.
#[derive(Debug, Clone)]
pub struct NewOffer {
    /// The release name as the indexer stated it.
    pub release_title: String,
    /// Which indexer it came from.
    pub indexer: Option<String>,
    /// The ranker's score for it.
    pub score: f64,
    /// Where it placed in the ranking.
    pub rank_position: i64,
    /// The typed reasons it was refused, as JSON.
    pub reasons_json: String,
    /// The source, serialized. Never returned to a caller that crosses IPC.
    pub source_json: String,
    /// Its stated size.
    pub size_bytes: Option<i64>,
    /// Its stated seeders.
    pub seeders: Option<i64>,
    /// The protocol the release transfers over: `torrent` or `usenet`.
    pub protocol: String,
}

/// One stored offer, without its source.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OfferRow {
    /// The row.
    pub id: i64,
    /// The release name.
    pub release_title: String,
    /// Which indexer it came from.
    pub indexer: Option<String>,
    /// The ranker's score.
    pub score: f64,
    /// Where it placed.
    pub rank_position: i64,
    /// The typed reasons it was refused, as JSON.
    pub reasons_json: String,
    /// Its stated size.
    pub size_bytes: Option<i64>,
    /// Its stated seeders.
    pub seeders: Option<i64>,
    /// The protocol the release transfers over.
    pub protocol: String,
}

/// Replaces the offers a new search found, leaving anything the user already acted on.
///
/// A dismissed or chosen row is kept, so an offer the user rejected does not come back on the
/// next search; only the undecided rows are replaced.
pub(crate) async fn replace_offers(
    connection: &mut SqliteConnection,
    title_id: i64,
    offers: &[NewOffer],
    at: i64,
) -> Result<usize, StorageError> {
    sqlx::query(
        "DELETE FROM release_decision WHERE title_id = ?1 AND chosen = 0 AND dismissed = 0",
    )
    .bind(title_id)
    .execute(&mut *connection)
    .await
    .map_err(StorageError::from_sqlx)?;

    let mut inserted = 0usize;
    for offer in offers {
        let result = sqlx::query(
            "INSERT INTO release_decision (title_id, release_title, indexer, score, rank_position, chosen, reasons_json, created_at, source_json, size_bytes, seeders, protocol, dismissed) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?8, ?9, ?10, ?11, 0)",
        )
        .bind(title_id)
        .bind(&offer.release_title)
        .bind(&offer.indexer)
        .bind(offer.score)
        .bind(offer.rank_position)
        .bind(&offer.reasons_json)
        .bind(at)
        .bind(&offer.source_json)
        .bind(offer.size_bytes)
        .bind(offer.seeders)
        .bind(&offer.protocol)
        .execute(&mut *connection)
        .await
        .map_err(StorageError::from_sqlx)?;
        inserted += result.rows_affected() as usize;
    }
    Ok(inserted)
}

/// The offers still on the table for one title, unresolved ones only, ranked order.
pub(crate) async fn list_offers(
    pool: &SqlitePool,
    title_id: i64,
) -> Result<Vec<OfferRow>, StorageError> {
    sqlx::query_as::<_, OfferRow>(
        "SELECT id, release_title, indexer, score, rank_position, reasons_json, size_bytes, seeders, protocol FROM release_decision WHERE title_id = ?1 AND chosen = 0 AND dismissed = 0 ORDER BY rank_position ASC, id ASC",
    )
    .bind(title_id)
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)
}

/// The offer a choice named, with the source that stays server-side.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ChosenOfferRow {
    /// The title the offer belongs to.
    pub title_id: i64,
    /// The release name.
    pub release_title: String,
    /// Which indexer it came from.
    pub indexer: Option<String>,
    /// Its stated size.
    pub size_bytes: Option<i64>,
    /// Its stated seeders.
    pub seeders: Option<i64>,
    /// The source, serialized. Never returned to a caller that crosses IPC.
    pub source_json: String,
}

/// Marks one offer chosen and answers it, or `None` when it was already acted on.
pub(crate) async fn choose_offer(
    connection: &mut SqliteConnection,
    id: i64,
) -> Result<Option<ChosenOfferRow>, StorageError> {
    sqlx::query_as::<_, ChosenOfferRow>(
        "UPDATE release_decision SET chosen = 1 WHERE id = ?1 AND chosen = 0 AND dismissed = 0 RETURNING title_id, release_title, indexer, size_bytes, seeders, source_json",
    )
    .bind(id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(StorageError::from_sqlx)
}

/// Marks one offer dismissed. Answers whether a row changed.
pub(crate) async fn dismiss_offer(
    connection: &mut SqliteConnection,
    id: i64,
) -> Result<bool, StorageError> {
    let result = sqlx::query(
        "UPDATE release_decision SET dismissed = 1 WHERE id = ?1 AND chosen = 0 AND dismissed = 0",
    )
    .bind(id)
    .execute(&mut *connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(result.rows_affected() > 0)
}
