//! Resolving skip markers for a playback session.
//!
//! **The browser never calls a provider.** A marker request is this process's, the cache is the
//! database's, and a play answers from what is already stored (AGENTS R7, MKR-01). A refresh runs
//! after the session opens, so the first frame never waits for a network round trip.
//!
//! The order is the one the plan states: local chapters, then the enabled network sources by
//! priority. The domain's [`peregrine_domain::resolve_markers`] merges the answers per kind, and
//! a suppressed row is filtered here before it can reach the screen.
//!
//! **A failed refresh is not a failed play.** Every provider failure is recorded against the
//! integration and logged without a URL; the player draws whatever the cache already holds.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use peregrine_domain::integration::IntegrationId;
use peregrine_domain::{
    Clock, IntegrationOutcome, MarkerKind, MarkerSource, PlaybackMarker, Provider, TitleKind,
    resolve_markers,
};
use peregrine_providers::{AniSkip, IntroDb, MarkerError, MarkerProvider, MarkerRequest};
use peregrine_secrets::{SecretKey, SecretStore};
use peregrine_storage::{Database, MarkerFetchOutcome, StoredMarker};
use tracing::{debug, warn};

use crate::integrations::IntegrationRegistry;
use crate::runtime::RuntimeError;

/// How long a positive answer is reused.
const FOUND_TTL_MS: i64 = 24 * 60 * 60 * 1_000;

/// How long "the provider has nothing" is reused.
///
/// Shorter than a positive answer: a community database is filled in over time, and a title that
/// had no marker yesterday may have one today.
const NONE_TTL_MS: i64 = 6 * 60 * 60 * 1_000;

/// How long a failure is reused before the next play tries again.
const FAILED_TTL_MS: i64 = 10 * 60 * 1_000;

/// The longest one provider may take before the refresh gives up on it.
const FETCH_TIMEOUT: Duration = Duration::from_secs(6);

/// One refresh's own clock and collaborators.
pub struct MarkerService {
    database: Arc<Database>,
    integrations: Arc<IntegrationRegistry>,
    secrets: Arc<dyn SecretStore>,
    clock: Arc<dyn Clock + Send + Sync>,
}

impl MarkerService {
    /// Builds the service from the runtime's handles.
    #[must_use]
    pub fn new(
        database: Arc<Database>,
        integrations: Arc<IntegrationRegistry>,
        secrets: Arc<dyn SecretStore>,
        clock: Arc<dyn Clock + Send + Sync>,
    ) -> Self {
        Self {
            database,
            integrations,
            secrets,
            clock,
        }
    }

    /// The markers a player should draw right now, from the cache only.
    ///
    /// Suppressed rows are removed here, so no screen has to remember the rule.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Storage`] when the cache cannot be read.
    pub async fn cached(
        &self,
        title_id: i64,
        episode_id: Option<i64>,
        duration_ms: Option<i64>,
    ) -> Result<Vec<PlaybackMarker>, RuntimeError> {
        let rows = self.database.markers_for(title_id, episode_id).await?;
        Ok(resolve_rows(rows, duration_ms))
    }

    /// Refreshes the cache from chapters and every enabled network source, then answers the cache.
    ///
    /// A source whose last answer is still fresh is not asked again. A failure is recorded and the
    /// refresh continues: one unavailable database must not hide another source's marker.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::UnknownTitle`] when the title is outside the installation scope, and
    /// [`RuntimeError::Storage`] when the cache cannot be written or read.
    #[allow(clippy::too_many_lines)] // One refresh: chapters, every enabled source, then the cache.
    pub async fn refresh(
        &self,
        title_id: i64,
        episode_id: Option<i64>,
        duration_ms: Option<i64>,
        file: Option<&Path>,
        ffprobe: Option<&Path>,
    ) -> Result<Vec<PlaybackMarker>, RuntimeError> {
        let Some(subject) = self.database.marker_subject(title_id, episode_id).await? else {
            return Err(RuntimeError::UnknownTitle);
        };
        let now = self.clock.now().as_millisecond();

        if let (Some(file), Some(ffprobe)) = (file, ffprobe)
            && self
                .fresh(title_id, episode_id, MarkerSource::Chapters, now)
                .await?
                .is_none()
        {
            match peregrine_playback::chapters::extract(ffprobe, file).await {
                Ok(chapters) => {
                    let markers = peregrine_playback::chapters::markers(&chapters);
                    self.record(
                        title_id,
                        episode_id,
                        MarkerSource::Chapters,
                        &markers,
                        Now {
                            at: now,
                            outcome: if markers.is_empty() {
                                MarkerFetchOutcome::None
                            } else {
                                MarkerFetchOutcome::Found
                            },
                            error_code: None,
                        },
                    )
                    .await?;
                }
                Err(error) => {
                    debug!(%error, "the chapter probe answered nothing for this file");
                    self.record_failure(
                        title_id,
                        episode_id,
                        MarkerSource::Chapters,
                        "CHAPTERS_UNREADABLE",
                        now,
                    )
                    .await?;
                }
            }
        }

        let request = MarkerRequest {
            kind: subject.kind,
            season: subject.season,
            episode: subject.episode,
            tvdb_id: subject
                .external_id(Provider::Tvdb)
                .and_then(|value| value.parse().ok()),
            tmdb_id: subject
                .external_id(Provider::Tmdb)
                .and_then(|value| value.parse().ok()),
            imdb_id: subject.external_id(Provider::Imdb).map(str::to_owned),
            mal_id: subject
                .external_id(Provider::Mal)
                .and_then(|value| value.parse().ok()),
            duration_ms,
            is_anime: subject.is_anime(),
        };

        let enabled: Vec<&'static crate::integrations::IntegrationManifest> = self
            .integrations
            .enabled()
            .into_iter()
            .filter(|entry| entry.category == peregrine_domain::IntegrationCategory::SkipMarkers)
            .filter(|entry| entry.id != "chapters")
            .collect();

        for entry in enabled {
            let source = source_of(entry.id);
            let Some(source) = source else {
                continue;
            };
            if !applicable(entry.id, &request) {
                continue;
            }
            if self
                .fresh(title_id, episode_id, source, now)
                .await?
                .is_some()
            {
                continue;
            }
            match self.fetch(entry.id, &request).await {
                Ok(markers) => {
                    self.record(
                        title_id,
                        episode_id,
                        source,
                        &markers,
                        Now {
                            at: now,
                            outcome: if markers.is_empty() {
                                MarkerFetchOutcome::None
                            } else {
                                MarkerFetchOutcome::Found
                            },
                            error_code: None,
                        },
                    )
                    .await?;
                    self.outcome(entry.id, IntegrationOutcome::Working, None, now)
                        .await;
                }
                Err(MarkerError::NotFound { .. }) => {
                    // **"No submissions" is an answer, not a failure.** TheIntroDB returns a 404
                    // with `no_submissions` for a title nobody has contributed to, and AniSkip
                    // does the same for an episode it has nothing on. Caching it as a failure
                    // retried every ten minutes and made a normal empty answer look like a
                    // broken source; it is an empty answer with the normal negative TTL.
                    self.record(
                        title_id,
                        episode_id,
                        source,
                        &[],
                        Now {
                            at: now,
                            outcome: MarkerFetchOutcome::None,
                            error_code: None,
                        },
                    )
                    .await?;
                    self.outcome(
                        entry.id,
                        IntegrationOutcome::Working,
                        Some("MARKER_NONE"),
                        now,
                    )
                    .await;
                    debug!(
                        source = entry.id,
                        "a marker provider has no markers for this subject"
                    );
                }
                Err(error) => {
                    let (outcome, code) = classify(&error);
                    self.record_failure(title_id, episode_id, source, code, now)
                        .await?;
                    self.outcome(entry.id, outcome, Some(code), now).await;
                    warn!(
                        source = entry.id,
                        code, "a marker provider did not answer; the cached markers stay"
                    );
                }
            }
        }

        self.cached(title_id, episode_id, duration_ms).await
    }

    /// Whether a source's cached answer is still inside its window; `None` means ask it.
    async fn fresh(
        &self,
        title_id: i64,
        episode_id: Option<i64>,
        source: MarkerSource,
        now: i64,
    ) -> Result<Option<()>, RuntimeError> {
        Ok(self
            .database
            .marker_fetch(title_id, episode_id, source)
            .await?
            .filter(|fetch| fetch.is_fresh(now))
            .map(|_| ()))
    }

    /// Asks one enabled network source.
    async fn fetch(
        &self,
        id: &'static str,
        request: &MarkerRequest,
    ) -> Result<Vec<PlaybackMarker>, MarkerError> {
        match id {
            "theintrodb" => {
                let key = self.credential("theintrodb", "apiKey").await;
                let adapter = IntroDb::new(IntroDb::BASE_URL, key)?;
                timeout(adapter.fetch(request.clone()), MarkerSource::IntroDb).await
            }
            "aniskip" => {
                let adapter = AniSkip::new(AniSkip::BASE_URL)?;
                timeout(adapter.fetch(request.clone()), MarkerSource::AniSkip).await
            }
            _ => Ok(Vec::new()),
        }
    }

    /// Reads one optional integration field, answering `None` when it is absent or refused.
    async fn credential(&self, integration: &'static str, field: &'static str) -> Option<String> {
        let store = Arc::clone(&self.secrets);
        let key = SecretKey::integration(integration, field);
        match tokio::task::spawn_blocking(move || store.get(&key)).await {
            Ok(Ok(Some(secret))) => Some(secret.expose_secret().to_owned()),
            Ok(Ok(None) | Err(_)) | Err(_) => None,
        }
    }

    /// Writes one source's answer and its cache stamp.
    async fn record(
        &self,
        title_id: i64,
        episode_id: Option<i64>,
        source: MarkerSource,
        markers: &[PlaybackMarker],
        now: Now,
    ) -> Result<(), RuntimeError> {
        self.database
            .replace_markers(title_id, episode_id, source, markers, now.at)
            .await?;
        let ttl = if now.outcome == MarkerFetchOutcome::Found {
            FOUND_TTL_MS
        } else {
            NONE_TTL_MS
        };
        self.database
            .record_marker_fetch(
                title_id,
                episode_id,
                source,
                now.outcome,
                now.error_code,
                None,
                now.at,
                now.at + ttl,
            )
            .await?;
        Ok(())
    }

    /// Records a failed lookup with a short retry window.
    async fn record_failure(
        &self,
        title_id: i64,
        episode_id: Option<i64>,
        source: MarkerSource,
        code: &'static str,
        now: i64,
    ) -> Result<(), RuntimeError> {
        self.database
            .record_marker_fetch(
                title_id,
                episode_id,
                source,
                MarkerFetchOutcome::Failed,
                Some(code),
                None,
                now,
                now + FAILED_TTL_MS,
            )
            .await?;
        Ok(())
    }

    /// Records an integration health outcome, best-effort.
    async fn outcome(
        &self,
        id: &'static str,
        outcome: IntegrationOutcome,
        code: Option<&'static str>,
        at: i64,
    ) {
        let Ok(id) = IntegrationId::new(id) else {
            return;
        };
        if let Err(error) = self
            .database
            .record_integration_outcome(&id, outcome, code, at)
            .await
        {
            debug!(%error, "the integration health row could not be written");
        }
        self.integrations.remember_outcome(&id, outcome, code, at);
    }
}

/// One write's stamp and outcome.
struct Now {
    at: i64,
    outcome: MarkerFetchOutcome,
    error_code: Option<&'static str>,
}

/// Runs one provider future under the refresh's bounded wait.
async fn timeout(
    future: peregrine_providers::MarkerFuture<'_, Vec<PlaybackMarker>>,
    source: MarkerSource,
) -> Result<Vec<PlaybackMarker>, MarkerError> {
    tokio::time::timeout(FETCH_TIMEOUT, future)
        .await
        .map_err(|_| MarkerError::Transport {
            marker_source: source,
            kind: peregrine_providers::TransportFailure::Timeout,
        })?
}

/// The source one integration id answers for.
fn source_of(id: &str) -> Option<MarkerSource> {
    match id {
        "theintrodb" => Some(MarkerSource::IntroDb),
        "aniskip" => Some(MarkerSource::AniSkip),
        "introhater" => Some(MarkerSource::IntroHater),
        _ => None,
    }
}

/// Whether a source has the identity or subject it needs.
fn applicable(id: &str, request: &MarkerRequest) -> bool {
    match id {
        "theintrodb" => {
            request.tvdb_id.is_some() || request.tmdb_id.is_some() || request.imdb_id.is_some()
        }
        "aniskip" => {
            request.is_anime
                && request.kind == TitleKind::Show
                && request.mal_id.is_some()
                && request.episode.is_some()
        }
        _ => false,
    }
}

/// The health outcome and stable code one failure maps onto.
fn classify(error: &MarkerError) -> (IntegrationOutcome, &'static str) {
    match error {
        MarkerError::Unauthorized { .. } => {
            (IntegrationOutcome::Unauthorized, "MARKER_UNAUTHORIZED")
        }
        MarkerError::RateLimited { .. } => (IntegrationOutcome::RateLimited, "MARKER_RATE_LIMITED"),
        MarkerError::Transport { .. } => (IntegrationOutcome::Unreachable, "MARKER_UNREACHABLE"),
        MarkerError::TooLarge { .. } | MarkerError::SchemaDrift { .. } => {
            (IntegrationOutcome::Unreachable, "MARKER_BAD_ANSWER")
        }
        MarkerError::Unsupported { .. } | MarkerError::NotFound { .. } => {
            (IntegrationOutcome::Working, "MARKER_NONE")
        }
    }
}

/// The domain's resolution over stored rows, with suppressions removed first.
fn resolve_rows(rows: Vec<StoredMarker>, duration_ms: Option<i64>) -> Vec<PlaybackMarker> {
    resolve_markers(
        rows.into_iter()
            .filter(|row| !row.suppressed)
            .map(|row| row.marker),
        duration_ms,
    )
}

/// A marker kind's name, for a log line.
#[must_use]
pub fn kind_name(kind: MarkerKind) -> &'static str {
    kind.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_two_shipped_sources_map_to_a_marker_source() {
        assert_eq!(source_of("theintrodb"), Some(MarkerSource::IntroDb));
        assert_eq!(source_of("aniskip"), Some(MarkerSource::AniSkip));
        assert_eq!(source_of("chapters"), None);
        assert_eq!(source_of("unknown"), None);
    }

    #[test]
    fn aniskip_requires_an_anime_episode_with_a_mal_id() {
        let base = MarkerRequest {
            kind: TitleKind::Show,
            season: Some(1),
            episode: Some(1),
            tvdb_id: None,
            tmdb_id: None,
            imdb_id: None,
            mal_id: Some(21),
            duration_ms: Some(1_440_000),
            is_anime: true,
        };
        assert!(applicable("aniskip", &base));
        assert!(!applicable(
            "aniskip",
            &MarkerRequest {
                is_anime: false,
                ..base.clone()
            }
        ));
        assert!(!applicable(
            "aniskip",
            &MarkerRequest {
                mal_id: None,
                ..base.clone()
            }
        ));
        assert!(!applicable(
            "aniskip",
            &MarkerRequest {
                kind: TitleKind::Movie,
                ..base
            }
        ));
    }

    #[test]
    fn theintrodb_needs_one_identity_and_accepts_any_of_the_three() {
        let mut request = MarkerRequest {
            kind: TitleKind::Movie,
            season: None,
            episode: None,
            tvdb_id: Some(169),
            tmdb_id: None,
            imdb_id: None,
            mal_id: None,
            duration_ms: None,
            is_anime: false,
        };
        assert!(applicable("theintrodb", &request));
        request.tvdb_id = None;
        request.imdb_id = Some("tt0111161".to_owned());
        assert!(applicable("theintrodb", &request));
        request.imdb_id = None;
        assert!(!applicable("theintrodb", &request));
    }

    #[test]
    fn a_suppressed_marker_never_reaches_the_player() {
        let marker = PlaybackMarker::chapter(MarkerKind::Intro, 30_000, 90_000).unwrap();
        let rows = vec![
            StoredMarker {
                id: 1,
                marker,
                suppressed: true,
            },
            StoredMarker {
                id: 2,
                marker: PlaybackMarker::chapter(MarkerKind::Credits, 1_300_000, 1_440_000).unwrap(),
                suppressed: false,
            },
        ];

        let resolved = resolve_rows(rows, Some(1_440_000));
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].kind(), MarkerKind::Credits);
    }

    #[test]
    fn a_failure_maps_to_one_health_outcome_and_code() {
        let unauthorized = MarkerError::Unauthorized {
            marker_source: MarkerSource::IntroDb,
        };
        assert_eq!(
            classify(&unauthorized),
            (IntegrationOutcome::Unauthorized, "MARKER_UNAUTHORIZED")
        );
        let none = MarkerError::NotFound {
            marker_source: MarkerSource::AniSkip,
        };
        assert_eq!(
            classify(&none),
            (IntegrationOutcome::Working, "MARKER_NONE")
        );
    }
}
