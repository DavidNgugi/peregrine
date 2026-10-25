//! The skip-marker cache and the local overrides a person can set against it.
//!
//! **Markers are keyed by title or episode, never by file.** The same episode has the same intro
//! in every copy, and keying by file would re-fetch the same answer for each quality. The source
//! is part of the key because two sources can disagree, and the resolver decides which one wins.
//!
//! **A negative answer is cached too.** `marker_fetch` records "the provider answered, and it had
//! nothing", which is different from "nobody has asked"; without it every play of a film with no
//! markers would spend a request to learn the same nothing.
//!
//! A suppression is local and permanent until the person clears it. It is never sent to a provider:
//! the point is to correct one's own copy without editing a community database (MKR-05).

use peregrine_domain::{MarkerConfidence, MarkerKind, MarkerSource, PlaybackMarker};
use sqlx::{Connection as _, SqliteConnection, SqlitePool};

use crate::error::StorageError;

/// What one provider lookup recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerFetchOutcome {
    /// The provider answered with at least one marker.
    Found,
    /// The provider answered and had nothing for this subject.
    None,
    /// The request failed; the row records the stable code and a short retry window.
    Failed,
}

impl MarkerFetchOutcome {
    /// The stored spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Found => "found",
            Self::None => "none",
            Self::Failed => "failed",
        }
    }

    /// Parses a stored spelling.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "found" => Some(Self::Found),
            "none" => Some(Self::None),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// The last lookup for one subject and source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkerFetch {
    /// What the provider answered.
    pub outcome: MarkerFetchOutcome,
    /// When it answered, in Unix milliseconds.
    pub fetched_at: i64,
    /// When asking again is worth it.
    pub expires_at: i64,
    /// The validator the provider sent, when it sent one.
    pub etag: Option<String>,
    /// The stable failure code, when the outcome is `Failed`.
    pub error_code: Option<String>,
}

impl MarkerFetch {
    /// Whether the answer is still inside its window at `now_ms`.
    #[must_use]
    pub const fn is_fresh(&self, now_ms: i64) -> bool {
        now_ms < self.expires_at
    }
}

/// One marker row as stored, with the local override flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoredMarker {
    /// The row id, which a suppression names.
    pub id: i64,
    /// The marker itself.
    pub marker: PlaybackMarker,
    /// Whether a person has hidden this marker on their own copy.
    pub suppressed: bool,
}

/// Reads every marker for a title or episode, suppressed ones included.
///
/// The caller decides whether to hide the suppressed rows: the player must not draw them, while a
/// settings surface that lets a person undo the suppression has to see them.
pub(crate) async fn read(
    pool: &SqlitePool,
    title_id: i64,
    episode_id: Option<i64>,
) -> Result<Vec<StoredMarker>, StorageError> {
    let rows = sqlx::query_as::<_, (i64, String, i64, Option<i64>, String, i64, i64)>(
        "SELECT id, kind, start_ms, end_ms, source, confidence, suppressed \
         FROM playback_marker WHERE title_id = ?1 AND episode_id IS ?2 \
         ORDER BY start_ms ASC, id ASC",
    )
    .bind(title_id)
    .bind(episode_id)
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;

    rows.into_iter()
        .map(
            |(id, kind, start_ms, end_ms, source, confidence, suppressed)| {
                let kind =
                    MarkerKind::parse(&kind).ok_or_else(|| StorageError::UnexpectedValue {
                        column: "playback_marker.kind",
                        found: kind.clone(),
                    })?;
                let source =
                    MarkerSource::parse(&source).ok_or_else(|| StorageError::UnexpectedValue {
                        column: "playback_marker.source",
                        found: source.clone(),
                    })?;
                let confidence =
                    u8::try_from(confidence).map_err(|_| StorageError::UnexpectedValue {
                        column: "playback_marker.confidence",
                        found: confidence.to_string(),
                    })?;
                let confidence = MarkerConfidence::new(confidence).map_err(|_| {
                    StorageError::UnexpectedValue {
                        column: "playback_marker.confidence",
                        found: confidence.to_string(),
                    }
                })?;
                let marker = PlaybackMarker::new(kind, start_ms, end_ms, source, confidence)
                    .map_err(|_| StorageError::UnexpectedValue {
                        column: "playback_marker.window",
                        found: format!("{start_ms}..{end_ms:?}"),
                    })?;
                Ok(StoredMarker {
                    id,
                    marker,
                    suppressed: suppressed != 0,
                })
            },
        )
        .collect()
}

/// Replaces one source's markers for a subject, preserving local suppressions.
///
/// The delete is restricted to unsuppressed rows, so a re-fetch cannot erase a person's override.
/// The insert is `OR IGNORE` because the suppressed row still owns the natural key; the suppressed
/// interval therefore stays hidden instead of reappearing under the same values.
pub(crate) async fn replace(
    connection: &mut SqliteConnection,
    title_id: i64,
    episode_id: Option<i64>,
    source: MarkerSource,
    markers: &[PlaybackMarker],
    at: i64,
) -> Result<(), StorageError> {
    let mut transaction = connection.begin().await.map_err(StorageError::from_sqlx)?;
    sqlx::query(
        "DELETE FROM playback_marker WHERE title_id = ?1 AND episode_id IS ?2 \
         AND source = ?3 AND suppressed = 0",
    )
    .bind(title_id)
    .bind(episode_id)
    .bind(source.as_str())
    .execute(&mut *transaction)
    .await
    .map_err(StorageError::from_sqlx)?;

    for marker in markers {
        sqlx::query(
            "INSERT OR IGNORE INTO playback_marker \
             (title_id, episode_id, kind, start_ms, end_ms, source, confidence, fetched_at, suppressed) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0)",
        )
        .bind(title_id)
        .bind(episode_id)
        .bind(marker.kind().as_str())
        .bind(marker.start_ms())
        .bind(marker.end_ms())
        .bind(marker.source().as_str())
        .bind(i64::from(marker.confidence().percent()))
        .bind(at)
        .execute(&mut *transaction)
        .await
        .map_err(StorageError::from_sqlx)?;
    }

    transaction.commit().await.map_err(StorageError::from_sqlx)
}

/// Reads the last lookup for one subject and source.
pub(crate) async fn fetch(
    pool: &SqlitePool,
    title_id: i64,
    episode_id: Option<i64>,
    source: MarkerSource,
) -> Result<Option<MarkerFetch>, StorageError> {
    let row = sqlx::query_as::<_, (String, i64, i64, Option<String>, Option<String>)>(
        "SELECT outcome, fetched_at, expires_at, etag, error_code FROM marker_fetch \
         WHERE title_id = ?1 AND episode_id = ?2 AND source = ?3",
    )
    .bind(title_id)
    .bind(episode_id.unwrap_or(0))
    .bind(source.as_str())
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;

    row.map(|(outcome, fetched_at, expires_at, etag, error_code)| {
        let outcome =
            MarkerFetchOutcome::parse(&outcome).ok_or_else(|| StorageError::UnexpectedValue {
                column: "marker_fetch.outcome",
                found: outcome.clone(),
            })?;
        Ok(MarkerFetch {
            outcome,
            fetched_at,
            expires_at,
            etag,
            error_code,
        })
    })
    .transpose()
}

/// Records one lookup, replacing the previous one for the same subject and source.
#[allow(clippy::too_many_arguments)] // One row, whose columns are the cache policy.
pub(crate) async fn record_fetch(
    connection: &mut SqliteConnection,
    title_id: i64,
    episode_id: Option<i64>,
    source: MarkerSource,
    outcome: MarkerFetchOutcome,
    error_code: Option<&str>,
    etag: Option<&str>,
    fetched_at: i64,
    expires_at: i64,
) -> Result<(), StorageError> {
    sqlx::query(
        "INSERT INTO marker_fetch \
         (title_id, episode_id, source, fetched_at, expires_at, etag, outcome, error_code) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT (title_id, episode_id, source) DO UPDATE SET \
             fetched_at = excluded.fetched_at, expires_at = excluded.expires_at, \
             etag = excluded.etag, outcome = excluded.outcome, error_code = excluded.error_code",
    )
    .bind(title_id)
    .bind(episode_id.unwrap_or(0))
    .bind(source.as_str())
    .bind(fetched_at)
    .bind(expires_at)
    .bind(etag)
    .bind(outcome.as_str())
    .bind(error_code)
    .execute(connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(())
}

/// Hides or restores one marker on this installation.
pub(crate) async fn set_suppressed(
    connection: &mut SqliteConnection,
    id: i64,
    suppressed: bool,
) -> Result<bool, StorageError> {
    let result = sqlx::query("UPDATE playback_marker SET suppressed = ?2 WHERE id = ?1")
        .bind(id)
        .bind(i64::from(suppressed))
        .execute(connection)
        .await
        .map_err(StorageError::from_sqlx)?;
    Ok(result.rows_affected() > 0)
}

/// One title's or episode's identity as a marker provider needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkerSubject {
    /// Film or series, so an anime-only source can be asked only about a series.
    pub kind: peregrine_domain::TitleKind,
    /// Standard, anime, documentary, or reality.
    pub subtype: peregrine_domain::TitleSubtype,
    /// The season number, when the subject is an episode.
    pub season: Option<u32>,
    /// The episode number, when the subject is an episode.
    pub episode: Option<u32>,
    /// The external ids the title carries.
    pub external_ids: Vec<peregrine_domain::ExternalId>,
}

impl MarkerSubject {
    /// The value one provider id holds, when the title carries it.
    #[must_use]
    pub fn external_id(&self, provider: peregrine_domain::Provider) -> Option<&str> {
        self.external_ids
            .iter()
            .find(|id| id.provider() == provider)
            .map(peregrine_domain::ExternalId::value)
    }

    /// Whether this is anime, which is the only case `AniSkip` may be asked about.
    #[must_use]
    pub fn is_anime(&self) -> bool {
        self.subtype == peregrine_domain::TitleSubtype::Anime
    }
}

/// Reads the identity a marker provider needs, or `None` when the scope may not see the title.
pub(crate) async fn subject(
    pool: &SqlitePool,
    title_id: i64,
    episode_id: Option<i64>,
) -> Result<Option<MarkerSubject>, StorageError> {
    let Some((kind, subtype, season, episode)) =
        sqlx::query_as::<_, (String, String, Option<i64>, Option<i64>)>(
            "SELECT t.kind, t.subtype, e.season_number, e.episode_number \
         FROM title t LEFT JOIN episode e ON e.id = ?2 WHERE t.id = ?1",
        )
        .bind(title_id)
        .bind(episode_id)
        .fetch_optional(pool)
        .await
        .map_err(StorageError::from_sqlx)?
    else {
        return Ok(None);
    };

    let kind =
        peregrine_domain::TitleKind::parse(&kind).ok_or_else(|| StorageError::UnexpectedValue {
            column: "title.kind",
            found: kind.clone(),
        })?;
    let subtype = peregrine_domain::TitleSubtype::parse(&subtype).ok_or_else(|| {
        StorageError::UnexpectedValue {
            column: "title.subtype",
            found: subtype.clone(),
        }
    })?;
    let external_ids = crate::catalogue::external_ids(pool, title_id)
        .await?
        .into_iter()
        .map(|held| held.id)
        .collect();
    Ok(Some(MarkerSubject {
        kind,
        subtype,
        season: season.and_then(|value| u32::try_from(value).ok()),
        episode: episode.and_then(|value| u32::try_from(value).ok()),
        external_ids,
    }))
}

#[cfg(test)]
mod tests {
    use peregrine_domain::{MarkerKind, PlaybackMarker};

    use super::*;

    fn intro(start: i64, end: i64) -> PlaybackMarker {
        PlaybackMarker::new(
            MarkerKind::Intro,
            start,
            Some(end),
            MarkerSource::IntroDb,
            MarkerConfidence::COMMUNITY,
        )
        .unwrap()
    }

    async fn with_film(
        directory: &tempfile::TempDir,
    ) -> (crate::database::Database, std::path::PathBuf) {
        let database = crate::test_support::database(directory).await;
        let path = crate::test_support::path_of(directory);
        crate::test_support::title(&path, 1, "A Film", 0, Some("PG-13")).await;
        (database, path)
    }

    #[tokio::test]
    async fn an_unfetched_subject_has_no_markers_and_no_fetch_row() {
        let directory = tempfile::TempDir::new().unwrap();
        let (database, _) = with_film(&directory).await;

        assert!(database.markers_for(1, None).await.unwrap().is_empty());
        assert!(
            database
                .marker_fetch(1, None, MarkerSource::IntroDb)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn markers_round_trip_with_their_source_and_confidence() {
        let directory = tempfile::TempDir::new().unwrap();
        let (database, _) = with_film(&directory).await;
        let markers = vec![intro(30_000, 90_000)];

        database
            .replace_markers(1, None, MarkerSource::IntroDb, &markers, 1)
            .await
            .unwrap();

        let stored = database.markers_for(1, None).await.unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].marker, markers[0]);
        assert!(!stored[0].suppressed);
    }

    #[tokio::test]
    async fn a_re_fetch_replaces_unsuppressed_rows_and_keeps_overrides() {
        let directory = tempfile::TempDir::new().unwrap();
        let (database, _) = with_film(&directory).await;
        database
            .replace_markers(1, None, MarkerSource::IntroDb, &[intro(30_000, 90_000)], 1)
            .await
            .unwrap();
        let stored = database.markers_for(1, None).await.unwrap();
        database
            .set_marker_suppressed(stored[0].id, true)
            .await
            .unwrap();
        database
            .replace_markers(1, None, MarkerSource::IntroDb, &[intro(31_000, 91_000)], 2)
            .await
            .unwrap();

        let stored = database.markers_for(1, None).await.unwrap();
        assert_eq!(stored.len(), 2, "the suppressed row is retained");
        assert_eq!(
            stored.iter().filter(|row| row.suppressed).count(),
            1,
            "the override survives the re-fetch"
        );
        assert!(
            stored.iter().any(|row| row.marker.start_ms() == 31_000),
            "the new interval was written"
        );
    }

    #[tokio::test]
    async fn a_fetch_record_round_trips_and_expires() {
        let directory = tempfile::TempDir::new().unwrap();
        let (database, _) = with_film(&directory).await;
        database
            .record_marker_fetch(
                1,
                None,
                MarkerSource::IntroDb,
                MarkerFetchOutcome::None,
                None,
                Some("\"abc\""),
                100,
                200,
            )
            .await
            .unwrap();

        let fetch = database
            .marker_fetch(1, None, MarkerSource::IntroDb)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetch.outcome, MarkerFetchOutcome::None);
        assert_eq!(fetch.etag.as_deref(), Some("\"abc\""));
        assert!(fetch.is_fresh(199));
        assert!(!fetch.is_fresh(200));
    }

    #[tokio::test]
    async fn the_subject_carries_the_external_ids_and_anime_flag() {
        let directory = tempfile::TempDir::new().unwrap();
        let database = crate::test_support::database(&directory).await;
        let path = crate::test_support::path_of(&directory);
        crate::test_support::execute(
            &path,
            "INSERT INTO title (id, kind, subtype, name, sort_name, content_rating, adult, \
             metadata_state, created_at, updated_at) \
             VALUES (1, 'show', 'anime', 'A Show', 'A Show', 'TV-14', 0, 'complete', 0, 0)",
        )
        .await;
        crate::test_support::execute(
            &path,
            "INSERT INTO external_id (title_id, provider, value, fetched_at) \
             VALUES (1, 'mal', '21', 0)",
        )
        .await;

        let subject = database.marker_subject(1, None).await.unwrap().unwrap();
        assert!(subject.is_anime());
        assert_eq!(
            subject.external_id(peregrine_domain::Provider::Mal),
            Some("21")
        );
    }

    #[tokio::test]
    async fn an_unknown_title_has_no_subject() {
        let directory = tempfile::TempDir::new().unwrap();
        let database = crate::test_support::database(&directory).await;

        assert!(
            database
                .marker_subject(9_999, None)
                .await
                .unwrap()
                .is_none()
        );
    }
}
