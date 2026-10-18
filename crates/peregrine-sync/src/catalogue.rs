//! Syncing a provider's answers into the catalogue.
//!
//! The last mile of FLOWS F3: ask a provider, decide which answers this catalogue already holds
//! (`peregrine_domain::matching`), and write the rest. What the sync does *not* do is guess — an answer
//! the rule calls ambiguous is counted and left alone, because the wrong answer there merges two titles
//! or splits one.

use std::num::NonZeroU32;

use peregrine_domain::catalogue::{ImageKind, MetadataState, TitleDetails, TitleKind};
use peregrine_domain::matching::{Candidate, MatchDecision, match_candidate, sort_name};
use peregrine_domain::text::NonEmptyText;
use peregrine_providers::error::ProviderError;
use peregrine_providers::provider::{MetadataProvider, SearchCandidate, SearchQuery};
use peregrine_storage::{Database, ProviderTitle, StorageError, TitleExternalId};

use crate::images::{ImageCache, ImageCacheError, ImageKey, ImageVariant};

/// What one sync did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    /// Candidates that named a title the catalogue already had.
    pub matched: usize,
    /// Candidates that became a new title.
    pub created: usize,
    /// Candidates the rule could not attribute, left for a person.
    pub ambiguous: usize,
    /// The candidates' external ids, in the order the provider answered them.
    ///
    /// **A provider search has to be able to answer with its own order rather than the index's.**
    /// The local index tokenises "Spider-Man" into `spider` and `man`, so a reader who types
    /// "spiderman" has no term the index can match; the provider that answered the search does know
    /// the title, and this list is what lets the caller read those titles back without re-searching
    /// text the index cannot spell. An id whose candidate the rule left ambiguous has no title, and
    /// a caller reading it finds none.
    pub found: Vec<peregrine_domain::ExternalId>,
}

/// Why a sync stopped.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// The provider refused or failed.
    #[error("the provider could not answer")]
    Provider(#[from] ProviderError),
    /// A write or read failed.
    #[error("the catalogue could not be read or written")]
    Storage(#[from] StorageError),
}

/// Asks `provider` for `query` and writes what came back into `database`.
///
/// # Errors
///
/// [`SyncError::Provider`] when the provider fails, [`SyncError::Storage`] when a read or write does.
pub async fn sync_search(
    provider: &dyn MetadataProvider,
    database: &Database,
    query: &SearchQuery,
    at: i64,
) -> Result<SyncOutcome, SyncError> {
    let candidates = provider.search(query).await?;
    let mut outcome = SyncOutcome::default();

    for candidate in candidates {
        // Recorded before any branch can skip the candidate, because the caller's read-back is about
        // what the *provider* found, including the answer the matching rule leaves as a question.
        outcome.found.push(candidate.external_id().clone());
        // Identity first, and it is a read of its own because it settles the common case without
        // gathering anything: an id the catalogue already carries is this title, whatever it is called.
        if database
            .title_for_external_id(candidate.external_id())
            .await?
            .is_some()
        {
            outcome.matched += 1;
            continue;
        }

        let Some(input) = candidate_input(&candidate) else {
            continue;
        };
        let locals = database
            .local_titles_for_sort_name(
                candidate.kind().as_str(),
                &sort_name(candidate.name().as_str()),
            )
            .await?;

        match match_candidate(&input, &locals) {
            MatchDecision::Matched { local_id, .. } => {
                // Attaching an id counts as a match even when another writer got there first: the id
                // belongs to a title now, which is the fact this loop is establishing.
                match database
                    .attach_external_ids(local_id, &[held(candidate.external_id(), at)])
                    .await
                {
                    Ok(()) | Err(StorageError::Conflict { .. }) => outcome.matched += 1,
                    Err(error) => return Err(SyncError::Storage(error)),
                }
            }
            MatchDecision::New => {
                database
                    .upsert_provider_title(ProviderTitle {
                        external_id: candidate.external_id().clone(),
                        kind: candidate.kind(),
                        subtype: peregrine_domain::catalogue::TitleSubtype::Standard,
                        name: candidate.name().as_str().to_owned(),
                        sort_name: sort_name(candidate.name().as_str()),
                        is_primary: true,
                        at,
                    })
                    .await?;
                outcome.created += 1;
            }
            // A question, not an answer: counted so the caller can report it, and left alone.
            MatchDecision::Ambiguous { .. } => outcome.ambiguous += 1,
        }
    }

    Ok(outcome)
}

/// What one title refresh wrote, and the pictures it left for the caller to fetch.
///
/// The pictures are here rather than fetched inside [`refresh_title`] because the bytes belong in the
/// image cache and this layer has no cache: the caller that owns one is the caller that fetches
/// (`AGENTS R11` — a blob never travels further than the cache that stores it). The title's own poster
/// and backdrop are on `details`; this is everything the season and episode writes added.
#[derive(Debug, Clone, PartialEq)]
pub struct RefreshedTitle {
    /// What the provider said about the title, as [`record_list_answer`] returns it.
    pub details: TitleDetails,
    /// Each picture to make sure the cache holds, as the kind and the provider-relative path.
    pub artwork: Vec<(ImageKind, String)>,
}

/// Fills a title the catalogue holds from the provider that knows it.
///
/// Answers with what it wrote, or `None` when this provider cannot serve title details at all —
/// a normal state rather than a failure, since a provider that declares only `search` is a provider this
/// step skips. The details come back rather than a bare success because the caller needs the artwork
/// paths they carry: filling a title and then fetching its poster is one step from a user's point of
/// view, and a second read to learn what was just written would be a second read for nothing.
///
/// **A series is filled in two reads, and the second is what the title page draws.** `title` carries
/// what a title has — a name, a synopsis, a poster, a score — and says nothing about the seasons under
/// it; the season list and each season's episodes are what a switcher and its cards read (`PG-E5-06`,
/// `PG-E7-13`). A film skips that step, and is not asked for seasons it cannot have.
///
/// # Errors
///
/// [`SyncError::Provider`] when the provider fails, [`SyncError::Storage`] when a read or write does.
pub async fn refresh_title(
    provider: &dyn MetadataProvider,
    database: &Database,
    id: i64,
    kind: peregrine_domain::catalogue::TitleKind,
    at: i64,
) -> Result<Option<RefreshedTitle>, SyncError> {
    // The id of *this* provider: a title may carry several, and asking the wrong provider for the wrong
    // one's identifier is how a film gets another film's synopsis.
    let external = database
        .external_ids_of(id)
        .await?
        .into_iter()
        .map(|held| held.id)
        .find(|external| external.provider() == provider.descriptor().provider());
    let Some(external) = external else {
        return Ok(None);
    };

    let details = match provider.title(&external, kind).await {
        Ok(details) => details,
        Err(ProviderError::Unsupported { .. }) => return Ok(None),
        Err(error) => return Err(SyncError::Provider(error)),
    };
    let sort = sort_name(details.name.as_str());
    let provider_name = provider.descriptor().provider().as_str().to_owned();
    database
        .update_title_metadata(
            id,
            details.clone(),
            sort,
            peregrine_domain::catalogue::MetadataState::Partial,
            provider_name.clone(),
            at,
        )
        .await?;

    let artwork = match kind {
        TitleKind::Show => {
            let artwork =
                refresh_seasons(provider, database, id, &external, &provider_name, at).await?;
            // **After the step, never before it.** The details were written above and they are what
            // makes the title look filled in; this is the only record that the *season* read
            // happened, and a marker written before it would turn a failed season read into a
            // title that is never asked about again. A failure above therefore leaves the column
            // empty and the next visit retries.
            database.mark_title_seasons_synced(id, at).await?;
            artwork
        }
        TitleKind::Movie => Vec::new(),
    };
    Ok(Some(RefreshedTitle { details, artwork }))
}

/// Reads a series' seasons and their episodes, and writes what the provider said about each.
///
/// **A season the provider does not hold is that season's problem and never the title's.** A series
/// whose answer is missing one season still has the others, and failing the whole refresh would discard
/// the seasons that did answer — a `404` on one season is a fact about a catalogue, not a fault of the
/// provider. Every other failure propagates, because a rate limit or a transport fault is about the
/// *provider* and asking again is what fixes it.
///
/// The order of the two writes matters: a season row is what `episode.season_id` points at, so the
/// season is upserted first and the id it answers with is what its episodes are written against. That
/// is also what makes an episode the library matched on disk keep the season it was placed in.
async fn refresh_seasons(
    provider: &dyn MetadataProvider,
    database: &Database,
    title_id: i64,
    external: &peregrine_domain::ExternalId,
    provider_name: &str,
    at: i64,
) -> Result<Vec<(ImageKind, String)>, SyncError> {
    let seasons = match provider.seasons(external, TitleKind::Show).await {
        Ok(seasons) => seasons,
        // A series with no season list has no seasons to write, and a provider that does not serve
        // seasons has said so: the title's own details were written before this ran, so neither is a
        // failed refresh.
        Err(ProviderError::NotFound { .. } | ProviderError::Unsupported { .. }) => {
            return Ok(Vec::new());
        }
        Err(error) => return Err(SyncError::Provider(error)),
    };

    let mut artwork = Vec::new();
    for season in seasons {
        // Read before the write moves the answer, and collected whether or not the row changed: the
        // cache may have been evicted, and the caller's `contains` check is the cheap one that decides.
        let number = season.season_number;
        let poster = season
            .poster_path
            .as_deref()
            .map(|path| (ImageKind::Poster, path.to_owned()));
        let (season_id, _) = database
            .upsert_season_details(title_id, season, provider_name.to_owned(), at)
            .await?;
        artwork.extend(poster);

        let episodes = match provider
            .season_episodes(external, TitleKind::Show, number)
            .await
        {
            Ok(episodes) => episodes,
            // The one failure that belongs to the season rather than to the provider.
            Err(ProviderError::NotFound { .. }) => continue,
            Err(error) => return Err(SyncError::Provider(error)),
        };
        for episode in episodes {
            if let Some(path) = episode.still_path.as_deref() {
                artwork.push((ImageKind::Still, path.to_owned()));
            }
            database
                .upsert_episode_details(title_id, season_id, episode, provider_name.to_owned(), at)
                .await?;
        }
    }
    Ok(artwork)
}

/// Writes what a *list* answer knows onto a title the catalogue holds, and answers with it.
///
/// A list answer — a rail, a search page — is not a title read: it carries an identity, a name, a
/// date, and at most a poster, a backdrop, a score, a short synopsis, and a vote count. Those five
/// are the whole reason this exists, because a list endpoint already sends them and a screen that had
/// to ask for a title's details to learn them would spend two requests where the provider's answer
/// needed one (`PG-E7-08`, `PG-E7-05`). The backdrop is among them because Discover's hero draws a
/// wide image rather than a portrait one, and it is the same free field the poster is — the row the
/// statement writes has held a `backdrop_path` since the details read was built, and nothing had
/// filled it from a list answer.
///
/// **It goes through the same statement a details fill does, and inherits the same rule:** every
/// optional column is written with `COALESCE`, so a later list answer carrying `None` leaves the
/// stored poster, backdrop, score, synopsis, and count alone (`PG-E5-07`). It also inherits the
/// statement's two stamps, and they are worth naming rather than discovering: `metadata_state`
/// becomes `Listed` — a list answer filled what a list carries, and no details read has happened,
/// which is the state `crate::artwork`'s `refresh_due` reads to decide that asking for the details
/// is still worth a request — and `last_synced_at` is set, because a provider did answer about the
/// title at that instant, which is what the column says and what the staleness policy
/// (`PG-E6-15`) will read.
///
/// # Errors
///
/// [`SyncError::Storage`] when the write fails.
pub async fn record_list_answer(
    database: &Database,
    title_id: i64,
    candidate: &SearchCandidate,
    at: i64,
) -> Result<TitleDetails, SyncError> {
    let details = TitleDetails {
        kind: candidate.kind(),
        name: candidate.name().clone(),
        // Everything a list answer does not carry is left `None`, which is what `COALESCE` turns
        // into "leave what is stored alone" — the alternative is erasing a synopsis because a rail
        // listed the title.
        original_name: None,
        overview: candidate.overview().map(str::to_owned),
        tagline: None,
        released: candidate.released(),
        runtime_minutes: None,
        original_language: None,
        origin_country: None,
        rating: candidate.rating(),
        votes: candidate.votes(),
        popularity: None,
        adult: None,
        in_production: None,
        season_count: None,
        episode_count: None,
        homepage: None,
        poster_path: candidate.artwork_path().map(str::to_owned),
        backdrop_path: candidate.backdrop_path().map(str::to_owned),
    };
    database
        .update_title_metadata(
            title_id,
            details.clone(),
            sort_name(candidate.name().as_str()),
            MetadataState::Listed,
            candidate.external_id().provider().as_str().to_owned(),
            at,
        )
        .await?;
    Ok(details)
}
/// One list candidate as the catalogue now holds it.
///
/// The pair rather than a bare row id because both halves have a caller: a screen needs the id to
/// navigate with, and it needs what was written — the name, the year, the score, the synopsis, the
/// artwork paths — to draw the tile without a second read. Returning only the id would make every
/// caller re-read the row it just wrote.
#[derive(Debug, Clone, PartialEq)]
pub struct ListedTitle {
    /// The catalogue row, which is what opening the title needs.
    pub title_id: i64,
    /// What the write stored, as [`record_list_answer`] read it back.
    pub details: TitleDetails,
}

/// Puts one *list* answer's candidate in the catalogue and writes what the list carried.
///
/// **The one writer for a list answer**, shared by the Discover rails job (`PG-E7-08`) and the
/// browse read (`PG-E7-09`), because both are asking the same question of the same kind of answer:
/// a provider's list gave us this candidate, do we already have it, and if so what does the list
/// add? Two copies of that rule would be two chances for a film to be merged into two titles on one
/// screen and one on another, which is exactly the class of mistake the matching rule exists to
/// prevent.
///
/// **Identity first, and it is a read of its own** because it settles the common case without
/// gathering anything: an id the catalogue already carries is this title, whatever it is called
/// ([`sync_search`]'s rule, and for the same reason). Otherwise the matching rule decides, and a
/// candidate it calls ambiguous is answered `None` rather than merged into a guess — a screen
/// cannot draw a question mark, and `PG-E5-13` owns what a person does about one.
///
/// A candidate that becomes a new title is created as a [`peregrine_domain::catalogue::TitleSubtype::Standard`]
/// stub and then immediately given the list's five fields, so a caller never sees the half-made row.
///
/// # Errors
///
/// [`SyncError::Storage`] when a read or the write fails.
pub async fn upsert_list_candidate(
    database: &Database,
    candidate: &SearchCandidate,
    at: i64,
) -> Result<Option<ListedTitle>, SyncError> {
    let title_id = match database
        .title_for_external_id(candidate.external_id())
        .await?
    {
        Some(id) => id,
        None => match match_or_create(database, candidate, at).await? {
            Some(id) => id,
            None => return Ok(None),
        },
    };
    let details = record_list_answer(database, title_id, candidate, at).await?;
    Ok(Some(ListedTitle { title_id, details }))
}

/// The matching rule, and the stub it decides to write.
///
/// Private because the only caller is [`upsert_list_candidate`]: a caller that created a stub
/// *without* writing the list answer it came from would leave a row the provider had already
/// described with none of the description.
async fn match_or_create(
    database: &Database,
    candidate: &SearchCandidate,
    at: i64,
) -> Result<Option<i64>, SyncError> {
    let Some(name) = NonEmptyText::new(candidate.name().as_str()).ok() else {
        return Ok(None);
    };
    let input = Candidate {
        external_id: candidate.external_id().clone(),
        name,
        kind: candidate.kind(),
        released: candidate.released(),
    };
    let locals = database
        .local_titles_for_sort_name(
            candidate.kind().as_str(),
            &sort_name(candidate.name().as_str()),
        )
        .await?;

    match match_candidate(&input, &locals) {
        MatchDecision::Matched { local_id, .. } => match database
            .attach_external_ids(
                local_id,
                &[TitleExternalId {
                    id: candidate.external_id().clone(),
                    is_primary: true,
                    fetched_at: at,
                }],
            )
            .await
        {
            // A concurrent writer attaching the same id to the same title is the outcome this call
            // was trying to produce, so it is the same arm and not a silent failure.
            Ok(()) | Err(StorageError::Conflict { .. }) => Ok(Some(local_id)),
            Err(error) => Err(SyncError::Storage(error)),
        },
        MatchDecision::New => database
            .upsert_provider_title(ProviderTitle {
                external_id: candidate.external_id().clone(),
                kind: candidate.kind(),
                subtype: peregrine_domain::catalogue::TitleSubtype::Standard,
                name: candidate.name().as_str().to_owned(),
                sort_name: sort_name(candidate.name().as_str()),
                is_primary: true,
                at,
            })
            .await
            .map(Some)
            .map_err(SyncError::Storage),
        // A question, not an answer: a screen cannot draw a guess, so the candidate is left out
        // rather than merged into one of two titles.
        MatchDecision::Ambiguous { .. } => Ok(None),
    }
}

/// The candidate as the matching rule's input, or `None` when it cannot be one.
///
/// The port's `NonEmptyText` is already non-empty, so this only fails on a name that is whitespace —
/// which the adapter trims away before it gets here.
fn candidate_input(
    candidate: &peregrine_providers::provider::SearchCandidate,
) -> Option<Candidate> {
    Some(Candidate {
        external_id: candidate.external_id().clone(),
        name: NonEmptyText::new(candidate.name().as_str()).ok()?,
        kind: candidate.kind(),
        released: candidate.released(),
    })
}

/// Finds the provider's entry for a title the library scanned, attaches its id, and fills it in.
///
/// A scanned title carries no external id, so `refresh_title` has nothing to ask about and returns
/// `None`; this is the search that gives it one. The candidate must name the same title and be the
/// same kind, compared by the catalogue's own sort name, so a fuzzy provider result cannot attach
/// itself to a film nobody asked it about. An ambiguous answer stays unattached.
pub async fn match_title(
    provider: &dyn MetadataProvider,
    database: &Database,
    id: i64,
    kind: TitleKind,
    at: i64,
) -> Result<Option<RefreshedTitle>, SyncError> {
    let Some(identity) = database.title_identity_unrestricted(id).await? else {
        return Ok(None);
    };
    if identity.kind != kind {
        return Ok(None);
    }
    let Some(text) = NonEmptyText::new(identity.name.as_str()).ok() else {
        return Ok(None);
    };
    let Some(limit) = NonZeroU32::new(10) else {
        return Ok(None);
    };
    let candidates = provider
        .search(&SearchQuery::new(text, limit, Some(kind)))
        .await?;
    let wanted = sort_name(identity.name.as_str());
    let Some(candidate) = candidates.into_iter().find(|candidate| {
        candidate.kind() == kind && sort_name(candidate.name().as_str()) == wanted
    }) else {
        return Ok(None);
    };
    match database
        .attach_external_ids(id, &[held(candidate.external_id(), at)])
        .await
    {
        Ok(()) | Err(StorageError::Conflict { .. }) => {}
        Err(error) => return Err(SyncError::Storage(error)),
    }
    refresh_title(provider, database, id, kind, at).await
}

/// One external id, in the shape the storage layer attaches.
fn held(id: &peregrine_domain::ExternalId, at: i64) -> TitleExternalId {
    TitleExternalId {
        id: id.clone(),
        is_primary: true,
        fetched_at: at,
    }
}

/// Fetches one image into the blob cache and answers with where it is on disk.
///
/// The key is content-addressed over the provider, the path, and the variant, so the same poster is
/// fetched once however many times a grid redraws — `contains` is what avoids the second *request*, and
/// `store` after it is the write that a first fetch needs.
///
/// # Errors
///
/// [`CacheError::Provider`] when the provider cannot be asked, and [`CacheError::Write`] when the bytes
/// cannot be written.
pub async fn cache_image(
    provider: &dyn MetadataProvider,
    cache: &ImageCache,
    path: &str,
    kind: peregrine_domain::catalogue::ImageKind,
) -> Result<std::path::PathBuf, CacheError> {
    let key = ImageKey::for_image(
        provider.descriptor().provider(),
        path,
        ImageVariant::for_kind(kind),
    );
    if cache.contains(&key) {
        return Ok(cache.disk_path(&key));
    }
    let bytes = provider.image(path, kind).await?;
    let stored = cache.store(&key, &bytes).map_err(CacheError::Write)?;
    Ok(stored.disk_path().to_path_buf())
}

/// Why an image could not be cached.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    /// The provider could not be asked.
    #[error("the image could not be fetched")]
    Provider(#[from] ProviderError),
    /// The bytes could not be written.
    #[error("the image could not be stored")]
    Write(#[source] ImageCacheError),
}
