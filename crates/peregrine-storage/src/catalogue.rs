//! The catalogue's identity: a title row in its `stub` state, and the external ids that
//! point at it.
//!
//! A provider does not identify a title by its name — two remakes share one — so a title is
//! found by an external id and created as a *stub* when none exists. That is not a
//! placeholder: `title.metadata_state` has a documented `stub` value for exactly this, and
//! the fields a provider fills later are nullable in the schema (`DATA_MODEL` §2.2, and the
//! sync flow in §4).
//!
//! The rest of the title's columns arrive with the provider adapters (PG-E5), which is where
//! the field-by-field mapping and its normalization rules belong; this module owns the
//! identity those adapters hang their data off.
//!
//! An external id belongs to exactly one title. Attaching one that is already attached
//! elsewhere is refused rather than moved, because silently re-pointing a provider id is how
//! two titles end up sharing a history.
//!
//! Every read takes an [`AccountScope`] (AGENTS R34). The scope is applied in SQL, not after
//! the rows are fetched, so content outside it never leaves the database: a read that
//! filtered in Rust would still have loaded the row, and the next read to forget the filter
//! would return it.

use peregrine_domain::{AccountScope, ExternalId, Provider};
use sqlx::Connection as _;

use crate::error::StorageError;
use crate::page::PageRequest;

// The three stored enums are the domain's. Declaring them here as well gave one `CHECK`
// constraint two definitions that could drift apart with nothing noticing, and `DATA_MODEL`
// §2 says the matching Rust enum is the source of truth. This module re-exports it, which
// keeps `peregrine_storage::catalogue::TitleKind` and the crate-root re-export working for
// every existing caller, and the schema test below asserts the spellings against the live
// database rather than restating them.
pub use peregrine_domain::{MetadataState, TitleKind, TitleSubtype};

/// The identity a stub title needs, and nothing more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StubTitle {
    /// Film or series.
    pub kind: TitleKind,
    /// Standard, anime, documentary, or reality.
    pub subtype: TitleSubtype,
    /// The display name the provider or the filename gave.
    pub name: String,
    /// The name in its original language, when it differs from `name`.
    pub original_name: Option<String>,
    /// The name to sort by, already normalized by the caller.
    pub sort_name: String,
    /// Unix milliseconds, for both `created_at` and `updated_at`.
    pub at: i64,
}

/// What a read of a title's identity returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleIdentity {
    /// The row id.
    pub id: i64,
    /// Film or series.
    pub kind: TitleKind,
    /// The subtype.
    pub subtype: TitleSubtype,
    /// The display name.
    pub name: String,
    /// The sort name.
    pub sort_name: String,
    /// How complete the metadata is.
    pub metadata_state: MetadataState,
    /// Whether the title is flagged adult, which a restricted scope hides.
    pub adult: bool,
}

/// One external id as stored against a title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleExternalId {
    /// The provider and its value.
    pub id: ExternalId,
    /// Whether this is the provider the title's own ids should be read from.
    pub is_primary: bool,
    /// Unix milliseconds when the id was last fetched.
    pub fetched_at: i64,
}

/// Inserts one stub title, indexes it, and reports the row id.
///
/// The title row and its search index row are written in one transaction. A title that
/// committed without an index row would be invisible to search until a rebuild, and a
/// rebuild is a maintenance operation rather than a step the write path may depend on.
pub(crate) async fn insert_stub(
    connection: &mut sqlx::SqliteConnection,
    stub: &StubTitle,
) -> Result<i64, StorageError> {
    let mut transaction = connection.begin().await.map_err(StorageError::from_sqlx)?;

    // `metadata_state` is written literally rather than bound: this statement creates a
    // stub, and the state that says so is part of what a stub is.
    let result = sqlx::query(
        "INSERT INTO title (kind, subtype, name, original_name, sort_name, metadata_state, \
         created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 'stub', ?6, ?6)",
    )
    .bind(stub.kind.as_str())
    .bind(stub.subtype.as_str())
    .bind(&stub.name)
    .bind(stub.original_name.as_deref())
    .bind(&stub.sort_name)
    .bind(stub.at)
    .execute(&mut *transaction)
    .await
    .map_err(StorageError::from_sqlx)?;

    let id = result.last_insert_rowid();
    crate::fts::reindex(&mut transaction, id)
        .await
        .map_err(StorageError::from_sqlx)?;
    transaction
        .commit()
        .await
        .map_err(StorageError::from_sqlx)?;
    Ok(id)
}

/// Attaches external ids to a title, refusing one that belongs to another.
///
/// Every row in one call is one transaction: a half-attached set of ids would leave the next
/// match resolving to a title that is missing the id it was matched on.
///
/// # Errors
///
/// [`StorageError::Conflict`] when an id is already attached to a different title, in which
/// case nothing in the call is applied.
pub(crate) async fn attach_external_ids(
    connection: &mut sqlx::SqliteConnection,
    title_id: i64,
    ids: &[TitleExternalId],
) -> Result<(), StorageError> {
    let mut transaction = connection.begin().await.map_err(StorageError::from_sqlx)?;

    for entry in ids {
        // The `WHERE` on the update is the guard: an insert takes the row, and a conflict
        // only updates when the row already belongs to this title, so a conflicting update
        // affects no rows and is reported below.
        let result = sqlx::query(
            "INSERT INTO external_id (title_id, provider, value, is_primary, fetched_at) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT (provider, value) DO UPDATE SET \
                 is_primary = excluded.is_primary, fetched_at = excluded.fetched_at \
             WHERE external_id.title_id = excluded.title_id",
        )
        .bind(title_id)
        .bind(entry.id.provider().as_str())
        .bind(entry.id.value())
        .bind(i64::from(entry.is_primary))
        .bind(entry.fetched_at)
        .execute(&mut *transaction)
        .await
        .map_err(StorageError::from_sqlx)?;

        if result.rows_affected() == 0 {
            transaction
                .rollback()
                .await
                .map_err(StorageError::from_sqlx)?;
            return Err(StorageError::Conflict {
                detail: format!("`{}` is already attached to another title", entry.id),
            });
        }
    }

    transaction.commit().await.map_err(StorageError::from_sqlx)
}

/// The scope's rule, as a flag a query binds.
///
/// A flag rather than assembled SQL: the alternative is building the `WHERE` clause from a
/// bool, which is how string composition creeps into a repository. The clause itself is a
/// literal in each statement, and this only decides whether its first branch applies.
pub(crate) const fn hidden_by_scope(scope: AccountScope) -> i64 {
    scope.is_restricted() as i64
}

/// Reads one title's identity, when the scope may see it.
///
/// A title outside the scope reads as `None`, which is the whole point of R34: a restricted
/// account must not be able to tell "no such title" from "not for you".
pub(crate) async fn identity(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    id: i64,
) -> Result<Option<TitleIdentity>, StorageError> {
    let row = sqlx::query_as::<_, IdentityRow>(
        "SELECT id, kind, subtype, name, sort_name, metadata_state, adult FROM title \
         WHERE id = ?1 AND (?2 = 0 OR (adult = 0 AND content_rating IS NOT NULL))",
    )
    .bind(id)
    .bind(hidden_by_scope(scope))
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    row.map(TryInto::try_into).transpose()
}

/// Reads one page of titles the scope may see, newest first.
///
/// The scope is the first branch of the `WHERE` clause and the cursor the second, so a
/// restricted page is still a page: the rows that were never visible do not consume the
/// limit, which a filter applied after the query would have done.
pub(crate) async fn list_titles(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    request: PageRequest,
) -> Result<Vec<TitleIdentity>, StorageError> {
    let rows = sqlx::query_as::<_, IdentityRow>(
        "SELECT id, kind, subtype, name, sort_name, metadata_state, adult FROM title \
         WHERE (?1 = 0 OR (adult = 0 AND content_rating IS NOT NULL)) \
           AND (?2 IS NULL OR id < ?2) ORDER BY id DESC LIMIT ?3",
    )
    .bind(hidden_by_scope(scope))
    .bind(request.bindable_cursor())
    .bind(request.bindable_limit())
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    rows.into_iter().map(TryInto::try_into).collect()
}

/// Reads the title an external id belongs to.
pub(crate) async fn title_for(
    pool: &sqlx::SqlitePool,
    id: &ExternalId,
) -> Result<Option<i64>, StorageError> {
    let title_id = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT title_id FROM external_id WHERE provider = ?1 AND value = ?2",
    )
    .bind(id.provider().as_str())
    .bind(id.value())
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    // A row whose owner is a person, season, or episode has a NULL `title_id`; it is not
    // this id's title, and `None` is the honest answer for a caller looking for one.
    Ok(title_id.flatten())
}

/// Reads every external id attached to a title.
pub(crate) async fn external_ids(
    pool: &sqlx::SqlitePool,
    title_id: i64,
) -> Result<Vec<TitleExternalId>, StorageError> {
    let rows = sqlx::query_as::<_, ExternalIdRow>(
        "SELECT provider, value, is_primary, fetched_at FROM external_id \
         WHERE title_id = ?1 ORDER BY provider, value",
    )
    .bind(title_id)
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;

    rows.into_iter().map(TryInto::try_into).collect()
}

/// Counts the rows in `title`, which is how a test asks whether a write was a no-op.
pub(crate) async fn title_count(pool: &sqlx::SqlitePool) -> Result<i64, StorageError> {
    sqlx::query_scalar("SELECT COUNT(*) FROM title")
        .fetch_one(pool)
        .await
        .map_err(StorageError::from_sqlx)
}

/// The `title` columns a scoped read returns.
///
/// Shared with `fts`, which reads the same columns for a search result: one mapper is what
/// keeps a title read by id and a title found by search the same value.
#[derive(sqlx::FromRow)]
pub(crate) struct IdentityRow {
    id: i64,
    kind: String,
    subtype: String,
    name: String,
    sort_name: String,
    metadata_state: String,
    adult: i64,
}

impl TryFrom<IdentityRow> for TitleIdentity {
    type Error = StorageError;

    fn try_from(row: IdentityRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            kind: TitleKind::parse(&row.kind).ok_or_else(|| StorageError::UnexpectedValue {
                column: "title.kind",
                found: row.kind.clone(),
            })?,
            subtype: TitleSubtype::parse(&row.subtype).ok_or_else(|| {
                StorageError::UnexpectedValue {
                    column: "title.subtype",
                    found: row.subtype.clone(),
                }
            })?,
            name: row.name,
            sort_name: row.sort_name,
            metadata_state: MetadataState::parse(&row.metadata_state).ok_or_else(|| {
                StorageError::UnexpectedValue {
                    column: "title.metadata_state",
                    found: row.metadata_state.clone(),
                }
            })?,
            adult: row.adult != 0,
        })
    }
}

#[derive(sqlx::FromRow)]
struct ExternalIdRow {
    provider: String,
    value: String,
    is_primary: i64,
    fetched_at: i64,
}

impl TryFrom<ExternalIdRow> for TitleExternalId {
    type Error = StorageError;

    fn try_from(row: ExternalIdRow) -> Result<Self, Self::Error> {
        let provider =
            row.provider
                .parse::<Provider>()
                .map_err(|_| StorageError::UnexpectedValue {
                    column: "external_id.provider",
                    found: row.provider.clone(),
                })?;
        let id =
            ExternalId::new(provider, &row.value).map_err(|_| StorageError::UnexpectedValue {
                column: "external_id.value",
                found: row.value.clone(),
            })?;
        Ok(Self {
            id,
            is_primary: row.is_primary != 0,
            fetched_at: row.fetched_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{Database, open_writer_connection};
    use crate::page::Cursor;
    use peregrine_domain::AccountId;
    use sqlx::SqliteConnection as SqlxConnection;
    use tempfile::TempDir;

    const NOW: i64 = 1_700_000_000_000;

    async fn database(directory: &TempDir) -> Database {
        Database::open(directory.path().join("peregrine.sqlite"))
            .await
            .unwrap()
    }

    fn stub(name: &str, kind: TitleKind) -> StubTitle {
        StubTitle {
            kind,
            subtype: TitleSubtype::Standard,
            name: name.to_owned(),
            original_name: None,
            sort_name: name.to_lowercase(),
            at: NOW,
        }
    }

    fn id(text: &str) -> ExternalId {
        ExternalId::parse(text).expect("the fixture is a valid external id")
    }

    /// The scope the fixtures are read with: an account with no ceiling and no curation.
    fn any_account() -> AccountScope {
        AccountScope::unrestricted(AccountId::new(1).expect("account one exists"))
    }

    /// Inserts a title with the two columns a scope reads.
    ///
    /// The stub insert deliberately leaves `adult` and `content_rating` alone — they arrive
    /// with the provider sync — so a test that is about visibility has to set them itself.
    async fn insert_rated(
        connection: &mut SqlxConnection,
        name: &str,
        adult: i64,
        rating: Option<&str>,
    ) -> i64 {
        let result = sqlx::query(
            "INSERT INTO title (kind, subtype, name, sort_name, content_rating, adult, \
             metadata_state, created_at, updated_at) \
             VALUES ('movie', 'standard', ?1, ?2, ?3, ?4, 'complete', 0, 0)",
        )
        .bind(name)
        .bind(name.to_lowercase())
        .bind(rating)
        .bind(adult)
        .execute(&mut *connection)
        .await
        .unwrap();
        result.last_insert_rowid()
    }

    /// The ids of a page, in the order it was read.
    fn ids_of(titles: &[TitleIdentity]) -> Vec<i64> {
        titles.iter().map(|title| title.id).collect()
    }

    fn attached(text: &str, is_primary: bool) -> TitleExternalId {
        TitleExternalId {
            id: id(text),
            is_primary,
            fetched_at: NOW,
        }
    }

    /// The spellings of the `CHECK (column IN (...))` clause the migration wrote.
    ///
    /// Read out of `CREATE TABLE` text rather than a table of names, because the point of the
    /// test below is to compare the enum against what the database actually accepts.
    fn constraint_spellings(create_table: &str, column: &str) -> Vec<String> {
        let marker = format!("CHECK ({column} IN (");
        let start = create_table
            .find(&marker)
            .unwrap_or_else(|| panic!("`{column}` has no IN constraint"))
            + marker.len();
        let rest = &create_table[start..];
        let end = rest
            .find("))")
            .unwrap_or_else(|| panic!("`{column}`'s constraint does not close"));
        let mut spellings: Vec<String> = rest[..end]
            .split(',')
            .map(|literal| literal.trim().trim_matches('\'').to_owned())
            .collect();
        spellings.sort();
        spellings
    }

    /// Each enum's spellings are exactly the ones the live schema accepts.
    ///
    /// The constraint text is read from `sqlite_master` after a real migration run. The test
    /// this replaced compared a hardcoded list against itself, so it could not fail and its
    /// claim that the enum and the database "cannot drift apart" was not true; this one fails
    /// when a migration widens a `CHECK` and the enum is not widened with it, which is the
    /// drift that matters.
    #[tokio::test]
    async fn every_enum_spelling_is_the_one_the_schema_constraint_lists() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");
        drop(Database::open(&path).await.unwrap());

        let mut connection = open_writer_connection(&path).await.unwrap();
        let create_table: String = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'title'",
        )
        .fetch_one(&mut connection)
        .await
        .unwrap();

        let mut expected: Vec<String> = TitleKind::ALL
            .iter()
            .map(|kind| kind.as_str().to_owned())
            .collect();
        expected.sort();
        assert_eq!(constraint_spellings(&create_table, "kind"), expected);

        let mut expected: Vec<String> = TitleSubtype::ALL
            .iter()
            .map(|subtype| subtype.as_str().to_owned())
            .collect();
        expected.sort();
        assert_eq!(constraint_spellings(&create_table, "subtype"), expected);

        let mut expected: Vec<String> = MetadataState::ALL
            .iter()
            .map(|state| state.as_str().to_owned())
            .collect();
        expected.sort();
        assert_eq!(
            constraint_spellings(&create_table, "metadata_state"),
            expected
        );
    }

    /// Every spelling an enum can produce is one it can parse back, so a value read from the
    /// database always round-trips and an unknown one is refused rather than defaulted.
    #[test]
    fn every_spelling_round_trips_and_unknown_ones_are_refused() {
        for kind in TitleKind::ALL {
            assert_eq!(TitleKind::parse(kind.as_str()), Some(*kind));
        }
        assert_eq!(TitleKind::parse("series"), None);
        assert_eq!(TitleKind::parse(""), None);

        for subtype in TitleSubtype::ALL {
            assert_eq!(TitleSubtype::parse(subtype.as_str()), Some(*subtype));
        }
        assert_eq!(TitleSubtype::parse("cartoon"), None);

        for state in MetadataState::ALL {
            assert_eq!(MetadataState::parse(state.as_str()), Some(*state));
        }
        assert_eq!(MetadataState::parse("fresh"), None);
    }

    #[tokio::test]
    async fn a_stub_title_round_trips() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;

        let id = database
            .insert_stub_title(StubTitle {
                kind: TitleKind::Show,
                subtype: TitleSubtype::Anime,
                name: "Cowboy Bebop".to_owned(),
                original_name: Some("カウボーイビバップ".to_owned()),
                sort_name: "cowboy bebop".to_owned(),
                at: NOW,
            })
            .await
            .unwrap();

        let identity = database
            .title_identity(any_account(), id)
            .await
            .unwrap()
            .expect("the row");
        assert_eq!(identity.id, id);
        assert_eq!(identity.kind, TitleKind::Show);
        assert_eq!(identity.subtype, TitleSubtype::Anime);
        assert_eq!(identity.name, "Cowboy Bebop");
        assert_eq!(identity.sort_name, "cowboy bebop");
        assert_eq!(
            identity.metadata_state,
            MetadataState::Stub,
            "an inserted title is a stub until a provider fills it"
        );
        assert!(!identity.adult);

        assert_eq!(database.title_count().await.unwrap(), 1);
        assert_eq!(
            database
                .title_identity(any_account(), id + 1)
                .await
                .unwrap(),
            None,
            "an id that does not exist reads as nothing"
        );
    }

    #[tokio::test]
    async fn external_ids_attach_and_resolve() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        let title = database
            .insert_stub_title(stub("Alien", TitleKind::Movie))
            .await
            .unwrap();

        database
            .attach_external_ids(
                title,
                &[
                    attached("tmdb:348", true),
                    attached("imdb:tt0078748", false),
                ],
            )
            .await
            .unwrap();

        assert_eq!(
            database
                .title_for_external_id(&id("tmdb:348"))
                .await
                .unwrap(),
            Some(title)
        );
        assert_eq!(
            database
                .title_for_external_id(&id("imdb:tt0078748"))
                .await
                .unwrap(),
            Some(title)
        );
        assert_eq!(
            database.title_for_external_id(&id("tvdb:1")).await.unwrap(),
            None,
            "an id nobody attached resolves to nothing"
        );

        let ids = database.external_ids_of(title).await.unwrap();
        assert_eq!(
            ids,
            vec![
                attached("imdb:tt0078748", false),
                attached("tmdb:348", true),
            ],
            "ids are read back in provider order"
        );
    }

    /// Attaching the same id twice updates the row rather than duplicating it, which is what
    /// makes a sync idempotent.
    #[tokio::test]
    async fn attaching_twice_updates_the_row() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        let title = database
            .insert_stub_title(stub("Alien", TitleKind::Movie))
            .await
            .unwrap();

        database
            .attach_external_ids(title, &[attached("tmdb:348", false)])
            .await
            .unwrap();
        database
            .attach_external_ids(title, &[attached("tmdb:348", true)])
            .await
            .unwrap();

        let ids = database.external_ids_of(title).await.unwrap();
        assert_eq!(ids.len(), 1, "one provider id is one row");
        assert!(ids[0].is_primary);
    }

    /// The hazard this module exists to prevent: two titles sharing one provider id.
    #[tokio::test]
    async fn an_id_that_belongs_to_another_title_is_refused() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        let first = database
            .insert_stub_title(stub("Alien", TitleKind::Movie))
            .await
            .unwrap();
        let second = database
            .insert_stub_title(stub("Aliens", TitleKind::Movie))
            .await
            .unwrap();

        database
            .attach_external_ids(first, &[attached("tmdb:348", true)])
            .await
            .unwrap();

        let error = database
            .attach_external_ids(second, &[attached("tmdb:348", true)])
            .await
            .expect_err("a provider id belongs to one title");
        assert!(matches!(error, StorageError::Conflict { .. }), "{error:?}");
        assert!(
            error.to_string().contains("tmdb:348"),
            "the refusal must name the id: {error}"
        );

        assert_eq!(
            database
                .title_for_external_id(&id("tmdb:348"))
                .await
                .unwrap(),
            Some(first),
            "the first title keeps its id"
        );
        assert!(
            database.external_ids_of(second).await.unwrap().is_empty(),
            "the refused call attaches nothing"
        );
    }

    /// A partial failure inside one call leaves nothing behind: the second id is taken, so
    /// the first must not stay attached.
    #[tokio::test]
    async fn a_refused_batch_attaches_nothing() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        let owner = database
            .insert_stub_title(stub("Alien", TitleKind::Movie))
            .await
            .unwrap();
        let other = database
            .insert_stub_title(stub("Aliens", TitleKind::Movie))
            .await
            .unwrap();

        database
            .attach_external_ids(owner, &[attached("imdb:tt0078748", true)])
            .await
            .unwrap();

        let error = database
            .attach_external_ids(
                other,
                &[
                    attached("tmdb:679", true),
                    attached("imdb:tt0078748", false),
                ],
            )
            .await
            .expect_err("the batch is refused as a whole");
        assert!(matches!(error, StorageError::Conflict { .. }), "{error:?}");

        assert!(
            database.external_ids_of(other).await.unwrap().is_empty(),
            "the first id of the refused batch must not stay attached"
        );
    }

    /// The whole point, end to end: find or create, once.
    #[tokio::test]
    async fn find_or_create_yields_one_title_for_one_provider_id() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        let wanted = id("tmdb:348");

        let missing = database.title_for_external_id(&wanted).await.unwrap();
        assert_eq!(missing, None, "the first look finds nothing");

        let created = database
            .insert_stub_title(stub("Alien", TitleKind::Movie))
            .await
            .unwrap();
        database
            .attach_external_ids(created, &[attached("tmdb:348", true)])
            .await
            .unwrap();

        assert_eq!(
            database.title_for_external_id(&wanted).await.unwrap(),
            Some(created),
            "the second look finds the title that was created"
        );
        assert_eq!(database.title_count().await.unwrap(), 1);
    }

    /// A row this build cannot read is reported rather than guessed at.
    ///
    /// The `CHECK` constraint would refuse an unknown `kind` through the pool, so the
    /// constraint is disabled for the one insert: that is what a file written by another
    /// version of this application amounts to, and the mapper's refusal is the path that
    /// keeps such a row from being read as a guess.
    #[tokio::test]
    async fn a_row_this_build_cannot_read_is_reported() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");
        let database = database(&directory).await;

        let mut connection = crate::database::open_writer_connection(&path)
            .await
            .unwrap();
        sqlx::query("PRAGMA ignore_check_constraints = ON")
            .execute(&mut connection)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO title (kind, subtype, name, sort_name, metadata_state, created_at, \
             updated_at) VALUES ('series', 'standard', 'X', 'x', 'stub', 0, 0)",
        )
        .execute(&mut connection)
        .await
        .unwrap();
        connection.close().await.unwrap();

        let error = database
            .title_identity(any_account(), 1)
            .await
            .expect_err("a kind this build does not know must not be read as one it does");
        assert!(
            matches!(
                error,
                StorageError::UnexpectedValue {
                    column: "title.kind",
                    ..
                }
            ),
            "{error:?}"
        );
        assert!(error.to_string().contains("series"), "{error}");
    }

    /// **I25** under R34: every catalogue read takes an `AccountScope`, and an unrestricted one
    /// sees what the catalogue holds, whatever the rating.
    #[tokio::test]
    async fn an_unrestricted_scope_sees_every_title() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");
        let database = database(&directory).await;
        let (rated, unrated, adult) = rated_fixtures(&path).await;

        let visible = database
            .list_titles(any_account(), PageRequest::first(10))
            .await
            .unwrap();

        assert_eq!(
            ids_of(&visible),
            vec![adult, unrated, rated],
            "newest first, and nothing is hidden"
        );
        assert!(
            visible.iter().any(|title| title.adult),
            "the adult flag is reported rather than stripped"
        );
    }

    /// **I26** and **I27** under R34's fail-closed rule: a restricted scope sees neither
    /// adult-flagged content (**I26**) nor content with no certification (**I27**), because
    /// unrated content is not evidence that it is safe.
    #[tokio::test]
    async fn a_restricted_scope_hides_adult_and_unrated_content() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");
        let database = database(&directory).await;
        let (rated, unrated, adult) = rated_fixtures(&path).await;

        let scope = AccountScope::restricted(AccountId::new(2).unwrap());
        let visible = database
            .list_titles(scope, PageRequest::first(10))
            .await
            .unwrap();
        assert_eq!(ids_of(&visible), vec![rated]);

        // A title outside the scope is indistinguishable from one that does not exist: the
        // read answers `None` for both, so a caller cannot probe for what it may not see.
        assert_eq!(database.title_identity(scope, adult).await.unwrap(), None);
        assert_eq!(database.title_identity(scope, unrated).await.unwrap(), None);
        assert_eq!(
            database.title_identity(scope, i64::MAX).await.unwrap(),
            None
        );
        assert!(
            database
                .title_identity(scope, rated)
                .await
                .unwrap()
                .is_some(),
            "a rated title it may see is still readable by id"
        );

        // The same rows are readable by the account that owns them, so the hiding is the
        // scope's and not a row that failed to be written.
        assert!(
            database
                .title_identity(any_account(), adult)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            database
                .title_identity(any_account(), unrated)
                .await
                .unwrap()
                .is_some()
        );
    }

    /// A stub has no certification, so a restricted scope cannot see it until a provider
    /// supplies one. That is the fail-closed rule applied to the sync path, and it is worth
    /// naming because it is easy to mistake for a bug.
    #[tokio::test]
    async fn a_fresh_stub_is_not_visible_to_a_restricted_scope() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        let stub = database
            .insert_stub_title(stub("Alien", TitleKind::Movie))
            .await
            .unwrap();

        let scope = AccountScope::restricted(AccountId::new(2).unwrap());
        assert!(
            database
                .list_titles(scope, PageRequest::first(10))
                .await
                .unwrap()
                .is_empty(),
            "a stub carries no certification, so a restricted scope cannot see it"
        );
        assert_eq!(database.title_identity(scope, stub).await.unwrap(), None);
        assert!(
            database
                .title_identity(any_account(), stub)
                .await
                .unwrap()
                .is_some(),
            "the row exists; the scope is what hides it"
        );
    }

    /// Hidden rows must not consume a page: a scope applied after the query would return
    /// short pages and eventually none at all, which looks like the catalogue emptying.
    #[tokio::test]
    async fn a_restricted_page_is_still_a_full_page() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");
        let database = database(&directory).await;

        let mut setup = open_writer_connection(&path).await.unwrap();
        let mut visible = Vec::new();
        for index in 0..4 {
            visible
                .push(insert_rated(&mut setup, &format!("Visible {index}"), 0, Some("PG")).await);
            let _ = insert_rated(&mut setup, &format!("Hidden {index}"), 0, None).await;
        }
        setup.close().await.unwrap();

        let scope = AccountScope::restricted(AccountId::new(2).unwrap());
        let first = database
            .list_titles(scope, PageRequest::first(2))
            .await
            .unwrap();
        assert_eq!(
            first.len(),
            2,
            "two visible rows, not two rows minus hidden ones"
        );

        let second = database
            .list_titles(
                scope,
                PageRequest::after(Cursor::new(first.last().unwrap().id), 2),
            )
            .await
            .unwrap();
        assert_eq!(ids_of(&second), vec![visible[1], visible[0]]);
        assert!(
            database
                .list_titles(
                    scope,
                    PageRequest::after(Cursor::new(second.last().unwrap().id), 2)
                )
                .await
                .unwrap()
                .is_empty(),
            "the pages end where the visible rows do"
        );
    }

    /// The empty case: a catalogue with nothing in it lists nothing, for either scope.
    #[tokio::test]
    async fn an_empty_catalogue_lists_nothing() {
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;

        for scope in [
            any_account(),
            AccountScope::restricted(AccountId::new(2).unwrap()),
        ] {
            assert!(
                database
                    .list_titles(scope, PageRequest::first(10))
                    .await
                    .unwrap()
                    .is_empty(),
                "an empty catalogue lists nothing"
            );
        }
    }

    /// Three titles that differ only in what a scope reads: a rated one, an unrated one, and
    /// an adult one. Returned in insertion order, so a test can say which is which.
    async fn rated_fixtures(path: &std::path::Path) -> (i64, i64, i64) {
        let mut setup = open_writer_connection(path).await.unwrap();
        let rated = insert_rated(&mut setup, "Alien", 0, Some("R")).await;
        let unrated = insert_rated(&mut setup, "Unrated", 0, None).await;
        let adult = insert_rated(&mut setup, "Adult", 1, Some("X")).await;
        setup.close().await.unwrap();
        (rated, unrated, adult)
    }
}
