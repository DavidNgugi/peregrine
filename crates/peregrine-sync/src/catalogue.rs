//! Syncing a provider's answers into the catalogue.
//!
//! The last mile of FLOWS F3: ask a provider, decide which answers this catalogue already holds
//! (`peregrine_domain::matching`), and write the rest. What the sync does *not* do is guess — an answer
//! the rule calls ambiguous is counted and left alone, because the wrong answer there merges two titles
//! or splits one.

use peregrine_domain::matching::{Candidate, MatchDecision, match_candidate, sort_name};
use peregrine_domain::text::NonEmptyText;
use peregrine_providers::error::ProviderError;
use peregrine_providers::provider::{MetadataProvider, SearchQuery};
use peregrine_storage::{Database, ProviderTitle, StorageError, TitleExternalId};

use crate::images::{ImageCache, ImageCacheError, ImageKey, ImageVariant};

/// What one sync did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    /// Candidates that named a title the catalogue already had.
    pub matched: usize,
    /// Candidates that became a new title.
    pub created: usize,
    /// Candidates the rule could not attribute, left for a person.
    pub ambiguous: usize,
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

/// Fills a title the catalogue holds from the provider that knows it.
///
/// Answers `false` when this provider cannot serve title details at all, which is a normal state rather
/// than a failure: a provider that declares only `search` is a provider this step skips. Any other
/// error is reported, because a refresh that silently did nothing looks exactly like one that worked.
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
) -> Result<bool, SyncError> {
    // The id of *this* provider: a title may carry several, and asking the wrong provider for the wrong
    // one's identifier is how a film gets another film's synopsis.
    let external = database
        .external_ids_of(id)
        .await?
        .into_iter()
        .map(|held| held.id)
        .find(|external| external.provider() == provider.descriptor().provider());
    let Some(external) = external else {
        return Ok(false);
    };

    let details = match provider.title(&external, kind).await {
        Ok(details) => details,
        Err(ProviderError::Unsupported { .. }) => return Ok(false),
        Err(error) => return Err(SyncError::Provider(error)),
    };
    let sort = sort_name(details.name.as_str());
    let provider_name = provider.descriptor().provider().as_str().to_owned();
    database
        .update_title_metadata(
            id,
            details,
            sort,
            peregrine_domain::catalogue::MetadataState::Partial,
            provider_name,
            at,
        )
        .await?;
    Ok(true)
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
    let variant = match kind {
        peregrine_domain::catalogue::ImageKind::Poster => ImageVariant::PosterW342,
        peregrine_domain::catalogue::ImageKind::Backdrop => ImageVariant::BackdropW780,
    };
    let key = ImageKey::for_image(provider.descriptor().provider(), path, variant);
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
