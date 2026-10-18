//! Where a title can be watched: the rows a provider's availability answer writes and a title
//! screen reads.
//!
//! **Not to be confused with `crate::availability`**, which answers whether this installation
//! holds a file. This module answers where someone else offers one, which is a different question
//! with a different provider and a different cache lifetime.
//!
//! The write is a replace rather than a merge: a provider's answer is the whole truth for the
//! title it was asked about, and a service that stopped carrying a film must disappear from the
//! row rather than linger until something else removes it.

use peregrine_domain::AccountScope;
use peregrine_domain::catalogue::{WatchAvailability, WatchCategory};

use crate::StorageError;

/// One offer as a screen draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchOfferRow {
    /// Subscription, rental, purchase, free, or ad-supported.
    pub category: WatchCategory,
    /// The provider's own id for the service.
    pub provider_id: i64,
    /// The service's name.
    pub provider_name: String,
    /// The provider-relative logo path, when the answer carried one.
    pub logo_path: Option<String>,
    /// The provider's ordering hint within its category.
    pub display_priority: Option<u32>,
}

/// The region-level part of an answer: the provider's own page and which adapter said it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchRegionRow {
    /// The provider's page for every option, when it sent one.
    pub link: Option<String>,
    /// The metadata provider that answered, which the logo cache key needs.
    pub source: String,
}

/// Replaces every region and offer for one title, and records that it was just fetched.
///
/// The delete and the inserts run on the writer's single connection, inside the transaction the
/// writer already holds for one request, so a screen never reads half of an answer.
pub(crate) async fn replace(
    connection: &mut sqlx::SqliteConnection,
    title_id: i64,
    source: &str,
    regions: &[WatchAvailability],
    at: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM watch_availability WHERE title_id = ?1")
        .bind(title_id)
        .execute(&mut *connection)
        .await?;

    for region in regions {
        sqlx::query(
            "INSERT INTO watch_availability (title_id, region, source, link) \
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(title_id)
        .bind(region.region.as_str())
        .bind(source)
        .bind(region.link.as_deref())
        .execute(&mut *connection)
        .await?;

        for offer in &region.offers {
            sqlx::query(
                "INSERT INTO watch_offer \
                 (title_id, region, category, provider_id, provider_name, logo_path, display_priority) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .bind(title_id)
            .bind(region.region.as_str())
            .bind(offer.category.as_str())
            .bind(offer.provider_id)
            .bind(offer.provider_name.as_str())
            .bind(offer.logo_path.as_deref())
            .bind(offer.display_priority.map(i64::from))
            .execute(&mut *connection)
            .await?;
        }
    }

    mark_synced(connection, title_id, at).await
}

/// Records that a provider answered for this title, even when the answer held no regions.
pub(crate) async fn mark_synced(
    connection: &mut sqlx::SqliteConnection,
    title_id: i64,
    at: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO watch_sync (title_id, synced_at) VALUES (?1, ?2) \
         ON CONFLICT (title_id) DO UPDATE SET synced_at = excluded.synced_at",
    )
    .bind(title_id)
    .bind(at)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// When this title's availability was last fetched, or `None` when it never was.
pub(crate) async fn synced_at(
    pool: &sqlx::SqlitePool,
    title_id: i64,
) -> Result<Option<i64>, StorageError> {
    sqlx::query_scalar::<_, i64>("SELECT synced_at FROM watch_sync WHERE title_id = ?1")
        .bind(title_id)
        .fetch_optional(pool)
        .await
        .map_err(StorageError::from_sqlx)
}

/// One region's answer, when the scope may see the title and the region was fetched.
pub(crate) async fn region(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    title_id: i64,
    region: &str,
) -> Result<Option<WatchRegionRow>, StorageError> {
    let row = sqlx::query_as::<_, (Option<String>, String)>(
        "SELECT a.link, a.source FROM watch_availability a \
         JOIN title t ON t.id = a.title_id \
         WHERE a.title_id = ?1 AND a.region = ?2 \
           AND (?3 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL))",
    )
    .bind(title_id)
    .bind(region)
    .bind(crate::library::hidden_by_scope(scope))
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(row.map(|(link, source)| WatchRegionRow { link, source }))
}

/// One region's offers, in provider priority order.
pub(crate) async fn offers(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    title_id: i64,
    region: &str,
) -> Result<Vec<WatchOfferRow>, StorageError> {
    #[derive(sqlx::FromRow)]
    struct Row {
        category: String,
        provider_id: i64,
        provider_name: String,
        logo_path: Option<String>,
        display_priority: Option<i64>,
    }

    let rows = sqlx::query_as::<_, Row>(
        "SELECT o.category, o.provider_id, o.provider_name, o.logo_path, o.display_priority \
         FROM watch_offer o \
         JOIN title t ON t.id = o.title_id \
         WHERE o.title_id = ?1 AND o.region = ?2 \
           AND (?3 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL)) \
         ORDER BY o.display_priority IS NULL, o.display_priority, o.provider_name",
    )
    .bind(title_id)
    .bind(region)
    .bind(crate::library::hidden_by_scope(scope))
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;

    rows.into_iter()
        .map(|row| {
            let category = WatchCategory::parse(&row.category).ok_or_else(|| {
                StorageError::UnexpectedValue {
                    column: "watch_offer.category",
                    found: row.category.clone(),
                }
            })?;
            Ok(WatchOfferRow {
                category,
                provider_id: row.provider_id,
                provider_name: row.provider_name,
                logo_path: row.logo_path,
                display_priority: row
                    .display_priority
                    .and_then(|value| u32::try_from(value).ok()),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{database, path_of, title};
    use peregrine_domain::catalogue::{Region, WatchAvailability, WatchOffer};
    use peregrine_domain::{AccountId, AccountScope, NonEmptyText};
    use tempfile::TempDir;

    fn offer(
        category: WatchCategory,
        provider_id: i64,
        name: &str,
        logo: Option<&str>,
        priority: Option<u32>,
    ) -> WatchOffer {
        WatchOffer {
            category,
            provider_id,
            provider_name: NonEmptyText::new(name).expect("a name"),
            logo_path: logo.map(str::to_owned),
            display_priority: priority,
        }
    }

    fn availability(region: &str, link: &str, offers: Vec<WatchOffer>) -> WatchAvailability {
        WatchAvailability {
            region: Region::new(region).expect("a region"),
            link: Some(link.to_owned()),
            offers,
        }
    }

    fn scope() -> AccountScope {
        AccountScope::unrestricted(AccountId::new(1).expect("account one"))
    }

    #[tokio::test]
    async fn a_replaced_answer_reads_back_by_region_and_marks_the_title_fetched() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        let path = path_of(&directory);
        title(&path, 1, "The Matrix", 0, Some("R")).await;

        database
            .replace_watch_availability(
                1,
                "tmdb".to_owned(),
                vec![availability(
                    "US",
                    "https://www.themoviedb.org/movie/1/watch?locale=US",
                    vec![
                        offer(
                            WatchCategory::Flatrate,
                            8,
                            "Netflix",
                            Some("/n.jpg"),
                            Some(0),
                        ),
                        offer(WatchCategory::Rent, 2, "Apple TV", Some("/a.jpg"), Some(1)),
                    ],
                )],
                1_700_000_000_000,
            )
            .await
            .expect("the answer is written");

        assert_eq!(
            database.watch_synced_at(1).await.expect("the marker"),
            Some(1_700_000_000_000)
        );
        let region = database
            .watch_region(scope(), 1, "US")
            .await
            .expect("the region")
            .expect("it was written");
        assert_eq!(region.source, "tmdb");
        assert_eq!(
            region.link.as_deref(),
            Some("https://www.themoviedb.org/movie/1/watch?locale=US")
        );
        let offers = database
            .watch_offers(scope(), 1, "US")
            .await
            .expect("the offers");
        assert_eq!(offers.len(), 2);
        assert_eq!(offers[0].category, WatchCategory::Flatrate);
        assert_eq!(offers[0].logo_path.as_deref(), Some("/n.jpg"));
        assert_eq!(offers[0].display_priority, Some(0));
    }

    #[tokio::test]
    async fn a_replacement_drops_a_region_that_disappeared() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        let path = path_of(&directory);
        title(&path, 1, "The Matrix", 0, Some("R")).await;

        database
            .replace_watch_availability(
                1,
                "tmdb".to_owned(),
                vec![
                    availability("US", "https://example.test/us", Vec::new()),
                    availability("DE", "https://example.test/de", Vec::new()),
                ],
                1,
            )
            .await
            .expect("the first answer");
        database
            .replace_watch_availability(
                1,
                "tmdb".to_owned(),
                vec![availability("US", "https://example.test/us", Vec::new())],
                2,
            )
            .await
            .expect("the second answer");

        assert!(
            database
                .watch_region(scope(), 1, "DE")
                .await
                .expect("the read")
                .is_none(),
            "a service that stopped carrying the title leaves no row"
        );
        assert_eq!(
            database.watch_synced_at(1).await.expect("the marker"),
            Some(2)
        );
    }

    #[tokio::test]
    async fn an_empty_answer_still_marks_the_title_fetched() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        let path = path_of(&directory);
        title(&path, 1, "The Matrix", 0, Some("R")).await;

        database
            .mark_watch_availability_synced(1, 99)
            .await
            .expect("the marker is written");
        assert_eq!(
            database.watch_synced_at(1).await.expect("the marker"),
            Some(99)
        );
        assert!(
            database
                .watch_region(scope(), 1, "US")
                .await
                .expect("the read")
                .is_none()
        );
    }
}
