//! Watch policy: where playback resumes, when a title counts as watched, and which episode
//! comes next.
//!
//! Three pure decisions, no IO and no clock ([`FLOWS`](../../../docs/FLOWS.md) F10):
//!
//! - [`resume_point`] chooses between starting at zero, starting at the stored position minus
//!   [`RESUME_REWIND_MS`], and not resuming at all, for a title that is finished.
//! - [`finalize`] turns the end of a playback into the watched flag, the instant it was
//!   watched, and the count bump to apply to the row.
//! - [`next_episode`] picks the following episode in the scheme the account is using, reading
//!   the ordering overlay rather than the canonical numbers.
//!
//! The threshold here is the *playback* threshold, [`WATCHED_PERCENT`]; it is deliberately
//! higher than the storage floor in [`crate::library`], and neither number decides the other.
//! The floor is the weakest watched row the database accepts ([`DATA_MODEL`] I6), which keeps a
//! manual or imported mark below this threshold representable. A policy-driven mark still has
//! to clear the floor, which is why [`WatchProgress::mark_watched`] can refuse one.
//!
//! [`DATA_MODEL`]: ../../../docs/DATA_MODEL.md
//! [`WatchProgress::mark_watched`]: crate::library::WatchProgress::mark_watched

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::catalogue::{EpisodeOrdering, EpisodePosition};
use crate::library::PlaybackPosition;

/// The share of a runtime at which a playback counts as finished (F10).
///
/// `>=` the threshold is watched; a position just below it is still resumable. The credits and
/// "played to the end" reach the same decision through [`finalize`]'s `ended_naturally`.
pub const WATCHED_PERCENT: f64 = 0.95;

/// Below this share of the runtime, playback starts at zero rather than resuming (F10).
///
/// `>=` the threshold resumes; a position just below it restarts, because a few seconds in is
/// not a place worth returning to.
pub const RESUME_PERCENT: f64 = 0.02;

/// How far before the stored position playback resumes (F10).
pub const RESUME_REWIND_MS: i64 = 3_000;

/// How far a stored position was played, in millionths of the runtime.
///
/// The thresholds are written as the fractions the documents state and converted to this once;
/// the policy's own comparisons are then integer arithmetic, so a boundary test exercises the
/// documented cutoff rather than the last bit of an `f64` division. A millionth is fine enough
/// that the fixtures on either side of a cutoff stay on their own side: at basis-point
/// resolution a position at 0.01999 would round up to the 0.02 cutoff and resume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Millionths(u32);

impl Millionths {
    /// The played fraction of a position, saturating at one million for a position past a
    /// stale runtime.
    fn of(position: PlaybackPosition) -> Self {
        let percent = position.percent();
        if !percent.is_finite() || percent <= 0.0 {
            return Self(0);
        }
        if percent >= 1.0 {
            return Self(1_000_000);
        }
        // The position and the runtime are both below 2^53 ms, so the ratio is exact to well
        // inside a millionth and the rounding here decides the last 1/1000000 of the runtime.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Self((percent * 1_000_000.0).round() as u32)
    }
}

/// Whether a playback position has reached the watched threshold (F10).
///
/// The entity does not answer this question any more: it validates the storage floor and takes
/// the decision from [`FinalizeOutcome`]. That separation is what keeps a row reported at 0.93
/// resumable instead of being stored watched and vanishing into the "finished" list.
#[must_use]
pub fn is_watched(position: PlaybackPosition) -> bool {
    Millionths::of(position) >= threshold_millionths(WATCHED_PERCENT)
}

/// A threshold as millionths, so it is written once as the fraction the documents state.
const fn threshold_millionths(percent: f64) -> Millionths {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Millionths((percent * 1_000_000.0).round() as u32)
}

/// What the player should do with a stored position (F10).
///
/// The three outcomes are the three answers F10 gives the caller — a start offset, a restart,
/// or a hand-off to the next episode — so the caller cannot mistake "no resume" for "resume at
/// zero" and quietly replay a finished title from the beginning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumePoint {
    /// Start playback from the beginning: there is no meaningful earlier position.
    Restart,
    /// Start playback at this offset into the file.
    StartAt {
        /// The offset, in milliseconds, never negative and never past the stored position.
        at_ms: i64,
    },
    /// Do not resume; the title is finished, and the caller should offer the next episode.
    NextEpisode,
}

/// What the policy decided about a playback that just ended (F10).
///
/// The entity applies this through [`WatchProgress::apply_finalize`], which is where the
/// storage floor is enforced; nothing else in the domain decides the watched flag.
///
/// [`WatchProgress::apply_finalize`]: crate::library::WatchProgress::apply_finalize
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalizeOutcome {
    /// Whether this playback finished the title under the policy.
    pub watched: bool,
    /// When it was finished, or `None` when the flag is false.
    pub watched_at: Option<Timestamp>,
    /// How much to add to the row's `play_count`: one for each playback that ends.
    pub play_count: u32,
}

/// Decides what to do with a stored position (F10).
///
/// `ended_naturally` is the player's report that the credits were reached or the file ended,
/// which is the second way a title counts as finished. It is a parameter rather than something
/// inferred from the position because a file can end before the position passes the threshold
/// — a short extra, a truncated encode — and F10 says that is watched.
#[must_use]
pub fn resume_point(position: PlaybackPosition, ended_naturally: bool) -> ResumePoint {
    if ended_naturally || is_watched(position) {
        return ResumePoint::NextEpisode;
    }
    if Millionths::of(position) < threshold_millionths(RESUME_PERCENT) {
        return ResumePoint::Restart;
    }
    // A file shorter than the rewind window must still be watchable: clamping keeps the offset
    // inside the media instead of producing a negative one the player would have to repair.
    ResumePoint::StartAt {
        at_ms: (position.position_ms() - RESUME_REWIND_MS).max(0),
    }
}

/// Decides how one finished playback changes a watch-progress row (F10).
///
/// The flag is the policy's own threshold or a natural end; `watched_at` is `at` only when this
/// playback is what finished the title, so a re-watch keeps the instant it was first finished.
/// Every call counts as a play, including a re-watch of something already watched.
#[must_use]
pub fn finalize(
    position: PlaybackPosition,
    ended_naturally: bool,
    at: Timestamp,
) -> FinalizeOutcome {
    let watched = ended_naturally || is_watched(position);
    FinalizeOutcome {
        watched,
        watched_at: watched.then_some(at),
        play_count: 1,
    }
}

/// Where one episode sits in the scheme the caller is asking about.
///
/// The canonical position is where the episode sits as aired; the overlay, when the caller has
/// one, is the row for the scheme it is ranking for. [`position`](Self::position) prefers the
/// overlay, and that preference is only sound for a placement built for the scheme in hand:
/// [`EpisodeOrderingStrategy::order`](crate::ordering::EpisodeOrderingStrategy::order) is what
/// decides, per scheme, whether an episode without an overlay row has a place at all rather than
/// keeping its aired one. Building one of these per episode is what lets [`next_episode`] rank a
/// list without ever comparing episode numbers itself.
///
/// [`DATA_MODEL`]: ../../../docs/DATA_MODEL.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpisodePlacement {
    /// The episode's row id.
    pub episode_id: i64,
    /// The episode's canonical, aired position.
    pub canonical: EpisodePosition,
    /// The `episode_ordering` row for the active scheme, when the scheme renumbers it.
    pub overlay: Option<EpisodeOrdering>,
}

impl EpisodePlacement {
    /// Places an episode with its canonical position and an optional overlay row.
    #[must_use]
    pub const fn new(
        episode_id: i64,
        canonical: EpisodePosition,
        overlay: Option<EpisodeOrdering>,
    ) -> Self {
        Self {
            episode_id,
            canonical,
            overlay,
        }
    }

    /// The episode's position in the scheme, which is the overlay when there is one.
    ///
    /// The overlay's scheme is trusted and not compared, because this accessor has no scheme to
    /// compare it with: the caller built the placement for the scheme it is ranking for.
    /// [`EpisodeOrderingStrategy::order`](crate::ordering::EpisodeOrderingStrategy::order) is
    /// handed the scheme and uses a row only when it matches, which is where a row read for the
    /// wrong scheme is caught.
    #[must_use]
    pub fn position(&self) -> EpisodePosition {
        self.overlay
            .map_or(self.canonical, EpisodeOrdering::position)
    }
}

/// The episode after the one at `current`, in the scheme the placements describe.
///
/// `current` is the playing episode's position *in that same scheme*, not its canonical
/// number: under an overlay the two differ, and comparing a scheme's positions against a
/// canonical one would pick the wrong episode. [`EpisodePlacement::position`] is what the
/// caller ranks by, so a next-up choice and the list the user sees come from one numbering.
///
/// The slice's order does not matter; the ranking is the scheme's. `None` means there is no
/// following episode: `current` is the last one, or nothing sits after it, or the list is
/// empty. An episode whose placement equals `current` is the playing one and is skipped.
#[must_use]
pub fn next_episode(
    order: &[EpisodePlacement],
    current: EpisodePosition,
) -> Option<EpisodePlacement> {
    order
        .iter()
        .filter(|placement| placement.position() > current)
        .min_by_key(|placement| placement.position())
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::{EpisodeNumber, OrderingScheme, SeasonNumber};
    use proptest::prelude::*;

    const SECOND: i64 = 1_000;
    const MINUTE: i64 = 60 * SECOND;
    /// The fixture runtime. A hundred seconds divides evenly by 100, so every whole percentage
    /// the acceptance names is a whole millisecond and the boundary assertions read exactly.
    const RUNTIME_MS: i64 = 100 * SECOND;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millisecond(millis).expect("a millisecond inside the representable range")
    }

    fn now() -> Timestamp {
        at(1_700_000_000_000)
    }

    /// A position at a whole percent of the fixture runtime.
    fn percent(whole: i64) -> PlaybackPosition {
        PlaybackPosition::new(RUNTIME_MS / 100 * whole, Some(RUNTIME_MS))
            .expect("the fixture position is valid")
    }

    fn position(position_ms: i64, duration_ms: i64) -> PlaybackPosition {
        PlaybackPosition::new(position_ms, Some(duration_ms))
            .expect("the fixture position is valid")
    }

    fn seat(season: u32, episode: u32) -> EpisodePosition {
        EpisodePosition {
            season_number: SeasonNumber::new(season),
            episode_number: EpisodeNumber::new(episode),
        }
    }

    /// One episode in canonical order with no overlay, which is what `aired` uses.
    fn aired(episode: u32) -> EpisodePlacement {
        seat_one(episode)
    }

    /// One episode whose active scheme renumbers it, as the `episode_ordering` row stores it.
    fn renumbered(canonical_episode: u32, scheme_episode: u32) -> EpisodePlacement {
        let overlay = EpisodeOrdering {
            episode_id: i64::from(canonical_episode),
            scheme: OrderingScheme::Absolute,
            season_number: SeasonNumber::new(1),
            episode_number: EpisodeNumber::new(scheme_episode),
        };
        EpisodePlacement::new(
            i64::from(canonical_episode),
            seat(1, canonical_episode),
            Some(overlay),
        )
    }

    /// A season-one episode with no overlay, which is what `aired` uses.
    fn seat_one(episode: u32) -> EpisodePlacement {
        // The tests name episodes by their position, which is unique in each fixture.
        EpisodePlacement::new(i64::from(episode), seat(1, episode), None)
    }

    fn season_one(episode: u32) -> EpisodePosition {
        seat(1, episode)
    }

    #[test]
    fn a_position_below_two_percent_restarts() {
        assert_eq!(
            resume_point(percent(0), false),
            ResumePoint::Restart,
            "zero is the empty watch, not a resume"
        );
        assert_eq!(resume_point(percent(1), false), ResumePoint::Restart);
    }

    #[test]
    fn the_restart_band_ends_exactly_at_two_percent() {
        assert_eq!(
            resume_point(position(199, 10_000), false),
            ResumePoint::Restart,
            "0.0199 is below the cutoff"
        );
        assert_eq!(
            resume_point(position(200, 10_000), false),
            ResumePoint::StartAt { at_ms: 0 },
            "0.02 is the first resumable position, and three seconds before it is zero"
        );
        assert_eq!(
            resume_point(position(201, 10_000), false),
            ResumePoint::StartAt { at_ms: 0 }
        );
    }

    #[test]
    fn a_position_inside_the_band_resumes_three_seconds_earlier() {
        assert_eq!(
            resume_point(percent(50), false),
            ResumePoint::StartAt {
                at_ms: RUNTIME_MS / 2 - RESUME_REWIND_MS
            }
        );
    }

    #[test]
    fn the_threshold_band_ends_exactly_at_the_watched_percent() {
        assert_eq!(
            resume_point(percent(92), false),
            ResumePoint::StartAt {
                at_ms: RUNTIME_MS / 100 * 92 - RESUME_REWIND_MS
            },
            "I6's storage floor is not the policy's cutoff: 0.92 still resumes"
        );
        assert_eq!(
            resume_point(position(94_999, RUNTIME_MS), false),
            ResumePoint::StartAt {
                at_ms: 94_999 - RESUME_REWIND_MS
            },
            "0.94999 is below the cutoff"
        );
        assert_eq!(
            resume_point(percent(95), false),
            ResumePoint::NextEpisode,
            "0.95 is the first finished position"
        );
        assert_eq!(resume_point(percent(99), false), ResumePoint::NextEpisode);
    }

    #[test]
    fn a_finished_position_is_never_rewound_into_the_credits() {
        assert_eq!(
            resume_point(percent(99), false),
            ResumePoint::NextEpisode,
            "resuming 3 s before 99% would re-enter the credits, so the caller is given the next episode instead"
        );
        assert_eq!(resume_point(percent(100), false), ResumePoint::NextEpisode);
    }

    #[test]
    fn a_natural_end_hands_off_even_below_the_threshold() {
        assert_eq!(
            resume_point(percent(50), true),
            ResumePoint::NextEpisode,
            "a file that ended is finished wherever the clock stopped"
        );
    }

    #[test]
    fn a_zero_length_runtime_restarts_rather_than_dividing_by_it() {
        let unknown = PlaybackPosition::new(0, Some(0)).expect("zero is a valid runtime");
        assert_eq!(resume_point(unknown, false), ResumePoint::Restart);
        assert_eq!(finalize(unknown, false, now()).watched_at, None);

        let no_runtime =
            PlaybackPosition::new(600_000, None).expect("a position without a runtime");
        assert_eq!(
            resume_point(no_runtime, false),
            ResumePoint::Restart,
            "without a runtime there is no percentage, so there is nowhere to resume to"
        );
    }

    #[test]
    fn a_video_shorter_than_the_rewind_window_stays_watchable() {
        let short = position(500, 4 * SECOND);
        assert_eq!(
            resume_point(short, false),
            ResumePoint::StartAt { at_ms: 0 },
            "clamped, not negative"
        );

        let exactly_the_window = position(SECOND, 3 * SECOND);
        assert_eq!(
            resume_point(exactly_the_window, false),
            ResumePoint::StartAt { at_ms: 0 }
        );

        let just_over = position(2 * SECOND, 10 * SECOND);
        assert_eq!(
            resume_point(just_over, false),
            ResumePoint::StartAt { at_ms: 0 }
        );
        assert_eq!(
            resume_point(position(5 * SECOND, 10 * SECOND), false),
            ResumePoint::StartAt { at_ms: 2 * SECOND }
        );
    }

    #[test]
    fn a_resume_offset_is_never_negative_or_past_the_stored_position() {
        // The last two are inside the band; the first two are not, and a case that never reaches
        // the band is asserted to restart rather than skipped.
        for (position_ms, duration_ms, resumable) in [
            (0, 10_000, false),
            (199, 10_000, false),
            (2_999, 10_000, true),
            (3_000, 10_000, true),
            (1, 4_000, false),
            (500, 4_000, true),
        ] {
            match resume_point(position(position_ms, duration_ms), false) {
                ResumePoint::StartAt { at_ms } => {
                    assert!(
                        resumable,
                        "{position_ms}/{duration_ms} is below the resume band"
                    );
                    assert!(at_ms >= 0, "a negative offset is never offered");
                    assert!(
                        at_ms <= position_ms,
                        "resuming later than the stored position would skip unwatched content"
                    );
                }
                ResumePoint::Restart => {
                    assert!(
                        !resumable,
                        "{position_ms}/{duration_ms} should have been resumable"
                    );
                }
                ResumePoint::NextEpisode => {
                    panic!("{position_ms}/{duration_ms} is not a finished position")
                }
            }
        }
    }

    #[test]
    fn finalize_marks_watched_at_the_threshold_and_not_before() {
        let below = finalize(percent(92), false, now());
        assert!(!below.watched);
        assert_eq!(below.watched_at, None);
        assert_eq!(
            below.play_count, 1,
            "a play happened even when it is unmarked"
        );

        let exactly = finalize(percent(95), false, now());
        assert!(exactly.watched);
        assert_eq!(exactly.watched_at, Some(now()));

        let above = finalize(percent(99), false, now());
        assert!(above.watched);
        assert_eq!(above.watched_at, Some(now()));
    }

    #[test]
    fn finalize_at_the_acceptance_boundaries() {
        let expected = [
            (0, false),
            (2, false),
            (50, false),
            (92, false),
            (95, true),
            (99, true),
            (100, true),
        ];
        for (whole, watched) in expected {
            let outcome = finalize(percent(whole), false, now());
            assert_eq!(
                outcome.watched, watched,
                "{whole}% was decided as watched = {watched}"
            );
            assert_eq!(
                outcome.watched_at.is_some(),
                watched,
                "the stamp follows the flag"
            );
            assert_eq!(outcome.play_count, 1);
        }
    }

    #[test]
    fn finalize_treats_a_zero_length_runtime_as_not_watched_unless_it_ended() {
        let zero = PlaybackPosition::new(0, Some(0)).expect("zero is a valid runtime");
        assert!(!finalize(zero, false, now()).watched);
        assert!(finalize(zero, true, now()).watched);

        let no_runtime =
            PlaybackPosition::new(600_000, None).expect("a position without a runtime");
        assert!(!finalize(no_runtime, false, now()).watched);
    }

    #[test]
    fn finalize_counts_a_natural_end_as_watched() {
        let outcome = finalize(percent(50), true, now());
        assert!(outcome.watched);
        assert_eq!(outcome.watched_at, Some(now()));
    }

    #[test]
    fn the_policy_threshold_is_above_the_storage_floor_it_must_satisfy() {
        // Both numbers are compile-time constants, so the relationship is checked at compile
        // time rather than by a test that a later edit could delete along with the rule.
        const { assert!(WATCHED_PERCENT > crate::library::STORAGE_FLOOR_PERCENT) };
        assert!(
            is_watched(percent(95)) && !is_watched(percent(92)),
            "the two numbers decide different questions"
        );
    }

    #[test]
    fn a_position_past_a_stale_runtime_is_watched_rather_than_overflowing() {
        let past = PlaybackPosition::new(10 * MINUTE, Some(MINUTE)).expect("valid");
        assert_eq!(Millionths::of(past), Millionths(1_000_000));
        assert!(is_watched(past));
        assert_eq!(
            resume_point(past, false),
            ResumePoint::NextEpisode,
            "2.0 of the runtime is past the end, not a resumable position"
        );
    }

    #[test]
    fn next_episode_follows_the_canonical_order() {
        let order = [aired(1), aired(2), aired(3)];
        assert_eq!(
            next_episode(&order, season_one(1)).map(|next| next.episode_id),
            Some(2)
        );
        assert_eq!(
            next_episode(&order, season_one(2)).map(|next| next.episode_id),
            Some(3),
            "the middle of a series is the ordinary case"
        );
        assert_eq!(
            next_episode(&order, season_one(3)),
            None,
            "nothing follows the last episode"
        );
    }

    #[test]
    fn next_episode_has_no_answer_without_one() {
        assert_eq!(
            next_episode(&[], season_one(1)),
            None,
            "an empty series has no next episode"
        );
        assert_eq!(
            next_episode(&[aired(1)], season_one(1)),
            None,
            "one episode has no next one"
        );
        assert_eq!(
            next_episode(&[aired(1)], season_one(9)),
            None,
            "an episode that is not in the list has nothing after it either"
        );
    }

    #[test]
    fn next_episode_takes_the_current_position_in_the_active_scheme() {
        // Aired these are 1x01 and 1x02; the scheme runs them as 1x03 then 1x04.
        let order = [renumbered(1, 3), renumbered(2, 4)];
        assert_eq!(
            next_episode(&order, season_one(3)).map(|next| next.episode_id),
            Some(2),
            "1x02 is next after 1x01 once both are read in the scheme"
        );
        assert_eq!(
            next_episode(&order, season_one(1)).map(|next| next.episode_id),
            Some(1),
            "against a canonical position the same list answers a different question, which is \
             why the caller has to hand over the scheme's own position"
        );
        assert_eq!(
            next_episode(&order, season_one(4)),
            None,
            "nothing follows the scheme's last episode"
        );
    }

    #[test]
    fn next_episode_ranks_by_the_overlay_where_it_exists() {
        // As aired the episodes run 1x01, 1x02, 1x03; the scheme broadcast them 1x02, 1x03, 1x01.
        let order = [renumbered(1, 3), renumbered(2, 1), renumbered(3, 2)];
        assert_eq!(
            next_episode(&order, season_one(1)).map(|next| next.episode_id),
            Some(3),
            "the scheme's 1x02 follows its 1x01"
        );
        assert_eq!(
            next_episode(&order, season_one(2)).map(|next| next.episode_id),
            Some(1),
            "and its 1x03 follows its 1x02"
        );
        assert_eq!(
            next_episode(&order, season_one(3)),
            None,
            "the scheme puts canonical 1x01 last"
        );
    }

    #[test]
    fn next_episode_keeps_the_canonical_position_where_the_overlay_is_silent() {
        // Only 1x02 is renumbered, to 1x40; the rest of the season keeps its aired numbers, so
        // the scheme's order is 1x01, 1x03, 1x40. That mixture is what the overlay being
        // additive rather than a full replacement list means.
        let order = [aired(1), renumbered(2, 40), aired(3)];
        assert_eq!(
            next_episode(&order, season_one(1)).map(|next| next.episode_id),
            Some(3),
            "an episode with no overlay of its own keeps its canonical position"
        );
        assert_eq!(
            next_episode(&order, season_one(3)).map(|next| next.episode_id),
            Some(2),
            "and the renumbered episode then follows it in the scheme"
        );
        assert_eq!(
            next_episode(&order, season_one(40)),
            None,
            "nothing sits after the renumbered episode"
        );
        assert_eq!(
            next_episode(&order, season_one(2)).map(|next| next.episode_id),
            Some(3),
            "the canonical 1x02 is not a position this scheme ranks by"
        );
    }

    #[test]
    fn an_episode_placement_reports_the_scheme_position_it_will_be_ranked_by() {
        assert_eq!(aired(3).position(), season_one(3));
        assert_eq!(renumbered(3, 40).position(), seat(1, 40));
    }

    #[test]
    fn a_finalize_outcome_round_trips_through_json() {
        let watched = FinalizeOutcome {
            watched: true,
            watched_at: Some(now()),
            play_count: 1,
        };
        assert_eq!(
            serde_json::from_str::<FinalizeOutcome>(&serde_json::to_string(&watched).unwrap())
                .unwrap(),
            watched
        );

        let unwatched = FinalizeOutcome {
            watched: false,
            watched_at: None,
            play_count: 2,
        };
        assert_eq!(
            serde_json::from_str::<FinalizeOutcome>(&serde_json::to_string(&unwatched).unwrap())
                .unwrap(),
            unwatched
        );
    }

    proptest! {
        #[test]
        fn a_resume_offset_is_always_inside_the_media(
            position_ms in 0i64..86_400_000,
            duration_ms in 1i64..86_400_000,
        ) {
            let span = PlaybackPosition::new(position_ms, Some(duration_ms)).expect("non-negative");
            match resume_point(span, false) {
                ResumePoint::StartAt { at_ms } => {
                    prop_assert!(at_ms >= 0, "an offset is never negative");
                    prop_assert!(at_ms <= position_ms, "and never past the stored position");
                    // The offset is the stored position less the rewind window whenever the
                    // media is long enough to give one, so the boundary fixtures and the rule
                    // cannot drift apart.
                    prop_assert_eq!(at_ms, (position_ms - RESUME_REWIND_MS).max(0));
                }
                ResumePoint::Restart => prop_assert!(span.percent() < RESUME_PERCENT),
                ResumePoint::NextEpisode => prop_assert!(is_watched(span)),
            }
        }
    }

    #[test]
    fn the_millionth_bounds_are_total() {
        assert_eq!(
            Millionths::of(position(i64::MAX, i64::MAX)),
            Millionths(1_000_000)
        );
        assert_eq!(Millionths::of(position(0, i64::MAX)), Millionths(0));
        assert_eq!(
            Millionths::of(PlaybackPosition::new(0, None).expect("valid")),
            Millionths(0)
        );
    }
}
