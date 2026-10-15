//! The Discover rails' membership, persisted (`PG-E7-08`).
//!
//! [`crate::Database`] holds the rails in memory so a screen can read them without touching
//! the network or failing, which is what `FLOWS F2` asks of a first paint. What memory cannot
//! do is survive a launch, so until this module existed every start drew seven empty rails and
//! fetched the same seven lists again. This is the manifest that fixes that: which titles a
//! rail held, and in what order.
//!
//! # Membership, not the item
//!
//! A rail item draws a name, a year, a score, a synopsis, a poster, a backdrop, and whether
//! the library holds a file — and every one of those is already somewhere else. The first five
//! are columns on the `title` row a list answer wrote, the artwork's location is a `stat` on
//! the cache, and the library's answer is an aggregate over `media_item` and `media_file`. So
//! the table holds the one thing none of them can express — the rail's own order — and the
//! read below gathers the rest in the same statement, because a screen that hydrated a hundred
//! titles one query at a time would spend the first paint it exists to protect.
//!
//! # No retention entry, and why
//!
//! `RETENTION_POLICY` records what bounds the data `DATA_MODEL` §6 names. This table is not in
//! it: a write replaces the whole manifest, so it holds one run's answer and never accumulates,
//! and its rows go with their title (`ON DELETE CASCADE`). Its size *is* the manifest's.

use peregrine_domain::AccountScope;
use sqlx::Connection as _;
use sqlx::{SqliteConnection, SqlitePool};

use crate::catalogue::{TitleKind, hidden_by_scope};
use crate::error::StorageError;
use crate::library::{ArtworkPath, artwork_path};

/// One title in one rail, with everything a rail draws.
///
/// The counts are carried rather than a `playable` flag: what counts as playable is the
/// library's rule and belongs where every other caller states it, not baked into a row a
/// repository returns.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoverRailRow {
    /// The rail's stable identifier, a `RailKind` spelling.
    pub rail: String,
    /// The catalogue row, which is what opening the item needs.
    pub title_id: i64,
    /// Film or series.
    pub kind: TitleKind,
    /// The name to draw.
    pub name: String,
    /// The date the provider listed it under, when the row holds a readable one.
    pub released: Option<jiff::civil::Date>,
    /// The score the list answer carried.
    pub rating: Option<f64>,
    /// How many people voted on it.
    pub votes: Option<u64>,
    /// The synopsis the list answer carried.
    pub overview: Option<String>,
    /// The provider-relative poster the title row records, when it records a usable pair.
    pub poster: Option<ArtworkPath>,
    /// The provider-relative backdrop, for the same reason.
    pub backdrop: Option<ArtworkPath>,
    /// How many files the library holds for the title.
    pub files: i64,
    /// How many of them the last scan could not find.
    pub missing_files: i64,
}

/// Replaces the whole manifest with `rails`, in one transaction.
///
/// Whole rather than per-rail because that is what a publish *is*: a run answers a set of
/// rails, and a manifest that kept a rail this run did not answer would show a screen
/// membership the last publish does not have.
///
/// Each rail's `Vec` is the order that rail lists its titles in, and positions are assigned
/// from it rather than accepted from the caller, so two titles sharing a place is not
/// expressible.
///
/// # Errors
///
/// [`StorageError`] when a statement fails. A title the catalogue does not hold fails the
/// whole write rather than being skipped: the reference is the database's, and a rail silently
/// missing the titles that did not resolve would be worse than a write that did not happen.
pub(crate) async fn replace(
    connection: &mut SqliteConnection,
    rails: &[(String, Vec<i64>)],
) -> Result<(), StorageError> {
    let mut transaction = connection.begin().await.map_err(StorageError::from_sqlx)?;
    sqlx::query("DELETE FROM discover_rail")
        .execute(&mut *transaction)
        .await
        .map_err(StorageError::from_sqlx)?;
    for (rail, titles) in rails {
        for (position, title_id) in titles.iter().enumerate() {
            // A rail is a page of a provider's list, so the index is nowhere near `i64`'s bound;
            // the checked conversion is the shape this crate uses for a `usize` column value and
            // the fallback branch is unreachable.
            let position = i64::try_from(position).unwrap_or(i64::MAX);
            sqlx::query("INSERT INTO discover_rail (rail, position, title_id) VALUES (?1, ?2, ?3)")
                .bind(rail)
                .bind(position)
                .bind(title_id)
                .execute(&mut *transaction)
                .await
                .map_err(StorageError::from_sqlx)?;
        }
    }
    transaction
        .commit()
        .await
        .map_err(StorageError::from_sqlx)?;
    Ok(())
}

/// Every title the manifest holds, in rail order and then in the rail's own order.
///
/// **The order is the read's, not the screen's.** Rails come back grouped by their identifier
/// in the order SQLite sorts strings, which is not the order a home screen shows them: that
/// order is the `RailKind` vocabulary's, and the caller reads it there (`RailKind::ALL`) while
/// grouping these rows by `rail`.
pub(crate) async fn read(
    pool: &SqlitePool,
    scope: AccountScope,
) -> Result<Vec<DiscoverRailRow>, StorageError> {
    let rows = sqlx::query_as::<_, DiscoverRailQueryRow>(
        // `LEFT JOIN media_item` and not an inner one, because a title a provider listed need
        // not be one the library holds: the rail draws it either way, and the counts answer
        // zero. `position` is selected for the grouping and the ordering it is doing rather
        // than for the caller, which is why it is not a field of the row.
        "SELECT d.rail, d.position, t.id AS title_id, t.kind, t.name, \
                t.release_date, t.first_air_date, t.tmdb_rating, t.tmdb_votes, t.overview, \
                t.poster_provider, t.poster_path, t.backdrop_provider, t.backdrop_path, \
                COUNT(f.id) AS files, \
                COALESCE(SUM(f.missing_since IS NOT NULL), 0) AS missing_files \
           FROM discover_rail d \
           JOIN title t ON t.id = d.title_id \
           LEFT JOIN media_item m ON m.title_id = t.id \
           LEFT JOIN media_file f ON f.media_item_id = m.id \
          WHERE (?1 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL)) \
          GROUP BY d.rail, d.position \
          ORDER BY d.rail, d.position",
    )
    .bind(hidden_by_scope(scope))
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    rows.into_iter().map(TryInto::try_into).collect()
}

/// The columns [`DiscoverRailRow`] is built from.
#[derive(sqlx::FromRow)]
struct DiscoverRailQueryRow {
    rail: String,
    title_id: i64,
    kind: String,
    name: String,
    release_date: Option<String>,
    first_air_date: Option<String>,
    tmdb_rating: Option<f64>,
    tmdb_votes: Option<i64>,
    overview: Option<String>,
    poster_provider: Option<String>,
    poster_path: Option<String>,
    backdrop_provider: Option<String>,
    backdrop_path: Option<String>,
    files: i64,
    missing_files: i64,
}

impl TryFrom<DiscoverRailQueryRow> for DiscoverRailRow {
    type Error = StorageError;

    fn try_from(row: DiscoverRailQueryRow) -> Result<Self, Self::Error> {
        let kind = TitleKind::parse(&row.kind).ok_or_else(|| StorageError::UnexpectedValue {
            column: "title.kind",
            found: row.kind.clone(),
        })?;
        // The sync writes a film's date to `release_date` and a series' to `first_air_date`,
        // so exactly one of the two is ever set by this application; a rail is drawn from
        // whichever the row has.
        let released = listed_date(row.release_date.or(row.first_air_date));
        Ok(Self {
            rail: row.rail,
            title_id: row.title_id,
            kind,
            name: row.name,
            released,
            rating: row.tmdb_rating,
            votes: row.tmdb_votes.and_then(|votes| u64::try_from(votes).ok()),
            overview: row.overview,
            poster: artwork_path(
                "title.poster_provider",
                row.poster_provider,
                row.poster_path,
            ),
            backdrop: artwork_path(
                "title.backdrop_provider",
                row.backdrop_provider,
                row.backdrop_path,
            ),
            files: row.files,
            missing_files: row.missing_files,
        })
    }
}

/// The date a rail's year is drawn from, when the column holds one this build can read.
///
/// A row whose date cannot be parsed is a rail item without a year rather than an unreadable
/// title: the date is decoration, and the write path only ever stores `jiff`'s own rendering
/// of a date, so a value this refuses came from outside this application.
fn listed_date(text: Option<String>) -> Option<jiff::civil::Date> {
    let text = text?;
    match text.parse::<jiff::civil::Date>() {
        Ok(date) => Some(date),
        Err(error) => {
            // A date column is not a path and not a secret, so the value is safe to record
            // (SECURITY.md T13).
            tracing::warn!(
                column = "title.release_date",
                value = %text,
                %error,
                "a title records a date this build cannot read"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use peregrine_domain::{AccountId, AccountScope, Provider};
    use tempfile::TempDir;

    use super::*;
    use crate::Database;
    use crate::test_support::{NOW, account, database, execute, path_of, seed, title};

    fn any_account() -> AccountScope {
        AccountScope::unrestricted(AccountId::new(1).expect("account one exists"))
    }

    fn restricted() -> AccountScope {
        AccountScope::restricted(AccountId::new(2).expect("account two exists"))
    }

    /// The catalogue a rail draws from: two certified titles the library partly holds, and one
    /// adult title a restricted scope must not see.
    async fn catalogue(directory: &TempDir) -> Database {
        let path = path_of(directory);
        let database = database(directory).await;
        account(&path, 1, "Adult").await;
        account(&path, 2, "Child").await;
        title(&path, 1, "Alien", 0, Some("R")).await;
        title(&path, 2, "Arrival", 0, Some("PG-13")).await;
        title(&path, 3, "An Adult Film", 1, Some("R")).await;
        seed(
            &path,
            &[
                "UPDATE title SET release_date = '1999-03-31', tmdb_rating = 8.2, \
                 tmdb_votes = 3000, overview = 'In space, no one can hear you scream.', \
                 poster_provider = 'tmdb', poster_path = '/alien.jpg', \
                 backdrop_provider = 'tmdb', backdrop_path = '/alien-wide.jpg' \
                 WHERE id = 1"
                    .to_owned(),
                format!(
                    "INSERT INTO media_item (id, title_id, state, origin, created_at, updated_at) \
                     VALUES (1, 1, 'available', 'scan', {NOW}, {NOW})"
                ),
                format!(
                    "INSERT INTO media_file (id, media_item_id, path, size_bytes, mtime_ms, \
                     added_at, updated_at) VALUES (1, 1, '/m/alien.mkv', 10, {NOW}, {NOW}, {NOW})"
                ),
                format!(
                    "INSERT INTO media_file (id, media_item_id, path, size_bytes, mtime_ms, \
                     added_at, updated_at, missing_since) \
                     VALUES (2, 1, '/m/alien-gone.mkv', 10, {NOW}, {NOW}, {NOW}, {NOW})"
                ),
                // A series, so the `first_air_date` half of the date rule is exercised too.
                "UPDATE title SET kind = 'show' WHERE id = 2".to_owned(),
                "UPDATE title SET first_air_date = '2016-11-11' WHERE id = 2".to_owned(),
            ],
        )
        .await;
        database
    }

    #[tokio::test]
    async fn an_empty_manifest_reads_as_no_rails() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        assert!(
            database
                .discover_rails(any_account())
                .await
                .unwrap()
                .is_empty(),
            "a database nothing has published to has no rails to hydrate"
        );
    }

    #[tokio::test]
    async fn the_manifest_round_trips_with_everything_a_rail_draws() {
        let directory = TempDir::new().unwrap();
        let database = catalogue(&directory).await;

        database
            .replace_discover_rails(&[
                ("trending_week".to_owned(), vec![2, 1]),
                ("popular_movies".to_owned(), vec![1]),
            ])
            .await
            .unwrap();

        let rows = database.discover_rails(any_account()).await.unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| (row.rail.as_str(), row.title_id))
                .collect::<Vec<_>>(),
            vec![
                ("popular_movies", 1),
                ("trending_week", 2),
                ("trending_week", 1)
            ],
            "the rail's own order is what the position column carries"
        );
        let alien = rows
            .iter()
            .find(|row| row.rail == "popular_movies")
            .expect("the film's rail");
        assert_eq!(alien.kind, TitleKind::Movie);
        assert_eq!(alien.name, "Alien");
        assert_eq!(alien.released, Some("1999-03-31".parse().unwrap()));
        assert_eq!(alien.rating, Some(8.2));
        assert_eq!(alien.votes, Some(3000));
        assert_eq!(
            alien.overview.as_deref(),
            Some("In space, no one can hear you scream.")
        );
        assert_eq!(
            alien.poster,
            Some(ArtworkPath {
                provider: Provider::Tmdb,
                path: "/alien.jpg".to_owned(),
            })
        );
        assert_eq!(
            alien.backdrop,
            Some(ArtworkPath {
                provider: Provider::Tmdb,
                path: "/alien-wide.jpg".to_owned(),
            }),
            "the hero draws a wide image, so the list answer's backdrop is part of the item"
        );
        assert_eq!(
            alien.files, 2,
            "the missing file is still a file the item holds"
        );
        assert_eq!(alien.missing_files, 1);
        let arrival = rows
            .iter()
            .find(|row| row.title_id == 2)
            .expect("the series");
        assert_eq!(arrival.kind, TitleKind::Show);
        assert_eq!(arrival.released, Some("2016-11-11".parse().unwrap()));
        assert_eq!(arrival.files, 0, "a title the library holds nothing of");
    }

    #[tokio::test]
    async fn a_later_publish_replaces_the_whole_manifest() {
        let directory = TempDir::new().unwrap();
        let database = catalogue(&directory).await;

        database
            .replace_discover_rails(&[("trending_week".to_owned(), vec![1, 2])])
            .await
            .unwrap();
        database
            .replace_discover_rails(&[("popular_shows".to_owned(), vec![2])])
            .await
            .unwrap();

        let rows = database.discover_rails(any_account()).await.unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| (row.rail.as_str(), row.title_id))
                .collect::<Vec<_>>(),
            vec![("popular_shows", 2)],
            "a publish is a whole answer: the rail it did not name is not in the manifest"
        );
    }

    #[tokio::test]
    async fn a_rail_that_lists_one_title_twice_is_refused() {
        let directory = TempDir::new().unwrap();
        let database = catalogue(&directory).await;

        let error = database
            .replace_discover_rails(&[("trending_week".to_owned(), vec![1, 1])])
            .await
            .expect_err("a rail lists a title once");
        assert!(
            error.to_string().contains("UNIQUE"),
            "the database is what refuses it, so the write cannot half-apply: {error}"
        );
        assert!(
            database
                .discover_rails(any_account())
                .await
                .unwrap()
                .is_empty(),
            "a refused write leaves the previous manifest in place"
        );
    }

    #[tokio::test]
    async fn a_restricted_scope_hydrates_only_what_it_may_see() {
        let directory = TempDir::new().unwrap();
        let database = catalogue(&directory).await;
        database
            .replace_discover_rails(&[("trending_week".to_owned(), vec![1, 3])])
            .await
            .unwrap();

        let unrestricted = database.discover_rails(any_account()).await.unwrap();
        assert_eq!(unrestricted.len(), 2);
        let visible = database.discover_rails(restricted()).await.unwrap();
        assert_eq!(
            visible.iter().map(|row| row.title_id).collect::<Vec<_>>(),
            vec![1],
            "an adult title is invisible to a restricted account (AGENTS R34)"
        );
    }

    #[tokio::test]
    async fn deleting_a_title_takes_its_places_with_it() {
        let directory = TempDir::new().unwrap();
        let database = catalogue(&directory).await;
        database
            .replace_discover_rails(&[
                ("trending_week".to_owned(), vec![1, 2]),
                ("popular_movies".to_owned(), vec![1]),
            ])
            .await
            .unwrap();

        let path = path_of(&directory);
        execute(&path, "DELETE FROM title WHERE id = 1").await;

        let rows = database.discover_rails(any_account()).await.unwrap();
        assert_eq!(
            rows.iter().map(|row| row.title_id).collect::<Vec<_>>(),
            vec![2],
            "the cascade removes the places a deleted title held, in every rail"
        );
    }

    #[tokio::test]
    async fn a_title_that_does_not_exist_fails_the_whole_write() {
        let directory = TempDir::new().unwrap();
        let database = catalogue(&directory).await;

        assert!(
            database
                .replace_discover_rails(&[("trending_week".to_owned(), vec![1, 9_999])])
                .await
                .is_err(),
            "the manifest names catalogue rows, and the database is what says so"
        );
        assert!(
            database
                .discover_rails(any_account())
                .await
                .unwrap()
                .is_empty(),
            "and the transaction leaves nothing behind"
        );
    }
}
