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

use peregrine_domain::catalogue::{EpisodeDetails, SeasonDetails, TitleDetails};
use peregrine_domain::library::TitleCandidate;
use peregrine_domain::matching::LocalTitle;
use peregrine_domain::text::NonEmptyText;
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

/// Inserts one season row and answers its id.
///
/// No search index row: `episode` and `season` are not searched, and FTS5 is what the title and its
/// aliases are for. The statement is the whole transaction because there is nothing to keep in step
/// with it.
pub(crate) async fn insert_season(
    connection: &mut sqlx::SqliteConnection,
    season: &StubSeason,
) -> Result<i64, StorageError> {
    let result = sqlx::query(
        "INSERT INTO season (title_id, season_number, is_specials, updated_at) VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(season.title_id)
    .bind(i64::from(season.season_number))
    // Season zero is where a show's specials live, and it is the one number whose meaning is fixed
    // rather than a convention: `DATA_MODEL` stores `is_specials` so a read does not have to know it.
    .bind(i64::from(season.season_number == 0))
    .bind(season.at)
    .execute(&mut *connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(result.last_insert_rowid())
}

/// Inserts one episode row and answers its id.
pub(crate) async fn insert_episode(
    connection: &mut sqlx::SqliteConnection,
    episode: &StubEpisode,
) -> Result<i64, StorageError> {
    let result = sqlx::query(
        "INSERT INTO episode (title_id, season_id, season_number, episode_number, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(episode.title_id)
    .bind(episode.season_id)
    .bind(i64::from(episode.season_number))
    .bind(i64::from(episode.episode_number))
    .bind(episode.at)
    .execute(&mut *connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(result.last_insert_rowid())
}

/// Writes what a provider said about one season, and answers its row id with whether anything moved.
///
/// **An upsert on `UNIQUE (title_id, season_number)`, so enrichment is idempotent.** A season the
/// library created as a stub — a row that exists so `episode.season_id` has somewhere to point — is
/// filled by the first provider that describes it, and a later run over the same answer adds no second
/// row: the unique index is the identity `DATA_MODEL` §2.3 gives a season, and a second row numbered
/// one would be two seasons a read cannot choose between.
///
/// **Every descriptive column is written with `COALESCE`, so an absent field leaves what is stored
/// alone**, for the reason [`update_title_metadata`] gives: a provider drops fields between releases,
/// and treating an omission as a value would erase a synopsis a user can already see.
///
/// **The number is the key and never a column this statement writes**, which is the ordering rule
/// `DATA_MODEL` §2.3 states: canonical numbering is the aired order the primary provider published, and
/// a provider whose own scheme disagrees is an `episode_ordering` row rather than a renumbering,
/// because rewriting the numbers breaks every already-matched file. `is_specials` is not written here
/// either: it is a function of the number, which is the conflict key, so an existing row can never need
/// it corrected.
///
/// The row id comes back because the caller writes that season's episodes against it, and `changed`
/// because a re-run that moved nothing must be able to say so — the `WHERE` on the update is what makes
/// that observable, since `SQLite` counts an update that stores the value already there.
pub(crate) async fn upsert_season_details(
    connection: &mut sqlx::SqliteConnection,
    title_id: i64,
    details: &SeasonDetails,
    provider: &str,
    at: i64,
) -> Result<(i64, bool), StorageError> {
    let number = i64::from(details.season_number.get());
    // The provider is bound only when this answer carried a path. A row recording a path without the
    // provider that serves it — or a provider with another provider's path — produces an artwork key
    // that names a picture nobody ever fetched, which renders as a poster that is silently absent.
    let provider = details.poster_path.as_ref().map(|_| provider);
    let air_date = details.air_date.map(|date| date.to_string());

    let mut transaction = connection.begin().await.map_err(StorageError::from_sqlx)?;
    let changed = sqlx::query(
        "INSERT INTO season (title_id, season_number, name, overview, air_date, poster_provider, \
         poster_path, episode_count, is_specials, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
         ON CONFLICT (title_id, season_number) DO UPDATE SET \
             name = COALESCE(excluded.name, season.name), \
             overview = COALESCE(excluded.overview, season.overview), \
             air_date = COALESCE(excluded.air_date, season.air_date), \
             poster_provider = COALESCE(excluded.poster_provider, season.poster_provider), \
             poster_path = COALESCE(excluded.poster_path, season.poster_path), \
             episode_count = COALESCE(excluded.episode_count, season.episode_count), \
             updated_at = excluded.updated_at \
         WHERE COALESCE(excluded.name, season.name) IS NOT season.name \
            OR COALESCE(excluded.overview, season.overview) IS NOT season.overview \
            OR COALESCE(excluded.air_date, season.air_date) IS NOT season.air_date \
            OR COALESCE(excluded.poster_provider, season.poster_provider) \
                 IS NOT season.poster_provider \
            OR COALESCE(excluded.poster_path, season.poster_path) IS NOT season.poster_path \
            OR COALESCE(excluded.episode_count, season.episode_count) IS NOT season.episode_count",
    )
    .bind(title_id)
    .bind(number)
    .bind(details.name.as_deref())
    .bind(details.overview.as_deref())
    .bind(air_date)
    .bind(provider)
    .bind(details.poster_path.as_deref())
    .bind(details.episode_count.map(i64::from))
    // Season zero is where a show's specials live, and the schema stores the flag so a read does not
    // have to know the convention.
    .bind(i64::from(details.season_number.get() == 0))
    .bind(at)
    .execute(&mut *transaction)
    .await
    .map_err(StorageError::from_sqlx)?
    .rows_affected()
        > 0;

    // The row's id either way, because an upsert's `last_insert_rowid` is the *previous* row's when the
    // conflict branch ran. The lookup is the unique index this statement just matched on.
    let (id,): (i64,) =
        sqlx::query_as("SELECT id FROM season WHERE title_id = ?1 AND season_number = ?2")
            .bind(title_id)
            .bind(number)
            .fetch_one(&mut *transaction)
            .await
            .map_err(StorageError::from_sqlx)?;

    transaction
        .commit()
        .await
        .map_err(StorageError::from_sqlx)?;
    Ok((id, changed))
}

/// Writes what a provider said about one episode, and answers whether anything moved.
///
/// **An upsert on `UNIQUE (title_id, season_number, episode_number)`, and the numbers are the only
/// identity it matches on.** That is the ordering rule the module exists to protect: the canonical
/// columns hold the aired order the primary provider published, an alternate scheme is an
/// `episode_ordering` row, and enrichment therefore writes *descriptive columns only*. `season_id` is
/// assigned by the row's first writer and is not a column this statement updates either — an episode
/// does not move between seasons because a later answer was read.
///
/// **A gap in the provider's answer leaves the missing episode alone.** An upsert keyed on the published
/// numbers cannot renumber a season from the order an answer happened to list, which is what a
/// positional write would do; an episode the provider no longer mentions simply is not visited.
///
/// Everything else follows [`upsert_season_details`]: `COALESCE` so an absent field never clears a
/// stored one, the provider bound only when a path came with it, and `changed` reported as whether the
/// statement moved a value.
pub(crate) async fn upsert_episode_details(
    connection: &mut sqlx::SqliteConnection,
    title_id: i64,
    season_id: i64,
    details: &EpisodeDetails,
    provider: &str,
    at: i64,
) -> Result<bool, StorageError> {
    let provider = details.still_path.as_ref().map(|_| provider);
    let air_date = details.air_date.map(|date| date.to_string());

    let changed = sqlx::query(
        "INSERT INTO episode (title_id, season_id, season_number, episode_number, name, overview, \
         air_date, runtime_minutes, still_provider, still_path, vote_average, vote_count, \
         episode_type, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14) \
         ON CONFLICT (title_id, season_number, episode_number) DO UPDATE SET \
             name = COALESCE(excluded.name, episode.name), \
             overview = COALESCE(excluded.overview, episode.overview), \
             air_date = COALESCE(excluded.air_date, episode.air_date), \
             runtime_minutes = COALESCE(excluded.runtime_minutes, episode.runtime_minutes), \
             still_provider = COALESCE(excluded.still_provider, episode.still_provider), \
             still_path = COALESCE(excluded.still_path, episode.still_path), \
             vote_average = COALESCE(excluded.vote_average, episode.vote_average), \
             vote_count = COALESCE(excluded.vote_count, episode.vote_count), \
             episode_type = COALESCE(excluded.episode_type, episode.episode_type), \
             updated_at = excluded.updated_at \
         WHERE COALESCE(excluded.name, episode.name) IS NOT episode.name \
            OR COALESCE(excluded.overview, episode.overview) IS NOT episode.overview \
            OR COALESCE(excluded.air_date, episode.air_date) IS NOT episode.air_date \
            OR COALESCE(excluded.runtime_minutes, episode.runtime_minutes) \
                 IS NOT episode.runtime_minutes \
            OR COALESCE(excluded.still_provider, episode.still_provider) \
                 IS NOT episode.still_provider \
            OR COALESCE(excluded.still_path, episode.still_path) IS NOT episode.still_path \
            OR COALESCE(excluded.vote_average, episode.vote_average) IS NOT episode.vote_average \
            OR COALESCE(excluded.vote_count, episode.vote_count) IS NOT episode.vote_count \
            OR COALESCE(excluded.episode_type, episode.episode_type) IS NOT episode.episode_type",
    )
    .bind(title_id)
    .bind(season_id)
    .bind(i64::from(details.season_number.get()))
    .bind(i64::from(details.episode_number.get()))
    .bind(details.name.as_deref())
    .bind(details.overview.as_deref())
    .bind(air_date)
    .bind(details.runtime_minutes.map(i64::from))
    .bind(provider)
    .bind(details.still_path.as_deref())
    .bind(details.rating)
    .bind(details.votes.and_then(|votes| i64::try_from(votes).ok()))
    .bind(
        details
            .episode_type
            .map(peregrine_domain::EpisodeType::as_str),
    )
    .bind(at)
    .execute(&mut *connection)
    .await
    .map_err(StorageError::from_sqlx)?
    .rows_affected()
        > 0;

    Ok(changed)
}

/// A match rule as a caller writes one, before the database assigns it an id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMatchRule {
    /// The title the parse read, normalized by the caller.
    pub pattern: String,
    /// The title the user said it was.
    pub title_id: i64,
    /// Unix milliseconds, for `created_at`.
    pub at: i64,
}

/// Records a match rule, or moves the existing rule for that pattern to where the user just said.
///
/// An upsert rather than an insert, because `UNIQUE (pattern)` means one pattern has one answer: a
/// user who corrects the same pattern twice is changing their mind, and a second row would leave the
/// read with two answers and no way to choose.
pub(crate) async fn insert_match_rule(
    connection: &mut sqlx::SqliteConnection,
    rule: &crate::catalogue::NewMatchRule,
) -> Result<i64, StorageError> {
    sqlx::query(
        "INSERT INTO match_rule (pattern, title_id, created_at) VALUES (?1, ?2, ?3) \
         ON CONFLICT (pattern) DO UPDATE SET title_id = excluded.title_id",
    )
    .bind(&rule.pattern)
    .bind(rule.title_id)
    .bind(rule.at)
    .execute(&mut *connection)
    .await
    .map_err(StorageError::from_sqlx)?;

    // The row's id either way, because the caller reports the rule it now has and an upsert's
    // `last_insert_rowid` is the *previous* row's when the conflict branch ran.
    let row: (i64,) = sqlx::query_as("SELECT id FROM match_rule WHERE pattern = ?1")
        .bind(&rule.pattern)
        .fetch_one(&mut *connection)
        .await
        .map_err(StorageError::from_sqlx)?;
    Ok(row.0)
}

/// One title's name, or `None` when the id names nothing.
///
/// The scanner's surface, like the lookups beside it: the match action resolves a suggestion's id to
/// the name a placement takes, and the suggestion was recorded by a scan.
pub(crate) async fn title_name(
    pool: &sqlx::SqlitePool,
    id: i64,
) -> Result<Option<String>, StorageError> {
    let row: Option<(String,)> = sqlx::query_as("SELECT name FROM title WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(StorageError::from_sqlx)?;
    Ok(row.map(|row| row.0))
}

/// The titles whose sort names fall between `lower` and `upper`, for a suggestion lookup.
///
/// A range rather than a `LIKE 'prefix%'`: the candidates a suggestion can be built from are
/// *prefixes* of the parked name, so they lie between the name truncated to the shortest usable
/// length and the name itself — and a range comparison uses `title_sort_name_idx`, while a pattern on
/// the right-hand side of `LIKE` cannot. The range is a superset of the answer by design: whether a
/// candidate really is a prefix, and whether what follows it means anything, is
/// [`peregrine_domain::library::suggest_title`]'s decision, not the statement's.
///
/// Unscoped, like the placement lookups beside it — the *scanner* is what writes a suggestion. The
/// queue's read is scoped, so a suggestion pointing at a title an account may not see is filtered
/// where a person would see it rather than where it is recorded.
pub(crate) async fn title_candidates_between(
    pool: &sqlx::SqlitePool,
    lower: &str,
    upper: &str,
) -> Result<Vec<TitleCandidate>, StorageError> {
    let rows = sqlx::query_as::<_, TitleCandidateRow>(
        "SELECT id, sort_name FROM title WHERE sort_name >= ?1 AND sort_name <= ?2 \
         ORDER BY length(sort_name) DESC LIMIT ?3",
    )
    .bind(lower)
    .bind(upper)
    .bind(i64::from(CANDIDATE_LIMIT))
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// How many candidates one suggestion lookup reads.
///
/// A bound rather than a page: the reading is the scanner's, the row count for a prefix is small by
/// construction, and a name that matched more than this many titles would be answered by no
/// suggestion anyway — the domain rule refuses ambiguity, and two titles sharing a sort name is
/// already one.
const CANDIDATE_LIMIT: u32 = 32;

/// One rule a user made, with the title it places files under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchRuleRow {
    /// The row id, which is what a removal names.
    pub id: i64,
    /// The title the parse read, normalized — what a later name is matched by.
    pub pattern: String,
    /// The title the rule places a matching file under.
    pub title_name: String,
}

/// A page of the rules a user has made, newest first.
///
/// Scoped, unlike the placement lookups beside it: this is a list a *person* reads, so a rule
/// pointing at a title an account may not see is not offered to it (I37). The placement's own read
/// takes no scope for the reason the module doc gives — a file exists whether or not anyone may see
/// the title it belongs to — and the two differ in audience rather than in subject.
pub(crate) async fn list_match_rules(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    request: PageRequest,
) -> Result<Vec<MatchRuleRow>, StorageError> {
    let rows = sqlx::query_as::<_, MatchRuleListRow>(
        "SELECT r.id, r.pattern, t.name AS title_name \
           FROM match_rule r JOIN title t ON t.id = r.title_id \
          WHERE (?1 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL)) \
          ORDER BY r.id DESC LIMIT ?2",
    )
    .bind(crate::library::hidden_by_scope(scope))
    .bind(request.limit())
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// Removes one rule, reporting whether the row was there.
///
/// What makes a wrong rule recoverable: a rule places every later file that reads like the one it was
/// made from, so a user who answered a question wrongly needs to take the answer back — and the files
/// it already placed are theirs to keep or remove through the library, which is a different action.
pub(crate) async fn remove_match_rule(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM match_rule WHERE id = ?1")
        .bind(id)
        .execute(&mut *connection)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// The title a user's rule places files under, for one pattern.
///
/// The *name* rather than the id, because that is what the placement needs: the indexer places a
/// file by the name it read, which is the same name the pattern was normalized from, so a rule's
/// answer and a fresh parse take one path rather than two.
///
/// Read on the scanner's side of the boundary, like the placement lookups beside it: the indexer
/// asks before it parks a file, and a rule is about a naming habit rather than about an account's
/// right to see a title (`crate::library`'s module doc states the exception).
pub(crate) async fn rule_title_for(
    pool: &sqlx::SqlitePool,
    pattern: &str,
) -> Result<Option<String>, StorageError> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT t.name FROM match_rule r JOIN title t ON t.id = r.title_id WHERE r.pattern = ?1",
    )
    .bind(pattern)
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(row.map(|row| row.0))
}

/// The id of the season a title and number already name, or `None`.
pub(crate) async fn season_id_for(
    pool: &sqlx::SqlitePool,
    title_id: i64,
    season_number: u32,
) -> Result<Option<i64>, StorageError> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM season WHERE title_id = ?1 AND season_number = ?2")
            .bind(title_id)
            .bind(i64::from(season_number))
            .fetch_optional(pool)
            .await
            .map_err(StorageError::from_sqlx)?;
    Ok(row.map(|row| row.0))
}

/// The id of the episode a title, season, and number already name, or `None`.
pub(crate) async fn episode_id_for(
    pool: &sqlx::SqlitePool,
    title_id: i64,
    season_number: u32,
    episode_number: u32,
) -> Result<Option<i64>, StorageError> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM episode WHERE title_id = ?1 AND season_number = ?2 AND episode_number = ?3",
    )
    .bind(title_id)
    .bind(i64::from(season_number))
    .bind(i64::from(episode_number))
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(row.map(|row| row.0))
}

/// The identity a stub season needs, and nothing more.
///
/// A season is a row the library creates to hang episodes off; a provider sync fills in its name,
/// overview, air date, and artwork later (`PG-E5-06`). It exists because `episode.season_id` is a
/// foreign key rather than a number: a placement that had the season wrong would put the episode
/// under the wrong set of rows, and the schema's `UNIQUE (title_id, season_number)` is what stops
/// two seasons existing for one number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StubSeason {
    /// The title the season belongs to.
    pub title_id: i64,
    /// The season number, which zero is for a show's specials.
    pub season_number: u32,
    /// Unix milliseconds, for `updated_at`; the table has no `created_at`.
    pub at: i64,
}

/// The identity a stub episode needs, and nothing more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StubEpisode {
    /// The title the episode belongs to, which the schema denormalises beside the season.
    pub title_id: i64,
    /// The season it sits in.
    pub season_id: i64,
    /// The season number, denormalised by the schema for the read that lists a title's episodes.
    pub season_number: u32,
    /// The episode number within the season.
    pub episode_number: u32,
    /// Unix milliseconds, for `updated_at`.
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

/// One title's identity without an account's scope.
///
/// The library matcher asks what a scanned title is called so it can search a provider for it.
/// That is not a read on an account's behalf: it happens before any account has chosen to look at
/// the title, and the id came from the scanner's own placement. Every screen read still takes a scope.
pub(crate) async fn identity_unrestricted(
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

/// A title a provider has identified, as an upsert creates one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTitle {
    /// The provider's own identifier, which is the identity the upsert keys on.
    pub external_id: ExternalId,
    /// A film or a series.
    pub kind: TitleKind,
    /// Standard, anime, documentary, or reality.
    pub subtype: TitleSubtype,
    /// The name the provider lists it under.
    pub name: String,
    /// The name to sort by, already normalized by the caller.
    pub sort_name: String,
    /// Whether this id is the title's primary one.
    pub is_primary: bool,
    /// Unix milliseconds.
    pub at: i64,
}

/// Finds the title an external id belongs to, or creates it and attaches the id — in one transaction.
///
/// The two steps are one because splitting them leaves a window with no owner: `title` has no UNIQUE
/// constraint, so two syncs that both read "no such id" would both insert a stub, only one
/// `attach_external_ids` could win, and the loser would leave **an orphan title that nothing points
/// at**. One transaction is what makes the id and the row arrive together or not at all.
pub(crate) async fn upsert_provider_title(
    connection: &mut sqlx::SqliteConnection,
    title: &ProviderTitle,
) -> Result<i64, StorageError> {
    let mut transaction = connection.begin().await.map_err(StorageError::from_sqlx)?;

    let existing: Option<(i64,)> =
        sqlx::query_as("SELECT title_id FROM external_id WHERE provider = ?1 AND value = ?2")
            .bind(title.external_id.provider().as_str())
            .bind(title.external_id.value())
            .fetch_optional(&mut *transaction)
            .await
            .map_err(StorageError::from_sqlx)?;
    if let Some((id,)) = existing {
        transaction
            .commit()
            .await
            .map_err(StorageError::from_sqlx)?;
        return Ok(id);
    }

    // The stub row and its index row, in this transaction rather than `insert_stub`'s own: a nested
    // `begin` would fail, and the index row has to commit with the title or the title is unsearchable
    // until a rebuild.
    let inserted = sqlx::query(
        "INSERT INTO title (kind, subtype, name, sort_name, metadata_state, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, 'stub', ?5, ?5)",
    )
    .bind(title.kind.as_str())
    .bind(title.subtype.as_str())
    .bind(&title.name)
    .bind(&title.sort_name)
    .bind(title.at)
    .execute(&mut *transaction)
    .await
    .map_err(StorageError::from_sqlx)?;
    let id = inserted.last_insert_rowid();
    crate::fts::reindex(&mut transaction, id)
        .await
        .map_err(StorageError::from_sqlx)?;

    sqlx::query(
        "INSERT INTO external_id (title_id, provider, value, is_primary, fetched_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(id)
    .bind(title.external_id.provider().as_str())
    .bind(title.external_id.value())
    .bind(i64::from(title.is_primary))
    .bind(title.at)
    .execute(&mut *transaction)
    .await
    .map_err(StorageError::from_sqlx)?;

    transaction
        .commit()
        .await
        .map_err(StorageError::from_sqlx)?;
    Ok(id)
}

/// Writes what a provider said onto an existing title, and reindexes it.
///
/// **Every optional field is written with `COALESCE`, so an absent one leaves what is stored alone.** A
/// provider omits freely and drops fields between releases; treating an omission as a value would erase
/// a synopsis or a rating the user can already see, and the sync has no way to tell the two apart.
///
/// The search index is rewritten in the same transaction, because `title`'s own columns feed it and no
/// trigger watches the table (`fts` module doc). A title whose name changed without an index rewrite is
/// a title search cannot find by its new name.
///
/// **`Listed` is a floor rather than an assignment, and that is why the state is written by a
/// `CASE`.** The two callers are a details fill and a list answer, and the same title is both:
/// every rails run re-lists the titles it drew, so a bare assignment would rewrite a title that a
/// details read had filled back to `Listed` on the next fetch — which makes
/// `peregrine_app::artwork::refresh_due` true again, and asks the provider for details it has
/// already given once per visit, for ever. A list answer therefore raises `stub` to `listed` and
/// leaves every state that already means a details read happened exactly as it was; a details fill
/// still writes what it was told to write.
pub(crate) async fn update_title_metadata(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
    details: &TitleDetails,
    sort_name: &str,
    state: MetadataState,
    provider: &str,
    at: i64,
) -> Result<bool, StorageError> {
    let origin_country = details
        .origin_country
        .as_ref()
        .and_then(|codes| serde_json::to_string(codes).ok());
    let (release, first_air) = match details.kind {
        TitleKind::Movie => (details.released, None),
        TitleKind::Show => (None, details.released),
    };
    let release = release.map(|date| date.to_string());
    let first_air = first_air.map(|date| date.to_string());

    let mut transaction = connection.begin().await.map_err(StorageError::from_sqlx)?;
    let changed = sqlx::query(
        "UPDATE title SET \
            name = ?2, sort_name = ?3, \
            original_name = COALESCE(?4, original_name), \
            overview = COALESCE(?5, overview), \
            tagline = COALESCE(?6, tagline), \
            release_date = COALESCE(?7, release_date), \
            first_air_date = COALESCE(?8, first_air_date), \
            runtime_minutes = COALESCE(?9, runtime_minutes), \
            original_language = COALESCE(?10, original_language), \
            origin_country = COALESCE(?11, origin_country), \
            tmdb_rating = COALESCE(?12, tmdb_rating), \
            tmdb_votes = COALESCE(?13, tmdb_votes), \
            popularity = COALESCE(?14, popularity), \
            adult = COALESCE(?15, adult), \
            in_production = COALESCE(?16, in_production), \
            season_count = COALESCE(?17, season_count), \
            episode_count = COALESCE(?18, episode_count), \
            homepage = COALESCE(?19, homepage), \
            poster_provider = COALESCE(?20, poster_provider), \
            poster_path = COALESCE(?21, poster_path), \
            backdrop_provider = COALESCE(?20, backdrop_provider), \
            backdrop_path = COALESCE(?22, backdrop_path), \
            metadata_state = CASE \
                WHEN ?23 = 'listed' AND metadata_state <> 'stub' THEN metadata_state \
                ELSE ?23 \
              END, \
            last_synced_at = ?24, updated_at = ?24 \
         WHERE id = ?1",
    )
    .bind(id)
    .bind(details.name.as_str())
    .bind(sort_name)
    .bind(details.original_name.as_deref())
    .bind(details.overview.as_deref())
    .bind(details.tagline.as_deref())
    .bind(release)
    .bind(first_air)
    .bind(details.runtime_minutes.map(i64::from))
    .bind(details.original_language.as_deref())
    .bind(origin_country.as_deref())
    .bind(details.rating)
    .bind(details.votes.and_then(|votes| i64::try_from(votes).ok()))
    .bind(details.popularity)
    .bind(details.adult.map(i64::from))
    .bind(details.in_production.map(i64::from))
    .bind(details.season_count.map(i64::from))
    .bind(details.episode_count.map(i64::from))
    .bind(details.homepage.as_deref())
    .bind(provider)
    .bind(details.poster_path.as_deref())
    .bind(details.backdrop_path.as_deref())
    .bind(state.as_str())
    .bind(at)
    .execute(&mut *transaction)
    .await
    .map_err(StorageError::from_sqlx)?;

    let changed = changed.rows_affected() > 0;
    if changed {
        crate::fts::reindex(&mut transaction, id)
            .await
            .map_err(StorageError::from_sqlx)?;
    }
    transaction
        .commit()
        .await
        .map_err(StorageError::from_sqlx)?;
    Ok(changed)
}

/// Records that a provider has answered this title's season step, and when.
///
/// **A fact of its own rather than a side effect of the season writes.** A show whose provider lists
/// no seasons writes no `season` row, and it is still a show whose seasons have been read; deriving
/// this from "does the season table hold a row" would ask such a show again on every visit. The other
/// direction matters more: a season read that *fails* leaves this column alone, so the title stays due
/// and the next visit retries, where a marker written before the read would have made the failure
/// permanent (PG-E5-06).
///
/// `at` is when the answer arrived, not how fresh the data is: the staleness policy that will read
/// this column (`PG-E6-15`) does not exist yet, and a timestamp is still the right shape for it.
pub(crate) async fn mark_title_seasons_synced(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
    at: i64,
) -> Result<bool, StorageError> {
    // `updated_at` moves with it: the column is the row's own "something about me changed", and a
    // marker that left it behind would make the two disagree about when this title was last written.
    let changed =
        sqlx::query("UPDATE title SET season_synced_at = ?2, updated_at = ?2 WHERE id = ?1")
            .bind(id)
            .bind(at)
            .execute(connection)
            .await
            .map_err(StorageError::from_sqlx)?
            .rows_affected()
            > 0;
    Ok(changed)
}

/// The local titles a candidate name could be, with everything the matching rule reads.
///
/// One row per title of that kind whose `sort_name` matches, plus its aliases and external ids — the
/// whole input `peregrine_domain::matching` needs, gathered here so the sync does not have to know the
/// schema. Aliases come back too, because a title is often listed under a name a person never typed and
/// matching only on `sort_name` would miss it.
pub(crate) async fn local_titles_for_sort_name(
    pool: &sqlx::SqlitePool,
    kind: &str,
    sort_name: &str,
) -> Result<Vec<LocalTitle>, StorageError> {
    let rows: Vec<(i64, String, String, Option<String>)> = sqlx::query_as(
        "SELECT id, kind, name, COALESCE(release_date, first_air_date) FROM title \
         WHERE sort_name = ?1 AND kind = ?2 ORDER BY id",
    )
    .bind(sort_name)
    .bind(kind)
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;

    let mut titles = Vec::with_capacity(rows.len());
    for (id, kind, name, released) in rows {
        // A row with no usable name cannot match anything by name, and `NonEmptyText` is how the rule
        // says so. A date that does not parse is a date nobody recorded, which is the same answer the
        // rule reaches either way.
        let (Some(kind), Ok(name)) = (TitleKind::parse(&kind), NonEmptyText::new(&name)) else {
            continue;
        };
        let external_ids = external_ids(pool, id)
            .await?
            .into_iter()
            .map(|held| held.id)
            .collect();
        let aliases = aliases(pool, id)
            .await?
            .into_iter()
            .filter_map(|alias| NonEmptyText::new(&alias).ok())
            .collect();
        titles.push(LocalTitle {
            id,
            kind,
            name,
            released: released.as_deref().and_then(|date| date.parse().ok()),
            external_ids,
            aliases,
        });
    }
    Ok(titles)
}

/// Every alias of one title, which the matching rule reads beside its name.
pub(crate) async fn aliases(
    pool: &sqlx::SqlitePool,
    title_id: i64,
) -> Result<Vec<String>, StorageError> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT alias FROM title_alias WHERE title_id = ?1 ORDER BY id")
            .bind(title_id)
            .fetch_all(pool)
            .await
            .map_err(StorageError::from_sqlx)?;
    Ok(rows.into_iter().map(|row| row.0).collect())
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

#[cfg(test)]
mod episode_tests {
    use tempfile::TempDir;

    use peregrine_domain::catalogue::{EpisodeDetails, EpisodeType, SeasonDetails};
    use peregrine_domain::{AccountId, AccountScope, EpisodeNumber, SeasonNumber};

    use crate::test_support::{count, execute, path_of};
    use crate::{
        Database, NewMatchRule, PageRequest, StubSeason, StubTitle, TitleKind, TitleSubtype,
    };

    use super::StubEpisode;

    const NOW: i64 = 1_700_000_000_000;

    async fn database(directory: &TempDir) -> (Database, i64) {
        let database = Database::open(directory.path().join("peregrine.sqlite"))
            .await
            .unwrap();
        let title = database
            .insert_stub_title(StubTitle {
                kind: TitleKind::Show,
                subtype: TitleSubtype::Standard,
                name: "Breaking Bad".to_owned(),
                original_name: None,
                sort_name: "breaking bad".to_owned(),
                at: NOW,
            })
            .await
            .unwrap();
        (database, title)
    }

    #[tokio::test]
    async fn a_season_and_its_episodes_round_trip_by_number() {
        // What a placement needs: create the rows once, find them again by the numbers the
        // filenames stated, and never create a second one for the same number.
        let directory = TempDir::new().unwrap();
        let (database, title) = database(&directory).await;

        let season = database
            .insert_stub_season(StubSeason {
                title_id: title,
                season_number: 1,
                at: NOW,
            })
            .await
            .unwrap();
        let episode = database
            .insert_stub_episode(StubEpisode {
                title_id: title,
                season_id: season,
                season_number: 1,
                episode_number: 2,
                at: NOW,
            })
            .await
            .unwrap();

        assert_eq!(
            database.season_id_for(title, 1).await.unwrap(),
            Some(season)
        );
        assert_eq!(
            database.episode_id_for(title, 1, 2).await.unwrap(),
            Some(episode)
        );
        assert_eq!(database.season_id_for(title, 2).await.unwrap(), None);
        assert_eq!(database.episode_id_for(title, 1, 3).await.unwrap(), None);
    }

    #[tokio::test]
    async fn the_schema_refuses_a_second_season_or_episode_for_one_number() {
        // The unique indices (I2's neighbours) are what stop a placement that lost its lookup from
        // writing a duplicate: the second insert fails rather than the library quietly holding two
        // seasons numbered one.
        let directory = TempDir::new().unwrap();
        let (database, title) = database(&directory).await;
        let season = database
            .insert_stub_season(StubSeason {
                title_id: title,
                season_number: 1,
                at: NOW,
            })
            .await
            .unwrap();
        database
            .insert_stub_episode(StubEpisode {
                title_id: title,
                season_id: season,
                season_number: 1,
                episode_number: 1,
                at: NOW,
            })
            .await
            .unwrap();

        assert!(
            database
                .insert_stub_season(StubSeason {
                    title_id: title,
                    season_number: 1,
                    at: NOW,
                })
                .await
                .is_err()
        );
        assert!(
            database
                .insert_stub_episode(StubEpisode {
                    title_id: title,
                    season_id: season,
                    season_number: 1,
                    episode_number: 1,
                    at: NOW,
                })
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_rule_is_listed_with_its_title_and_can_be_taken_back() {
        // The two halves a user needs to trust a rule: see what it says, and remove it. A rule places
        // every later file that reads like the one it was made from, so a wrong answer has to be
        // withdrawable — and the scope is applied here rather than at the call site (I37).
        let directory = TempDir::new().unwrap();
        let (database, title) = database(&directory).await;
        let rule = database
            .insert_match_rule(NewMatchRule {
                pattern: "the matrix".to_owned(),
                title_id: title,
                at: NOW,
            })
            .await
            .unwrap();

        let listed = database
            .list_match_rules(
                AccountScope::unrestricted(AccountId::new(1).expect("account one")),
                PageRequest::first(10),
            )
            .await
            .unwrap();

        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, rule);
        assert_eq!(listed[0].pattern, "the matrix");
        assert_eq!(listed[0].title_name, "Breaking Bad", "the fixture's title");
        assert!(database.remove_match_rule(rule).await.unwrap());
        assert!(
            !database.remove_match_rule(rule).await.unwrap(),
            "removing a rule that is gone reports that nothing changed"
        );
        // `assert_eq!` rather than `assert!` so a failure shows what was still there (clippy's
        // `assert_is_empty`, which is right: "the list is not empty" is not a diagnosis).
        assert_eq!(
            database
                .list_match_rules(
                    AccountScope::unrestricted(AccountId::new(1).expect("account one")),
                    PageRequest::first(10)
                )
                .await
                .unwrap(),
            Vec::new()
        );
    }

    #[tokio::test]
    async fn a_rule_for_a_title_a_restricted_scope_cannot_see_is_not_listed() {
        // I37 at the rules list: the same answer for a hidden rule as for one that does not exist.
        // The placement read is deliberately not scoped — a file is placed whether or not an account
        // may see the title — and this list is, because a person is reading it.
        let directory = TempDir::new().unwrap();
        let (database, title) = database(&directory).await;
        // The fixture's title is a stub, and a stub has no certification — which is exactly what a
        // restricted scope hides, so no second write is needed to set the case up.
        database
            .insert_match_rule(NewMatchRule {
                pattern: "the matrix".to_owned(),
                title_id: title,
                at: NOW,
            })
            .await
            .unwrap();

        let restricted = database
            .list_match_rules(
                AccountScope::restricted(AccountId::new(1).expect("account one")),
                PageRequest::first(10),
            )
            .await
            .unwrap();

        assert!(
            restricted.is_empty(),
            "an unrated title's rule is not offered"
        );
    }

    #[tokio::test]
    async fn one_pattern_has_one_rule() {
        // I40: a user who corrects the same pattern twice is changing their mind, and the read that
        // consults a rule has to have one answer. A second row for a pattern would leave it choosing.
        let directory = TempDir::new().unwrap();
        let (database, first) = database(&directory).await;
        let second = database
            .insert_stub_title(StubTitle {
                kind: TitleKind::Movie,
                subtype: TitleSubtype::Standard,
                name: "The Matrix Reloaded".to_owned(),
                original_name: None,
                sort_name: "the matrix reloaded".to_owned(),
                at: NOW,
            })
            .await
            .unwrap();
        let rule = |title_id: i64| NewMatchRule {
            pattern: "the matrix".to_owned(),
            title_id,
            at: NOW,
        };

        database.insert_match_rule(rule(first)).await.unwrap();
        database.insert_match_rule(rule(second)).await.unwrap();

        assert_eq!(
            database
                .rule_title_for("the matrix")
                .await
                .unwrap()
                .as_deref(),
            Some("The Matrix Reloaded"),
            "the later correction is the rule"
        );
        let pool = sqlx::SqlitePool::connect(&format!(
            "sqlite://{}",
            directory.path().join("peregrine.sqlite").display()
        ))
        .await
        .unwrap();
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM match_rule")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "one pattern, one row");
    }

    #[tokio::test]
    async fn season_zero_is_marked_as_specials() {
        // Season zero is where a show's specials live, and the schema stores a flag so a read does
        // not have to know that convention. A row written without it would sort specials among the
        // seasons as if they were one.
        let directory = TempDir::new().unwrap();
        let (database, title) = database(&directory).await;
        let season = database
            .insert_stub_season(StubSeason {
                title_id: title,
                season_number: 0,
                at: NOW,
            })
            .await
            .unwrap();

        let pool = sqlx::SqlitePool::connect(&format!(
            "sqlite://{}",
            directory.path().join("peregrine.sqlite").display()
        ))
        .await
        .unwrap();
        let (specials,): (i64,) = sqlx::query_as("SELECT is_specials FROM season WHERE id = ?1")
            .bind(season)
            .fetch_one(&pool)
            .await
            .unwrap();

        assert_eq!(specials, 1);
    }

    /// The provider's answer for one season, with the fields a season row has.
    fn season_details(number: u32) -> SeasonDetails {
        SeasonDetails {
            season_number: SeasonNumber::new(number),
            name: Some(format!("Season {number}")),
            overview: Some("A season of television.".to_owned()),
            air_date: Some("2008-01-20".parse().expect("a date")),
            poster_path: Some(format!("/season-{number}.jpg")),
            episode_count: Some(7),
        }
    }

    /// The provider's answer for one episode, with every descriptive column set.
    fn episode_details(season: u32, episode: u32) -> EpisodeDetails {
        EpisodeDetails {
            season_number: SeasonNumber::new(season),
            episode_number: EpisodeNumber::new(episode),
            name: Some(format!("Episode {episode}")),
            overview: Some("An episode.".to_owned()),
            air_date: Some("2008-01-27".parse().expect("a date")),
            runtime_minutes: Some(47),
            still_path: Some(format!("/still-{season}-{episode}.jpg")),
            rating: Some(8.1),
            votes: Some(1_200),
            episode_type: Some(EpisodeType::Standard),
        }
    }

    #[tokio::test]
    async fn a_re_run_over_the_same_answer_reports_that_nothing_changed() {
        // `RowChanged`/`RowUpserted` are what make a re-run observable, so a second pass over an
        // unchanged answer must report `false` for every row and add none. A sync that could not tell
        // "already held" from "wrote it again" has idempotence as an intention rather than a fact —
        // and the `WHERE` on the upsert is the only thing that makes SQLite's row count mean anything,
        // since it counts an update that stored the value already there.
        let directory = TempDir::new().unwrap();
        let (database, title) = database(&directory).await;

        let (season, created) = database
            .upsert_season_details(title, season_details(1), "tmdb".to_owned(), NOW)
            .await
            .unwrap();
        assert!(created, "the first write creates the season row");
        assert!(
            database
                .upsert_episode_details(
                    title,
                    season,
                    episode_details(1, 1),
                    "tmdb".to_owned(),
                    NOW
                )
                .await
                .unwrap(),
            "and the first write of an episode fills it"
        );

        let (same, created) = database
            .upsert_season_details(title, season_details(1), "tmdb".to_owned(), NOW)
            .await
            .unwrap();
        assert_eq!(same, season, "the second write is the same row");
        assert!(!created, "and it moved nothing");
        assert!(
            !database
                .upsert_episode_details(
                    title,
                    season,
                    episode_details(1, 1),
                    "tmdb".to_owned(),
                    NOW
                )
                .await
                .unwrap(),
            "an episode the catalogue already holds is not written a second time"
        );

        let path = path_of(&directory);
        assert_eq!(count(&path, "SELECT COUNT(*) FROM season").await, 1);
        assert_eq!(count(&path, "SELECT COUNT(*) FROM episode").await, 1);
    }

    #[tokio::test]
    async fn a_season_the_provider_announced_with_no_episodes_is_still_a_season() {
        // The state between the season list and the season read: the switcher draws the season from
        // `episode_count`, and a read of its episodes answers an empty list. A season that only existed
        // once an episode referenced it would be a season a screen could not show.
        let directory = TempDir::new().unwrap();
        let (database, title) = database(&directory).await;

        let (_, created) = database
            .upsert_season_details(title, season_details(2), "tmdb".to_owned(), NOW)
            .await
            .unwrap();

        let path = path_of(&directory);
        assert!(created);
        assert_eq!(count(&path, "SELECT COUNT(*) FROM season").await, 1);
        assert_eq!(count(&path, "SELECT episode_count FROM season").await, 7);
        assert_eq!(count(&path, "SELECT COUNT(*) FROM episode").await, 0);
    }

    #[tokio::test]
    async fn an_episode_with_no_air_date_and_no_still_is_still_written() {
        // A provider dates and pictures some episodes and not others. An episode missing both is an
        // episode with a name and a runtime rather than a row the sync skips — and an *absent* field
        // never clears a stored one, because a provider that dropped a still between releases would
        // otherwise blank a card that has a picture on disk.
        let directory = TempDir::new().unwrap();
        let (database, title) = database(&directory).await;
        let (season, _) = database
            .upsert_season_details(title, season_details(1), "tmdb".to_owned(), NOW)
            .await
            .unwrap();
        let undated = EpisodeDetails {
            air_date: None,
            still_path: None,
            ..episode_details(1, 2)
        };

        assert!(
            database
                .upsert_episode_details(title, season, undated, "tmdb".to_owned(), NOW)
                .await
                .unwrap()
        );

        let path = path_of(&directory);
        assert_eq!(
            count(&path, "SELECT COUNT(*) FROM episode WHERE air_date IS NULL").await,
            1
        );
        assert_eq!(
            count(
                &path,
                "SELECT COUNT(*) FROM episode WHERE still_path IS NULL"
            )
            .await,
            1
        );
        assert_eq!(
            count(
                &path,
                "SELECT COUNT(*) FROM episode WHERE name = 'Episode 2' AND runtime_minutes = 47"
            )
            .await,
            1,
            "the fields the answer did carry are written"
        );

        // The same episode, an answer that no longer mentions its still: the stored path survives and
        // the write reports that it changed nothing.
        let still_held = EpisodeDetails {
            still_path: None,
            air_date: None,
            ..episode_details(1, 2)
        };
        assert!(
            !database
                .upsert_episode_details(title, season, still_held, "tmdb".to_owned(), NOW)
                .await
                .unwrap()
        );
        assert_eq!(
            count(
                &path,
                "SELECT COUNT(*) FROM episode WHERE still_path IS NULL"
            )
            .await,
            1,
            "an absent path did not clear a stored one"
        );
    }

    #[tokio::test]
    async fn enrichment_never_rewrites_the_canonical_numbering() {
        // `DATA_MODEL` §2.3: canonical `season_number`/`episode_number` are always the aired order the
        // primary provider published, and any other scheme is an `episode_ordering` row — "canonical
        // plus overlay, never in-place rewriting, because rewriting breaks every already-matched file".
        //
        // The provider's answer is therefore fed in a different order and with a gap, and every
        // assertion is about a *number keeping the name the provider gave that number*. A positional
        // write — `episode_number = index + 1`, which is the shape a list-shaped answer invites — would
        // hand episode 3's name to episode 2 and report success.
        let directory = TempDir::new().unwrap();
        let (database, title) = database(&directory).await;
        let (season, _) = database
            .upsert_season_details(title, season_details(1), "tmdb".to_owned(), NOW)
            .await
            .unwrap();

        // Episode two is matched on disk and carries a DVD overlay, which is exactly the row a
        // renumbering write would damage: it is the one the provider's answer does not mention.
        let matched = database
            .insert_stub_episode(StubEpisode {
                title_id: title,
                season_id: season,
                season_number: 1,
                episode_number: 2,
                at: NOW,
            })
            .await
            .unwrap();
        execute(
            &path_of(&directory),
            &format!(
                "INSERT INTO episode_ordering (episode_id, scheme, season_number, episode_number) \
                 VALUES ({matched}, 'dvd', 1, 1)"
            ),
        )
        .await;

        for number in [3, 1] {
            database
                .upsert_episode_details(
                    title,
                    season,
                    episode_details(1, number),
                    "tmdb".to_owned(),
                    NOW,
                )
                .await
                .unwrap();
        }

        let path = path_of(&directory);
        assert_eq!(count(&path, "SELECT COUNT(*) FROM episode").await, 3);
        for number in [1, 3] {
            assert_eq!(
                count(
                    &path,
                    &format!(
                        "SELECT COUNT(*) FROM episode WHERE episode_number = {number} \
                         AND name = 'Episode {number}'"
                    )
                )
                .await,
                1,
                "episode {number} keeps the answer that named it"
            );
        }
        assert_eq!(
            count(
                &path,
                "SELECT COUNT(*) FROM episode WHERE episode_number = 2 AND name IS NULL"
            )
            .await,
            1,
            "an episode the answer did not mention is left exactly as the library recorded it"
        );
        assert_eq!(
            count(
                &path,
                &format!(
                    "SELECT COUNT(*) FROM episode_ordering WHERE episode_id = {matched} \
                     AND scheme = 'dvd' AND season_number = 1 AND episode_number = 1"
                )
            )
            .await,
            1,
            "the alternate scheme is an overlay row, and enrichment neither reads nor moves it"
        );
        // The lookup every already-matched file resolves through still finds the same row.
        assert_eq!(
            database.episode_id_for(title, 1, 2).await.unwrap(),
            Some(matched)
        );
    }
}

/// The row shape of the scoped rule list.
#[derive(sqlx::FromRow)]
struct MatchRuleListRow {
    id: i64,
    pattern: String,
    title_name: String,
}

impl From<MatchRuleListRow> for MatchRuleRow {
    fn from(row: MatchRuleListRow) -> Self {
        Self {
            id: row.id,
            pattern: row.pattern,
            title_name: row.title_name,
        }
    }
}

/// The row shape of a suggestion lookup.
#[derive(sqlx::FromRow)]
struct TitleCandidateRow {
    id: i64,
    sort_name: String,
}

impl From<TitleCandidateRow> for TitleCandidate {
    fn from(row: TitleCandidateRow) -> Self {
        Self {
            id: row.id,
            sort_name: row.sort_name,
        }
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
