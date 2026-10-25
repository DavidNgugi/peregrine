//! Playback markers: the intro, recap, credits, and preview windows a player can skip.
//!
//! A marker is a fact about a **title or one episode**, never about a file: the same episode
//! watched from a 1080p copy and a 4K copy has the same intro. The source is carried beside the
//! interval because a person is entitled to know who said it, and because local chapters outrank
//! every network source on trust.
//!
//! **A marker is never guessed.** A provider that cannot name the title returns nothing, the
//! resolution below drops anything below the confidence floor, and a title with no marker has no
//! skip control at all. A wrong skip is worse than no skip: it moves the film to a place the
//! person did not ask for.
//!
//! **Duration is a bound, not a source.** A provider marker that starts past the file's runtime is
//! a marker for another cut, and it is dropped rather than clamped to the end; one that ends past
//! the runtime is clamped, because a credits marker that opens before the end is still usable and
//! ffprobe's duration can be a second short. A marker whose endpoints collapse after clamping is
//! dropped for the same reason.
//!
//! The rules here are pure (AGENTS R7): storage and provider adapters supply the candidates, and
//! the player draws what this module resolved.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::stored_enum;

stored_enum! {
    /// What a marker describes.
    ///
    /// The four names mirror `playback_marker.kind`'s `CHECK`, and they are the four segment
    /// types `TheIntroDB` publishes.
    MarkerKind {
        /// The opening sequence.
        Intro => "intro",
        /// A previously-on sequence.
        Recap => "recap",
        /// The end credits and whatever follows them.
        Credits => "credits",
        /// A next-episode preview inside the credits.
        Preview => "preview",
    }
}

stored_enum! {
    /// Where a marker came from.
    ///
    /// **The order is the trust order** and [`MarkerSource::rank`] is the one place that says so:
    /// chapters are the file talking about itself, `TheIntroDB` is a moderated community database,
    /// `AniSkip` is the same for anime with an exact MAL identity, and `IntroHater` is optional and
    /// experimental.
    MarkerSource {
        /// Chapters in the file's own container.
        Chapters => "chapters",
        /// `TheIntroDB`'s community timestamps.
        IntroDb => "introdb",
        /// `AniSkip`'s anime timestamps.
        AniSkip => "aniskip",
        /// `IntroHater`'s community database.
        IntroHater => "introhater",
    }
}

impl MarkerSource {
    /// The trust order, lowest number first.
    ///
    /// Used to choose which source answers for one kind when several have an opinion. It is
    /// deliberately per-kind rather than per-subject: chapters can win `intro` while `AniSkip` wins
    /// `credits`, and a single winner for the whole episode would throw away a usable marker.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Chapters => 0,
            Self::IntroDb => 1,
            Self::AniSkip => 2,
            Self::IntroHater => 3,
        }
    }

    /// Whether this source is a network source, for the log line and the cache policy.
    #[must_use]
    pub const fn is_network(self) -> bool {
        !matches!(self, Self::Chapters)
    }
}

/// The confidence below which a network marker is not shown.
///
/// **A policy, not a provider's number.** `TheIntroDB` reports a weighted average per segment and
/// `AniSkip` does not report one at all (it is treated as [`MarkerConfidence::COMMUNITY`]); the floor
/// is what keeps a single unweighted submission from producing a skip button. Local chapters carry
/// [`MarkerConfidence::EXACT`] and are never filtered.
pub const CONFIDENCE_FLOOR: u8 = 40;

/// How far apart two intervals of the same kind and source may be before they are two markers.
///
/// A community database can hold the same intro twice with a second of disagreement; anything
/// closer than this is one window, and splitting it would draw two buttons for one sequence.
pub const MERGE_GAP_MS: i64 = 500;

/// A confidence in whole percent, 0 through 100.
///
/// A newtype rather than a bare `u8` because the range is an invariant every marker carries into
/// the database's `CHECK`, and [`PlaybackMarker::new`] is the only public constructor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MarkerConfidence(u8);

impl MarkerConfidence {
    /// The highest confidence: chapters, which are exact by construction.
    pub const EXACT: Self = Self(100);

    /// What a source that reports no confidence is trusted with.
    ///
    /// `AniSkip` publishes endpoints without a score; its community has voted the interval into the
    /// dataset, which is the same evidence `TheIntroDB` scores around the middle of its range. It is
    /// above [`CONFIDENCE_FLOOR`], because treating an unreported score as zero would discard a
    /// source that works.
    pub const COMMUNITY: Self = Self(60);

    /// Builds a confidence from a whole percent.
    ///
    /// # Errors
    ///
    /// [`MarkerError::ConfidenceOutOfRange`] above 100.
    pub const fn new(percent: u8) -> Result<Self, MarkerError> {
        if percent > 100 {
            return Err(MarkerError::ConfidenceOutOfRange { percent });
        }
        Ok(Self(percent))
    }

    /// Reads a provider's own score, which may be a fraction or a percentage.
    ///
    /// **`TheIntroDB`'s `details` score is documented as a weighted average without a stated
    /// scale**, and the two plausible scales are 0–1 and 0–100. A value at or below one is
    /// therefore read as a fraction, and anything larger is read as a percentage. A missing or
    /// nonsensical value becomes [`Self::COMMUNITY`] rather than zero: a source that answered
    /// with a segment has said something, and the floor is the right place to weigh it.
    #[must_use]
    pub fn from_provider(score: Option<f64>) -> Self {
        let Some(score) = score else {
            return Self::COMMUNITY;
        };
        if !score.is_finite() || score <= 0.0 {
            return Self::COMMUNITY;
        }
        let percent = if score <= 1.0 { score * 100.0 } else { score };
        // `clamp` first, then a checked conversion: a score of 1e300 is a provider bug and must not
        // panic a cast in debug.
        let rounded = percent.round().clamp(0.0, 100.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Self(rounded as u8)
    }

    /// The percentage.
    #[must_use]
    pub const fn percent(self) -> u8 {
        self.0
    }
}

impl Default for MarkerConfidence {
    fn default() -> Self {
        Self::COMMUNITY
    }
}

/// One resolved marker window.
///
/// Fields are private so [`Self::new`] is the only way in: a marker with a negative start, a
/// non-positive window, or a null end on a non-credits kind cannot exist. That is the same
/// invariant the database's `CHECK` constraints state, and the two are kept in step by
/// `peregrine-storage`'s schema test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "PlaybackMarkerDraft", into = "PlaybackMarkerDraft")]
pub struct PlaybackMarker {
    kind: MarkerKind,
    start_ms: i64,
    end_ms: Option<i64>,
    source: MarkerSource,
    confidence: MarkerConfidence,
}

/// The wire and storage shape, which validation turns into a [`PlaybackMarker`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlaybackMarkerDraft {
    kind: MarkerKind,
    start_ms: i64,
    end_ms: Option<i64>,
    source: MarkerSource,
    confidence: MarkerConfidence,
}

impl TryFrom<PlaybackMarkerDraft> for PlaybackMarker {
    type Error = MarkerError;

    fn try_from(draft: PlaybackMarkerDraft) -> Result<Self, Self::Error> {
        Self::new(
            draft.kind,
            draft.start_ms,
            draft.end_ms,
            draft.source,
            draft.confidence,
        )
    }
}

impl From<PlaybackMarker> for PlaybackMarkerDraft {
    fn from(marker: PlaybackMarker) -> Self {
        Self {
            kind: marker.kind,
            start_ms: marker.start_ms,
            end_ms: marker.end_ms,
            source: marker.source,
            confidence: marker.confidence,
        }
    }
}

impl PlaybackMarker {
    /// Builds one marker, refusing a shape the schema would refuse.
    ///
    /// # Errors
    ///
    /// [`MarkerError::NegativeStart`], [`MarkerError::NonPositiveWindow`], or
    /// [`MarkerError::OpenEndedKind`] when a kind other than credits has no end.
    pub fn new(
        kind: MarkerKind,
        start_ms: i64,
        end_ms: Option<i64>,
        source: MarkerSource,
        confidence: MarkerConfidence,
    ) -> Result<Self, MarkerError> {
        if start_ms < 0 {
            return Err(MarkerError::NegativeStart { start_ms });
        }
        match end_ms {
            Some(end) if end <= start_ms => {
                return Err(MarkerError::NonPositiveWindow {
                    start_ms,
                    end_ms: end,
                });
            }
            None if kind != MarkerKind::Credits => {
                return Err(MarkerError::OpenEndedKind { kind });
            }
            _ => {}
        }
        Ok(Self {
            kind,
            start_ms,
            end_ms,
            source,
            confidence,
        })
    }

    /// A chapter-derived marker, which is exact and always closed-ended.
    ///
    /// Chapters are containers' own timestamps and always have an end; a chapter that does not is
    /// not a chapter this build maps, so the caller has already dropped it.
    ///
    /// # Errors
    ///
    /// As [`Self::new`].
    pub fn chapter(kind: MarkerKind, start_ms: i64, end_ms: i64) -> Result<Self, MarkerError> {
        Self::new(
            kind,
            start_ms,
            Some(end_ms),
            MarkerSource::Chapters,
            MarkerConfidence::EXACT,
        )
    }

    /// What this marker describes.
    #[must_use]
    pub const fn kind(self) -> MarkerKind {
        self.kind
    }

    /// Where the window opens, in milliseconds from the start of the file.
    #[must_use]
    pub const fn start_ms(self) -> i64 {
        self.start_ms
    }

    /// Where it closes, or `None` for credits that run to the end of the media.
    #[must_use]
    pub const fn end_ms(self) -> Option<i64> {
        self.end_ms
    }

    /// Who said so.
    #[must_use]
    pub const fn source(self) -> MarkerSource {
        self.source
    }

    /// How much the source is trusted, in whole percent.
    #[must_use]
    pub const fn confidence(self) -> MarkerConfidence {
        self.confidence
    }

    /// The end the window really has, given the file's runtime.
    ///
    /// A null end is the media's end; an unknown runtime leaves it unknown. This is what the skip
    /// control uses to decide whether it is still inside the window.
    #[must_use]
    pub fn end_or(self, duration_ms: Option<i64>) -> Option<i64> {
        self.end_ms.or(duration_ms)
    }

    /// Whether `position_ms` is inside this window.
    ///
    /// A credits marker with no end runs to the end of the media, so every position after its
    /// start is inside it. A marker with an unknown duration and a null end is treated the same
    /// way: there is no later point at which this build knows it has ended.
    #[must_use]
    pub fn contains(self, position_ms: i64) -> bool {
        if position_ms < self.start_ms {
            return false;
        }
        match self.end_ms {
            Some(end) => position_ms < end,
            None => true,
        }
    }

    /// This marker bounded by the file's runtime, or `None` when the runtime leaves nothing.
    ///
    /// A start at or past the runtime belongs to another cut of the title, and is dropped rather
    /// than moved to the end. An end past the runtime is clamped, because a marker that opens
    /// inside the file is still usable. The credits kind keeps its null end: that is what "to the
    /// end of the media" means.
    #[must_use]
    pub fn bounded(self, duration_ms: i64) -> Option<Self> {
        if duration_ms <= 0 || self.start_ms >= duration_ms {
            return None;
        }
        let end = match self.end_ms {
            Some(end) if end >= duration_ms => Some(duration_ms),
            Some(end) => Some(end),
            None => None,
        };
        if end == Some(self.start_ms) {
            return None;
        }
        Some(Self {
            end_ms: end,
            ..self
        })
    }

    /// Whether this marker is worth showing on its own.
    #[must_use]
    pub fn clears_floor(self) -> bool {
        self.source == MarkerSource::Chapters || self.confidence.percent() >= CONFIDENCE_FLOOR
    }
}

/// Why a marker could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum MarkerError {
    /// A start before the beginning of the file.
    #[error("a marker cannot start at {start_ms} ms")]
    NegativeStart {
        /// The refused start.
        start_ms: i64,
    },
    /// An end that is not after the start.
    #[error("a marker from {start_ms} ms to {end_ms} ms has no window")]
    NonPositiveWindow {
        /// The refused start.
        start_ms: i64,
        /// The refused end.
        end_ms: i64,
    },
    /// A kind other than credits with no end.
    #[error("a {kind:?} marker must have an end")]
    OpenEndedKind {
        /// The kind that needed an end.
        kind: MarkerKind,
    },
    /// A confidence above 100.
    #[error("a confidence of {percent} is not a percentage")]
    ConfidenceOutOfRange {
        /// The refused percentage.
        percent: u8,
    },
}

/// One source's answer for a subject, ready to be stored or resolved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkerCandidate {
    /// The marker itself.
    pub marker: PlaybackMarker,
    /// The source's own confidence, when it reported one.
    pub provider_confidence: Option<f64>,
}

impl MarkerCandidate {
    /// A candidate whose confidence is already resolved.
    #[must_use]
    pub const fn new(marker: PlaybackMarker) -> Self {
        Self {
            marker,
            provider_confidence: None,
        }
    }
}

/// Resolves candidates into what a player should show.
///
/// Four rules, in this order:
///
/// 1. **A source's opinion is per kind.** The best-ranked source that has anything to say about
///    `intro` answers `intro`; a source with only `credits` does not suppress a lower-ranked
///    source's `intro`.
/// 2. **Confidence filters network markers.** Local chapters always clear the floor; a network
///    marker below [`CONFIDENCE_FLOOR`] is dropped before it can win a kind.
/// 3. **Intervals within one kind and source are merged** when they overlap or sit within
///    [`MERGE_GAP_MS`], because a community database can hold one sequence twice.
/// 4. **The runtime bounds every marker**, and the result is ordered by start.
///
/// It never invents an interval: an empty input is an empty output.
#[must_use]
pub fn resolve(
    candidates: impl IntoIterator<Item = PlaybackMarker>,
    duration_ms: Option<i64>,
) -> Vec<PlaybackMarker> {
    let mut accepted: Vec<PlaybackMarker> = candidates
        .into_iter()
        .filter(|marker| marker.clears_floor())
        .filter_map(|marker| match duration_ms {
            Some(duration) => marker.bounded(duration),
            None => Some(marker),
        })
        .collect();

    // The best source with an opinion for each kind. `rank` is the trust order and is total.
    let mut best: Option<[Option<MarkerSource>; 4]> = None;
    for marker in &accepted {
        let slot = kind_slot(marker.kind);
        let table = best.get_or_insert([None; 4]);
        let current = table[slot];
        if current.is_none_or(|source| marker.source.rank() < source.rank()) {
            table[slot] = Some(marker.source);
        }
    }
    let Some(best) = best else {
        return Vec::new();
    };

    accepted.retain(|marker| best[kind_slot(marker.kind)] == Some(marker.source));
    accepted.sort_by_key(|marker| {
        (
            kind_slot(marker.kind),
            marker.start_ms,
            marker.end_ms.unwrap_or(i64::MAX),
        )
    });

    let mut merged: Vec<PlaybackMarker> = Vec::with_capacity(accepted.len());
    for marker in accepted {
        match merged.last_mut() {
            Some(previous)
                if previous.kind == marker.kind
                    && previous.source == marker.source
                    && previous.end_ms.is_some()
                    && previous.end_ms.unwrap_or(i64::MAX) + MERGE_GAP_MS >= marker.start_ms =>
            {
                let end = match (previous.end_ms, marker.end_ms) {
                    (None, _) | (_, None) => None,
                    (Some(left), Some(right)) => Some(left.max(right)),
                };
                previous.end_ms = end;
            }
            _ => merged.push(marker),
        }
    }
    merged.sort_by_key(|marker| (marker.start_ms, kind_slot(marker.kind)));
    merged
}

/// The fixed slot a kind occupies in the per-kind source table.
const fn kind_slot(kind: MarkerKind) -> usize {
    match kind {
        MarkerKind::Intro => 0,
        MarkerKind::Recap => 1,
        MarkerKind::Credits => 2,
        MarkerKind::Preview => 3,
    }
}

/// The marker a skip control should offer at `position_ms`.
///
/// The earliest marker that contains the position wins. A preview inside the credits would
/// otherwise shadow the credits window, and the credits window is the one that leads to next-up.
#[must_use]
pub fn active_at(markers: &[PlaybackMarker], position_ms: i64) -> Option<PlaybackMarker> {
    markers
        .iter()
        .copied()
        .filter(|marker| marker.contains(position_ms))
        .min_by_key(|marker| marker.start_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker(
        kind: MarkerKind,
        start: i64,
        end: Option<i64>,
        source: MarkerSource,
    ) -> PlaybackMarker {
        PlaybackMarker::new(kind, start, end, source, MarkerConfidence::COMMUNITY).expect("valid")
    }

    #[test]
    fn a_window_must_open_before_it_closes() {
        assert_eq!(
            PlaybackMarker::new(
                MarkerKind::Intro,
                90_000,
                Some(90_000),
                MarkerSource::Chapters,
                MarkerConfidence::EXACT
            ),
            Err(MarkerError::NonPositiveWindow {
                start_ms: 90_000,
                end_ms: 90_000
            })
        );
        assert_eq!(
            PlaybackMarker::new(
                MarkerKind::Intro,
                90_000,
                Some(30_000),
                MarkerSource::Chapters,
                MarkerConfidence::EXACT
            ),
            Err(MarkerError::NonPositiveWindow {
                start_ms: 90_000,
                end_ms: 30_000
            })
        );
    }

    #[test]
    fn a_negative_start_is_refused() {
        assert_eq!(
            PlaybackMarker::new(
                MarkerKind::Recap,
                -1,
                Some(1_000),
                MarkerSource::IntroDb,
                MarkerConfidence::COMMUNITY
            ),
            Err(MarkerError::NegativeStart { start_ms: -1 })
        );
    }

    #[test]
    fn only_credits_may_run_to_the_end() {
        assert!(
            PlaybackMarker::new(
                MarkerKind::Intro,
                0,
                None,
                MarkerSource::IntroDb,
                MarkerConfidence::COMMUNITY
            )
            .is_err()
        );
        assert!(
            PlaybackMarker::new(
                MarkerKind::Credits,
                0,
                None,
                MarkerSource::IntroDb,
                MarkerConfidence::COMMUNITY
            )
            .is_ok()
        );
    }

    #[test]
    fn a_position_inside_a_window_is_contained_at_both_ends() {
        let intro = marker(
            MarkerKind::Intro,
            10_000,
            Some(70_000),
            MarkerSource::IntroDb,
        );

        assert!(!intro.contains(9_999));
        assert!(intro.contains(10_000), "the start is inside");
        assert!(intro.contains(69_999));
        assert!(!intro.contains(70_000), "the end is outside");
    }

    #[test]
    fn an_open_credits_marker_contains_every_later_position() {
        let credits = marker(MarkerKind::Credits, 6_000_000, None, MarkerSource::IntroDb);

        assert!(!credits.contains(5_999_999));
        assert!(credits.contains(6_000_000));
        assert!(credits.contains(i64::MAX));
    }

    #[test]
    fn a_marker_past_the_runtime_is_dropped_rather_than_clamped() {
        let marker = marker(
            MarkerKind::Intro,
            7_300_000,
            Some(7_400_000),
            MarkerSource::IntroDb,
        );

        assert_eq!(marker.bounded(7_200_000), None);
    }

    #[test]
    fn a_marker_that_ends_past_the_runtime_is_clamped() {
        let marker = marker(
            MarkerKind::Intro,
            60_000,
            Some(7_400_000),
            MarkerSource::IntroDb,
        );
        let bounded = marker.bounded(7_200_000).expect("opens inside the file");

        assert_eq!(bounded.end_ms(), Some(7_200_000));
    }

    #[test]
    fn clamping_never_produces_an_empty_window() {
        let marker = marker(
            MarkerKind::Intro,
            7_200_000 - 1,
            Some(8_000_000),
            MarkerSource::IntroDb,
        );

        assert_eq!(
            marker.bounded(7_200_000).map(PlaybackMarker::end_ms),
            Some(Some(7_200_000))
        );
        let collapsed = PlaybackMarker::new(
            MarkerKind::Intro,
            7_200_000,
            Some(7_200_000),
            MarkerSource::IntroDb,
            MarkerConfidence::COMMUNITY,
        );
        assert!(collapsed.is_err());
    }

    #[test]
    fn chapters_beat_the_network_for_the_same_kind() {
        let chapters = PlaybackMarker::chapter(MarkerKind::Intro, 30_000, 90_000).unwrap();
        let database = marker(
            MarkerKind::Intro,
            31_000,
            Some(91_000),
            MarkerSource::IntroDb,
        );

        assert_eq!(
            resolve([database, chapters], Some(1_440_000)),
            vec![chapters]
        );
    }

    #[test]
    fn a_source_with_only_credits_does_not_suppress_a_lower_sources_intro() {
        let chapters_credits =
            PlaybackMarker::chapter(MarkerKind::Credits, 1_300_000, 1_440_000).unwrap();
        let database_intro = marker(
            MarkerKind::Intro,
            30_000,
            Some(90_000),
            MarkerSource::IntroDb,
        );

        let resolved = resolve([chapters_credits, database_intro], Some(1_440_000));

        assert_eq!(resolved.len(), 2);
        assert!(resolved.contains(&database_intro));
        assert!(resolved.contains(&chapters_credits));
    }

    #[test]
    fn a_source_below_the_floor_is_dropped() {
        let weak = PlaybackMarker::new(
            MarkerKind::Intro,
            30_000,
            Some(90_000),
            MarkerSource::IntroDb,
            MarkerConfidence::new(39).unwrap(),
        )
        .unwrap();

        assert!(resolve([weak], Some(1_440_000)).is_empty());
    }

    #[test]
    fn chapters_are_exempt_from_the_floor() {
        let chapters = PlaybackMarker::chapter(MarkerKind::Intro, 30_000, 90_000).unwrap();

        assert!(resolve([chapters], Some(1_440_000)).contains(&chapters));
    }

    #[test]
    fn duplicate_intervals_from_one_source_merge() {
        let first = marker(
            MarkerKind::Intro,
            30_000,
            Some(90_000),
            MarkerSource::IntroDb,
        );
        let second = marker(
            MarkerKind::Intro,
            30_500,
            Some(90_500),
            MarkerSource::IntroDb,
        );

        let resolved = resolve([first, second], Some(1_440_000));

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].start_ms(), 30_000);
        assert_eq!(resolved[0].end_ms(), Some(90_500));
    }

    #[test]
    fn intervals_separated_by_more_than_the_gap_stay_distinct() {
        let first = marker(MarkerKind::Recap, 0, Some(30_000), MarkerSource::IntroDb);
        let second = marker(
            MarkerKind::Recap,
            30_000 + MERGE_GAP_MS + 1,
            Some(60_000),
            MarkerSource::IntroDb,
        );

        assert_eq!(resolve([first, second], Some(1_440_000)).len(), 2);
    }

    #[test]
    fn the_result_is_ordered_by_start() {
        let credits = marker(MarkerKind::Credits, 1_300_000, None, MarkerSource::IntroDb);
        let intro = marker(
            MarkerKind::Intro,
            30_000,
            Some(90_000),
            MarkerSource::IntroDb,
        );

        assert_eq!(
            resolve([credits, intro], Some(1_440_000))
                .iter()
                .map(|marker| marker.kind())
                .collect::<Vec<_>>(),
            vec![MarkerKind::Intro, MarkerKind::Credits]
        );
    }

    #[test]
    fn an_empty_answer_stays_empty() {
        assert!(resolve([], Some(1_440_000)).is_empty());
        assert!(resolve([], None).is_empty());
    }

    #[test]
    fn an_unknown_runtime_does_not_drop_a_marker() {
        let intro = marker(
            MarkerKind::Intro,
            30_000,
            Some(90_000),
            MarkerSource::IntroDb,
        );

        assert_eq!(resolve([intro], None), vec![intro]);
    }

    #[test]
    fn the_active_marker_is_the_earliest_window_that_contains_the_position() {
        let preview = marker(
            MarkerKind::Preview,
            1_350_000,
            Some(1_390_000),
            MarkerSource::IntroDb,
        );
        let credits = marker(MarkerKind::Credits, 1_300_000, None, MarkerSource::IntroDb);

        assert_eq!(
            active_at(&[preview, credits], 1_360_000).map(PlaybackMarker::kind),
            Some(MarkerKind::Credits),
            "the credits window leads to next-up, so it wins the overlap"
        );
        assert_eq!(
            active_at(&[preview, credits], 1_395_000).map(PlaybackMarker::kind),
            Some(MarkerKind::Credits)
        );
        assert_eq!(active_at(&[preview, credits], 1_299_999), None);
    }

    #[test]
    fn a_provider_score_is_read_on_either_scale() {
        assert_eq!(MarkerConfidence::from_provider(Some(0.72)).percent(), 72);
        assert_eq!(MarkerConfidence::from_provider(Some(72.0)).percent(), 72);
        assert_eq!(
            MarkerConfidence::from_provider(None),
            MarkerConfidence::COMMUNITY
        );
        assert_eq!(
            MarkerConfidence::from_provider(Some(f64::NAN)),
            MarkerConfidence::COMMUNITY
        );
        assert_eq!(
            MarkerConfidence::from_provider(Some(0.0)),
            MarkerConfidence::COMMUNITY
        );
        assert_eq!(MarkerConfidence::from_provider(Some(1e300)).percent(), 100);
    }

    #[test]
    fn a_marker_round_trips_through_json_without_losing_its_bounds() {
        let marker = PlaybackMarker::new(
            MarkerKind::Credits,
            1_300_000,
            None,
            MarkerSource::AniSkip,
            MarkerConfidence::COMMUNITY,
        )
        .unwrap();
        let text = serde_json::to_string(&marker).unwrap();
        let parsed: PlaybackMarker = serde_json::from_str(&text).unwrap();

        assert_eq!(parsed, marker);
        assert!(
            serde_json::from_str::<PlaybackMarker>(
                r#"{"kind":"intro","startMs":10,"endMs":null,"source":"introdb","confidence":60}"#
            )
            .is_err()
        );
    }
}
