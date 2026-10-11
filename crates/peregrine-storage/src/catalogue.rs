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

use peregrine_domain::{ExternalId, Provider};
use sqlx::Connection as _;

use crate::error::StorageError;

/// A title's kind, as `title.kind` stores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleKind {
    /// A film.
    Movie,
    /// A series.
    Show,
}

impl TitleKind {
    /// The spelling the `CHECK` constraint accepts.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Show => "show",
        }
    }

    /// Parses the stored spelling, or `None` when it is not one this build knows.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "movie" => Some(Self::Movie),
            "show" => Some(Self::Show),
            _ => None,
        }
    }
}

/// A title's subtype, as `title.subtype` stores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleSubtype {
    /// An ordinary film or series.
    Standard,
    /// Animated, ordered by absolute numbering as often as not.
    Anime,
    /// Non-fiction.
    Documentary,
    /// Unscripted.
    Reality,
}

impl TitleSubtype {
    /// The spelling the `CHECK` constraint accepts.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Anime => "anime",
            Self::Documentary => "documentary",
            Self::Reality => "reality",
        }
    }

    /// Parses the stored spelling, or `None` when it is not one this build knows.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "standard" => Some(Self::Standard),
            "anime" => Some(Self::Anime),
            "documentary" => Some(Self::Documentary),
            "reality" => Some(Self::Reality),
            _ => None,
        }
    }
}

/// How complete a title's metadata is, as `title.metadata_state` stores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataState {
    /// The identity exists and nothing else does.
    Stub,
    /// Some fields are filled.
    Partial,
    /// Every field the providers supply is filled.
    Complete,
    /// Filled, but past its refresh window.
    Stale,
    /// The last refresh failed; what is stored is still shown.
    Failed,
}

impl MetadataState {
    /// The spelling the `CHECK` constraint accepts.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stub => "stub",
            Self::Partial => "partial",
            Self::Complete => "complete",
            Self::Stale => "stale",
            Self::Failed => "failed",
        }
    }

    /// Parses the stored spelling, or `None` when it is not one this build knows.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "stub" => Some(Self::Stub),
            "partial" => Some(Self::Partial),
            "complete" => Some(Self::Complete),
            "stale" => Some(Self::Stale),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

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

/// Inserts one stub title and reports the row id.
pub(crate) async fn insert_stub(
    connection: &mut sqlx::SqliteConnection,
    stub: &StubTitle,
) -> Result<i64, sqlx::Error> {
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
    .execute(&mut *connection)
    .await?;
    Ok(result.last_insert_rowid())
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

/// Reads one title's identity.
pub(crate) async fn identity(
    pool: &sqlx::SqlitePool,
    id: i64,
) -> Result<Option<TitleIdentity>, StorageError> {
    let row = sqlx::query_as::<_, IdentityRow>(
        "SELECT id, kind, subtype, name, sort_name, metadata_state, adult FROM title WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    row.map(TryInto::try_into).transpose()
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

#[derive(sqlx::FromRow)]
struct IdentityRow {
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
    use crate::database::Database;
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

    fn attached(text: &str, is_primary: bool) -> TitleExternalId {
        TitleExternalId {
            id: id(text),
            is_primary,
            fetched_at: NOW,
        }
    }

    /// Every spelling is the one the schema's `CHECK` constraints list, so the enums and the
    /// database cannot drift apart without this failing.
    #[test]
    fn every_spelling_matches_the_schema_constraint() {
        let kinds = [(TitleKind::Movie, "movie"), (TitleKind::Show, "show")];
        for (kind, spelling) in kinds {
            assert_eq!(kind.as_str(), spelling);
            assert_eq!(TitleKind::parse(spelling), Some(kind));
        }
        assert_eq!(TitleKind::parse("series"), None);
        assert_eq!(TitleKind::parse(""), None);

        let subtypes = [
            (TitleSubtype::Standard, "standard"),
            (TitleSubtype::Anime, "anime"),
            (TitleSubtype::Documentary, "documentary"),
            (TitleSubtype::Reality, "reality"),
        ];
        for (subtype, spelling) in subtypes {
            assert_eq!(subtype.as_str(), spelling);
            assert_eq!(TitleSubtype::parse(spelling), Some(subtype));
        }
        assert_eq!(TitleSubtype::parse("cartoon"), None);

        let states = [
            (MetadataState::Stub, "stub"),
            (MetadataState::Partial, "partial"),
            (MetadataState::Complete, "complete"),
            (MetadataState::Stale, "stale"),
            (MetadataState::Failed, "failed"),
        ];
        for (state, spelling) in states {
            assert_eq!(state.as_str(), spelling);
            assert_eq!(MetadataState::parse(spelling), Some(state));
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

        let identity = database.title_identity(id).await.unwrap().expect("the row");
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
            database.title_identity(id + 1).await.unwrap(),
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
            .title_identity(1)
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
}
