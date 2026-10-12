//! Episode ordering: the list a scheme puts a title's episodes in, built over the canonical
//! numbers rather than instead of them.
//!
//! [`EpisodeOrderingStrategy::order`] is a pure function from a set of [`Episode`]s, the
//! `episode_ordering` rows a repository read for them, and one [`OrderingScheme`], to the list
//! that scheme displays. It fills the gap [`next_episode`](crate::watch::next_episode) assumes:
//! that function ranks an already-ordered slice of
//! [`EpisodePlacement`](crate::watch::EpisodePlacement)s and never compares episode numbers
//! itself, so a caller holding episodes and overlay rows has nothing ordered to hand it.
//!
//! **The overlay renumbers the display and the canonical numbers never move** ([`DATA_MODEL`]
//! §2.3). Every placement returned carries the canonical position it arrived with, and a
//! scheme's number is always the `episode_ordering` row for that scheme: the strategy never
//! derives one scheme's number from another scheme's, nor a scheme number from a column that
//! belongs to a different numbering. `docs/research/data-sources.md` §1.7 states the same rule
//! for storage ("Store **all three** numbering vectors per episode row ... and never derive one
//! from another"), because a derived number is exactly the silent fallback this module exists
//! to avoid. `episode.absolute_number` is the convenience column §2.3 describes and not a fifth
//! source of truth: where a provider gives the running number on the episode, the caller
//! materializes the `absolute` row from it.
//!
//! ## What each scheme does
//!
//! Four schemes, four rules, because that is what the providers present:
//!
//! - **`aired`** is the canonical `episode` columns, season then episode
//!   ([`EpisodePosition`](crate::catalogue::EpisodePosition)'s own order). §2.3 makes those
//!   columns the aired order, so `aired` is the only scheme that needs no row and the only one
//!   that can place every episode. A provider parks a special in season 0 (the specials season,
//!   [`SeasonNumber`](crate::catalogue::SeasonNumber)), so specials lead the aired list whatever
//!   date they aired. A caller that supplies an `aired` row anyway is trusted with it, exactly
//!   as [`EpisodePlacement::position`](crate::watch::EpisodePlacement::position) trusts it, so the
//!   list stays consistent with the number `next_episode` will rank by.
//! - **`dvd`** is the disc order, which is a permutation the aired columns cannot express.
//!   Firefly is the standard example: Fox broadcast the episodes out of sequence and the disc
//!   set restored the intended one. Each episode's disc number is its `dvd` row, and an episode
//!   without one has no disc number.
//! - **`absolute`** is one running number for the whole series — the scheme's own description is
//!   "a single running number across every season" — which is the order anime lists and TVDB's
//!   `/episodes/absolute` use. The number is the `absolute` row's `episode_number`; the row's
//!   season is the notional bucket the number is stored under, and the strategy ranks by the
//!   stored position rather than flattening it, so the list it returns is the list
//!   [`EpisodePlacement::position`](crate::watch::EpisodePlacement::position) reports and
//!   [`next_episode`](crate::watch::next_episode) therefore follows.
//! - **`anime`** is the broadcast order, and it mixes the numbering with specials. A special the
//!   source numbered takes that place in the run. A special the source did not number is refused
//!   ([`UnplacedReason::UnnumberedSpecial`]): `AniDB` types specials and gives them no season or
//!   episode pair at all, so whether such an episode belongs before the first episode, between
//!   two of them, or after the finale is not derivable from anything stored, and guessing would
//!   change which episode plays next.
//!
//! Refusing is the point. A scheme that cannot place an episode reports it in
//! [`OrderedEpisodes::unplaced`] with a typed reason instead of dropping it or ranking it at a
//! number the source never gave; a caller showing the DVD order of a title the provider never
//! gave a disc order for gets an empty list and a reason per episode, not the aired order under
//! a disc label.
//!
//! ## Totality and determinism
//!
//! Every episode handed in is either in [`OrderedEpisodes::placements`] once or in
//! [`OrderedEpisodes::unplaced`] once, whatever the input: an empty series, a single episode, an
//! episode with no number for the active scheme, two episodes contending for one position (both
//! are kept, ordered by episode id), and a broken `(episode_id, scheme)` key with two rows for
//! one episode (the higher position is used). The output never depends on the order the caller
//! read its episodes or its rows in, so a keyset-paged query and a cache may arrive in any order.
//!
//! ## The default scheme
//!
//! [`DEFAULT_SCHEME`] is `aired`. FLOWS F10 asks for "the active ordering scheme" and names no
//! fallback; the aired order is the only one whose numbers are in the `episode` row itself
//! (§2.3), so it is the one a caller can produce without having read `episode_ordering` first,
//! and the one a freshly synced title has.
//!
//! [`DATA_MODEL`]: ../../../docs/DATA_MODEL.md

use crate::catalogue::{Episode, EpisodeOrdering, EpisodeType, OrderingScheme, SeasonNumber};
use crate::watch::EpisodePlacement;

/// The scheme a caller uses when it has no preference (FLOWS F10).
pub const DEFAULT_SCHEME: OrderingScheme = OrderingScheme::Aired;

/// Why the active scheme has no place for an episode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnplacedReason {
    /// The episode has no `episode_ordering` row for the active scheme.
    ///
    /// A scheme other than `aired` is a parallel numbering the provider supplies, so an episode
    /// without a row for it has no number rather than a default one. Reporting this is what stops
    /// a disc or absolute list from quietly becoming the aired list.
    NoSchemeNumber {
        /// The scheme that has no number for the episode.
        scheme: OrderingScheme,
    },
    /// The episode is a special and the `anime` scheme has no number for it.
    ///
    /// Only `anime` produces this. Providers that number anime as one run type specials
    /// separately and give them no season or episode pair, so their place among the numbered
    /// episodes is not stored anywhere; the strategy says so instead of choosing a slot.
    UnnumberedSpecial,
}

/// One episode the active scheme has no place for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Unplaced {
    /// The episode's row id, so the caller can name or re-sync it.
    pub episode_id: i64,
    /// Why the scheme has no place for it.
    pub reason: UnplacedReason,
}

/// The active scheme's list, and the episodes it could not place.
///
/// The two halves partition the input; a caller renders [`placements`](Self::placements) and
/// decides what to do about [`unplaced`](Self::unplaced), but it never has to guess whether an
/// episode was dropped, because the lists together hold every episode it handed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderedEpisodes {
    placements: Vec<EpisodePlacement>,
    unplaced: Vec<Unplaced>,
}

impl OrderedEpisodes {
    /// The episodes in the scheme's order, ready for
    /// [`next_episode`](crate::watch::next_episode).
    #[must_use]
    pub fn placements(&self) -> &[EpisodePlacement] {
        &self.placements
    }

    /// The episodes the scheme has no number for, ordered by episode id.
    ///
    /// Sorted rather than left in the caller's read order, so the report is as deterministic as
    /// the list.
    #[must_use]
    pub fn unplaced(&self) -> &[Unplaced] {
        &self.unplaced
    }
}

/// Builds one scheme's list from the canonical episodes and the overlay rows.
///
/// A unit type like [`ReleaseRanker`](crate::rank::ReleaseRanker): the scheme is an argument
/// rather than an implementation, so one call site can offer the user a scheme choice without a
/// trait object per scheme.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EpisodeOrderingStrategy;

impl EpisodeOrderingStrategy {
    /// Orders `episodes` the way `scheme` numbers them, reading `overlays` for the scheme's rows.
    ///
    /// `overlays` may hold rows for any scheme and rows for episodes outside `episodes`; a row is
    /// used only when its scheme is `scheme` and its `episode_id` names an episode that is
    /// present. Where the schema's `(episode_id, scheme)` key has been broken and two rows arrive
    /// for one episode, the higher position is used, so the list does not depend on the order the
    /// caller read its rows in.
    #[must_use]
    pub fn order(
        scheme: OrderingScheme,
        episodes: &[Episode],
        overlays: &[EpisodeOrdering],
    ) -> OrderedEpisodes {
        let mut placements = Vec::with_capacity(episodes.len());
        let mut unplaced = Vec::new();
        for episode in episodes {
            match number_for(scheme, episode, overlays) {
                Ok(overlay) => {
                    placements.push(EpisodePlacement::new(
                        episode.id,
                        episode.position(),
                        overlay,
                    ));
                }
                Err(reason) => unplaced.push(Unplaced {
                    episode_id: episode.id,
                    reason,
                }),
            }
        }
        // The key is the position the placement reports, so the list and
        // `EpisodePlacement::position` — and therefore `next_episode` — cannot disagree. The
        // episode id breaks a tie, which is what keeps a contended position in a fixed order
        // instead of the order the rows happened to be read in.
        placements.sort_by_key(|placement| (placement.position(), placement.episode_id));
        unplaced.sort_by_key(|episode| episode.episode_id);
        OrderedEpisodes {
            placements,
            unplaced,
        }
    }
}

/// The overlay row to rank one episode by under `scheme`, or why there is none.
///
/// Taking the maximum of several rows for one episode is the duplicate-row rule: the schema's
/// primary key is `(episode_id, scheme)`, so a second row is a defect, and a fixed choice is what
/// keeps two callers that read the defect in different orders agreeing on the list.
fn number_for(
    scheme: OrderingScheme,
    episode: &Episode,
    overlays: &[EpisodeOrdering],
) -> Result<Option<EpisodeOrdering>, UnplacedReason> {
    if let Some(row) = overlays
        .iter()
        .filter(|row| row.episode_id == episode.id && row.scheme == scheme)
        .max_by_key(|row| row.position())
    {
        return Ok(Some(*row));
    }
    match scheme {
        // The canonical columns are the aired order, so this scheme needs no row (§2.3).
        OrderingScheme::Aired => Ok(None),
        // A special with no `anime` number is the one case with no place at all rather than a
        // gap to fill: see the module docs.
        OrderingScheme::Anime if is_special(episode) => Err(UnplacedReason::UnnumberedSpecial),
        _ => Err(UnplacedReason::NoSchemeNumber { scheme }),
    }
}

/// Whether the episode is a special, by either marker in the schema.
///
/// `episode_type` is the provider's own typing and season 0 is the specials season the schema
/// reserves, which is where a provider that does not type specials puts them.
fn is_special(episode: &Episode) -> bool {
    episode.episode_type == Some(EpisodeType::Special)
        || episode.season_number == SeasonNumber::new(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::EpisodeNumber;
    use crate::watch::next_episode;
    use jiff::Timestamp;
    use proptest::prelude::*;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millisecond(millis).expect("a millisecond inside the representable range")
    }

    /// An episode at a canonical position. Everything the ordering never reads is None.
    fn episode(id: i64, season: u32, number: u32, kind: Option<EpisodeType>) -> Episode {
        Episode {
            id,
            title_id: 1,
            season_id: i64::from(season) + 1,
            season_number: SeasonNumber::new(season),
            episode_number: EpisodeNumber::new(number),
            absolute_number: None,
            name: None,
            overview: None,
            air_date: None,
            runtime_minutes: None,
            still_provider: None,
            still_path: None,
            still_blurhash: None,
            vote_average: None,
            vote_count: None,
            episode_type: kind,
            production_code: None,
            updated_at: at(1_700_000_000_000),
        }
    }

    /// One `episode_ordering` row under the scheme's number.
    fn row(episode_id: i64, scheme: OrderingScheme, season: u32, number: u32) -> EpisodeOrdering {
        EpisodeOrdering {
            episode_id,
            scheme,
            season_number: SeasonNumber::new(season),
            episode_number: EpisodeNumber::new(number),
        }
    }

    fn ids(ordered: &OrderedEpisodes) -> Vec<i64> {
        ordered
            .placements()
            .iter()
            .map(|placement| placement.episode_id)
            .collect()
    }

    fn refused(ordered: &OrderedEpisodes) -> Vec<(i64, UnplacedReason)> {
        ordered
            .unplaced()
            .iter()
            .map(|episode| (episode.episode_id, episode.reason))
            .collect()
    }

    /// The series every golden case orders, as the canonical columns hold it: four episodes and
    /// the special the provider parked in season 0.
    fn series() -> Vec<Episode> {
        vec![
            episode(1, 1, 1, None),
            episode(2, 1, 2, None),
            episode(3, 1, 3, None),
            episode(4, 1, 4, None),
            episode(5, 0, 1, Some(EpisodeType::Special)),
        ]
    }

    /// One golden case: the scheme, the rows a repository would read for this title, the list the
    /// scheme has to return, and the episodes it must report rather than place.
    struct Golden {
        scheme: OrderingScheme,
        rows: Vec<EpisodeOrdering>,
        expected: Vec<i64>,
        refused: Vec<(i64, UnplacedReason)>,
    }

    fn golden(
        scheme: OrderingScheme,
        rows: Vec<EpisodeOrdering>,
        expected: Vec<i64>,
        refused: Vec<(i64, UnplacedReason)>,
    ) -> Golden {
        Golden {
            scheme,
            rows,
            expected,
            refused,
        }
    }

    /// The acceptance fixture corpus: one case per scheme, each one readable as the rule it tests.
    ///
    /// Every case orders the same five canonical episodes, so the four lists at the top are the
    /// same input under four schemes — the property that proves the strategy is not canonical
    /// sorting with a scheme label.
    fn golden_cases() -> Vec<Golden> {
        const DVD: OrderingScheme = OrderingScheme::Dvd;
        const ABS: OrderingScheme = OrderingScheme::Absolute;
        const ANIME: OrderingScheme = OrderingScheme::Anime;
        vec![
            // Aired: the canonical columns, season then episode. Season 0 sorts first, so the
            // special leads even though it aired after episode 4.
            golden(OrderingScheme::Aired, vec![], vec![5, 1, 2, 3, 4], vec![]),
            // DVD: Firefly's discs put episode 1 (aired 1x01) third and open with the episode Fox
            // aired second. The special is the last disc.
            golden(
                DVD,
                vec![
                    row(1, DVD, 1, 3),
                    row(2, DVD, 1, 1),
                    row(3, DVD, 1, 2),
                    row(4, DVD, 1, 4),
                    row(5, DVD, 1, 5),
                ],
                vec![2, 3, 1, 4, 5],
                vec![],
            ),
            // Absolute: one running number for the series, so the special that aired after
            // episode 4 is number 5 and the aired order's season-0-first rule does not apply.
            golden(
                ABS,
                vec![
                    row(1, ABS, 1, 1),
                    row(2, ABS, 1, 2),
                    row(3, ABS, 1, 3),
                    row(4, ABS, 1, 4),
                    row(5, ABS, 1, 5),
                ],
                vec![1, 2, 3, 4, 5],
                vec![],
            ),
            // Anime: the broadcast run, where the source numbered the recap third, between two
            // numbered episodes. This is the mixing the scheme exists for.
            golden(
                ANIME,
                vec![
                    row(1, ANIME, 1, 1),
                    row(2, ANIME, 1, 2),
                    row(5, ANIME, 1, 3),
                    row(3, ANIME, 1, 4),
                    row(4, ANIME, 1, 5),
                ],
                vec![1, 2, 5, 3, 4],
                vec![],
            ),
            // Anime, unnumbered special: no number in the run, so no place in the run.
            golden(
                ANIME,
                vec![
                    row(1, ANIME, 1, 1),
                    row(2, ANIME, 1, 2),
                    row(3, ANIME, 1, 3),
                    row(4, ANIME, 1, 4),
                ],
                vec![1, 2, 3, 4],
                vec![(5, UnplacedReason::UnnumberedSpecial)],
            ),
            // Anime, unnumbered standard episode: the ordinary missing-number case, while the
            // special beside it has a number and takes its place at the end of the run.
            golden(
                ANIME,
                vec![
                    row(1, ANIME, 1, 1),
                    row(2, ANIME, 1, 2),
                    row(4, ANIME, 1, 4),
                    row(5, ANIME, 1, 5),
                ],
                vec![1, 2, 4, 5],
                vec![(3, UnplacedReason::NoSchemeNumber { scheme: ANIME })],
            ),
            // A title the provider never gave a disc order for: every episode is reported, and
            // none is silently ranked at its aired number instead.
            golden(
                DVD,
                vec![],
                vec![],
                vec![
                    (1, UnplacedReason::NoSchemeNumber { scheme: DVD }),
                    (2, UnplacedReason::NoSchemeNumber { scheme: DVD }),
                    (3, UnplacedReason::NoSchemeNumber { scheme: DVD }),
                    (4, UnplacedReason::NoSchemeNumber { scheme: DVD }),
                    (5, UnplacedReason::NoSchemeNumber { scheme: DVD }),
                ],
            ),
        ]
    }

    /// Each case above, run over the one series they all describe.
    #[test]
    fn a_golden_table_reproduces_each_schemes_expected_list() {
        let episodes = series();
        let table = golden_cases();
        assert_eq!(table.len(), 7, "the table is the whole golden set");
        for case in &table {
            let ordered = EpisodeOrderingStrategy::order(case.scheme, &episodes, &case.rows);
            assert_eq!(ids(&ordered), case.expected, "{:?}", case.scheme);
            assert_eq!(refused(&ordered), case.refused, "{:?}", case.scheme);
            for placement in ordered.placements() {
                let source = episodes
                    .iter()
                    .find(|episode| episode.id == placement.episode_id)
                    .expect("every placement names an episode that was handed in");
                assert_eq!(
                    placement.canonical,
                    source.position(),
                    "an overlay renumbers the display, never the episode"
                );
                assert!(
                    placement
                        .overlay
                        .is_none_or(|overlay| overlay.scheme == case.scheme),
                    "a placement carries no row from another scheme"
                );
                if case.scheme != OrderingScheme::Aired {
                    assert!(
                        placement.overlay.is_some(),
                        "a non-aired scheme places nothing without its own row"
                    );
                }
            }
        }
    }

    #[test]
    fn a_row_for_another_scheme_is_not_a_number_for_this_one() {
        let episodes = [episode(1, 1, 1, None)];
        // The account is on the absolute scheme, so the repository read only these rows and the
        // disk-order list has no number for the episode.
        let absolutes = [row(1, OrderingScheme::Absolute, 1, 9)];

        let dvd = EpisodeOrderingStrategy::order(OrderingScheme::Dvd, &episodes, &absolutes);
        assert_eq!(ids(&dvd).len(), 0);
        assert_eq!(
            refused(&dvd),
            [(
                1,
                UnplacedReason::NoSchemeNumber {
                    scheme: OrderingScheme::Dvd
                }
            )]
        );

        // Aired places it canonically, and the foreign row is not carried into the list either:
        // whatever ranks this placement, `EpisodePlacement::position` must report the same
        // number the strategy sorted by.
        let aired = EpisodeOrderingStrategy::order(OrderingScheme::Aired, &episodes, &absolutes);
        assert_eq!(ids(&aired), [1]);
        assert_eq!(aired.placements()[0].position(), episodes[0].position());
        assert!(aired.placements()[0].overlay.is_none());
    }

    #[test]
    fn an_aired_row_is_trusted_exactly_as_a_position_is() {
        // §2.3 makes an `aired` row redundant, and this is the one case where honouring it
        // matters: `EpisodePlacement::position` prefers it, so a strategy that ignored it would
        // return a list `next_episode` ranks in a different order than the strategy sorted.
        let episodes = [episode(1, 1, 1, None), episode(2, 1, 2, None)];
        let rows = [
            row(2, OrderingScheme::Aired, 1, 1),
            row(1, OrderingScheme::Aired, 1, 2),
        ];
        let ordered = EpisodeOrderingStrategy::order(OrderingScheme::Aired, &episodes, &rows);
        assert_eq!(ids(&ordered), [2, 1]);
        assert_eq!(ordered.placements()[0].position(), rows[0].position());
    }

    #[test]
    fn a_special_is_recognised_by_its_type_or_by_its_season() {
        // Providers disagree about which marker they set: one types the special, another parks
        // it in season 0 and types nothing. Both are specials, and anime refuses both without a
        // number.
        let episodes = [
            episode(1, 1, 1, None),
            episode(2, 1, 8, Some(EpisodeType::Special)),
            episode(3, 0, 3, None),
        ];
        let rows = [row(1, OrderingScheme::Anime, 1, 1)];
        let ordered = EpisodeOrderingStrategy::order(OrderingScheme::Anime, &episodes, &rows);
        assert_eq!(ids(&ordered), [1]);
        assert_eq!(
            refused(&ordered),
            [
                (2, UnplacedReason::UnnumberedSpecial),
                (3, UnplacedReason::UnnumberedSpecial),
            ]
        );
    }

    #[test]
    fn an_empty_series_has_an_empty_list_and_nothing_to_report() {
        for scheme in OrderingScheme::ALL {
            let ordered = EpisodeOrderingStrategy::order(*scheme, &[], &[]);
            assert_eq!(ordered.placements().len(), 0, "{scheme:?}");
            assert_eq!(ordered.unplaced().len(), 0, "{scheme:?}");
        }
    }

    #[test]
    fn a_single_episode_is_placed_or_reported_on_its_own() {
        let episodes = [episode(1, 1, 1, None)];
        let aired = EpisodeOrderingStrategy::order(OrderingScheme::Aired, &episodes, &[]);
        assert_eq!(ids(&aired), [1], "one episode is its own list");
        assert_eq!(aired.unplaced().len(), 0);

        let anime = EpisodeOrderingStrategy::order(OrderingScheme::Anime, &episodes, &[]);
        assert_eq!(ids(&anime).len(), 0);
        assert_eq!(
            refused(&anime),
            [(
                1,
                UnplacedReason::NoSchemeNumber {
                    scheme: OrderingScheme::Anime
                }
            )]
        );
    }

    #[test]
    fn two_episodes_contending_for_one_position_are_both_kept_in_a_fixed_order() {
        let contended = [
            row(9, OrderingScheme::Dvd, 1, 2),
            row(4, OrderingScheme::Dvd, 1, 2),
        ];
        let first = [episode(9, 1, 9, None), episode(4, 1, 4, None)];
        let second = [episode(4, 1, 4, None), episode(9, 1, 9, None)];

        let ordered = EpisodeOrderingStrategy::order(OrderingScheme::Dvd, &first, &contended);
        assert_eq!(
            ids(&ordered),
            [4, 9],
            "both stay in the list; the episode id decides, not the read order"
        );
        assert_eq!(ordered.unplaced().len(), 0);
        assert_eq!(
            ordered,
            EpisodeOrderingStrategy::order(OrderingScheme::Dvd, &second, &contended),
            "the caller's query order must not decide the display order"
        );
    }

    #[test]
    fn a_second_row_for_one_episode_does_not_depend_on_the_read_order() {
        // `episode_ordering`'s key is (episode_id, scheme), so this is a broken key. One of the
        // two rows has to win, and it has to be the same one for both callers.
        let episodes = [episode(1, 1, 1, None)];
        let forward = [
            row(1, OrderingScheme::Dvd, 1, 3),
            row(1, OrderingScheme::Dvd, 1, 7),
        ];
        let backward = [
            row(1, OrderingScheme::Dvd, 1, 7),
            row(1, OrderingScheme::Dvd, 1, 3),
        ];
        let first = EpisodeOrderingStrategy::order(OrderingScheme::Dvd, &episodes, &forward);
        let second = EpisodeOrderingStrategy::order(OrderingScheme::Dvd, &episodes, &backward);
        assert_eq!(first, second);
        assert_eq!(first.placements()[0].position().episode_number.get(), 7);
    }

    #[test]
    fn a_row_for_an_episode_outside_the_list_is_ignored() {
        let episodes = [episode(1, 1, 1, None)];
        let rows = [
            row(1, OrderingScheme::Dvd, 1, 1),
            row(99, OrderingScheme::Dvd, 1, 2),
        ];
        let ordered = EpisodeOrderingStrategy::order(OrderingScheme::Dvd, &episodes, &rows);
        assert_eq!(ids(&ordered), [1]);
        assert_eq!(
            ordered.unplaced().len(),
            0,
            "the absent episode is not the caller's problem here"
        );
    }

    #[test]
    fn next_episode_follows_the_list_this_strategy_returns() {
        // The gap this module fills: `next_episode` ranks a slice it assumes is already ordered
        // and never compares episode numbers itself, so an unordered season is what picks the
        // wrong episode. The DVD order below is 1x01 = episode 2, 1x02 = episode 3, 1x03 = 1.
        let episodes = [
            episode(1, 1, 1, None),
            episode(2, 1, 2, None),
            episode(3, 1, 3, None),
        ];
        let rows = [
            row(1, OrderingScheme::Dvd, 1, 3),
            row(2, OrderingScheme::Dvd, 1, 1),
            row(3, OrderingScheme::Dvd, 1, 2),
        ];
        let ordered = EpisodeOrderingStrategy::order(OrderingScheme::Dvd, &episodes, &rows);
        let playing = ordered.placements()[0];
        assert_eq!(playing.episode_id, 2);

        let next = next_episode(ordered.placements(), playing.position());
        assert_eq!(next.map(|placement| placement.episode_id), Some(3));
        let last = *ordered.placements().last().expect("three episodes");
        assert_eq!(last.episode_id, 1);
        assert_eq!(next_episode(ordered.placements(), last.position()), None);
    }

    #[test]
    fn the_default_scheme_is_aired_because_it_needs_no_rows() {
        assert_eq!(DEFAULT_SCHEME, OrderingScheme::Aired);
        let episodes = [episode(1, 1, 1, None), episode(2, 1, 2, None)];
        let ordered = EpisodeOrderingStrategy::order(DEFAULT_SCHEME, &episodes, &[]);
        assert_eq!(
            ids(&ordered),
            [1, 2],
            "the default is total with no overlay read"
        );
    }

    fn kind_strategy() -> impl Strategy<Value = Option<EpisodeType>> {
        prop::sample::select(vec![
            None,
            Some(EpisodeType::Standard),
            Some(EpisodeType::Special),
            Some(EpisodeType::Finale),
        ])
    }

    /// A series with unique ids, so a returned placement can be traced back to its episode.
    fn series_strategy() -> impl Strategy<Value = Vec<Episode>> {
        prop::collection::vec((0_u32..4, 0_u32..4, kind_strategy()), 0..6).prop_map(|parts| {
            parts
                .into_iter()
                .enumerate()
                .map(|(index, (season, number, kind))| {
                    episode(
                        i64::try_from(index).expect("a small index"),
                        season,
                        number,
                        kind,
                    )
                })
                .collect()
        })
    }

    fn scheme_strategy() -> impl Strategy<Value = OrderingScheme> {
        prop::sample::select(OrderingScheme::ALL.to_vec())
    }

    /// Rows for episodes in and out of the series, for any scheme, with duplicates allowed: the
    /// key is `(episode_id, scheme)` and the generator is allowed to break it.
    fn rows_strategy() -> impl Strategy<Value = Vec<EpisodeOrdering>> {
        prop::collection::vec((0_i64..8, scheme_strategy(), 0_u32..4, 0_u32..4), 0..8).prop_map(
            |rows| {
                rows.into_iter()
                    .map(|(episode_id, scheme, season, number)| {
                        row(episode_id, scheme, season, number)
                    })
                    .collect()
            },
        )
    }

    proptest! {
        /// Totality: nothing handed in disappears. Every episode is either placed once or
        /// reported once, which is the failure this module exists to prevent.
        #[test]
        fn every_episode_is_either_placed_or_reported(
            episodes in series_strategy(),
            scheme in scheme_strategy(),
            rows in rows_strategy(),
        ) {
            let ordered = EpisodeOrderingStrategy::order(scheme, &episodes, &rows);
            prop_assert_eq!(
                ordered.placements().len() + ordered.unplaced().len(),
                episodes.len()
            );

            let mut seen: Vec<i64> = ordered
                .placements()
                .iter()
                .map(|placement| placement.episode_id)
                .chain(ordered.unplaced().iter().map(|episode| episode.episode_id))
                .collect();
            seen.sort_unstable();
            let mut given: Vec<i64> = episodes.iter().map(|episode| episode.id).collect();
            given.sort_unstable();
            prop_assert_eq!(seen, given);
        }

        /// A pure function of its inputs: the same series and rows always give the same list.
        #[test]
        fn the_same_inputs_always_order_the_same(
            episodes in series_strategy(),
            scheme in scheme_strategy(),
            rows in rows_strategy(),
        ) {
            prop_assert_eq!(
                EpisodeOrderingStrategy::order(scheme, &episodes, &rows),
                EpisodeOrderingStrategy::order(scheme, &episodes, &rows)
            );
        }

        /// Determinism over the caller's read order: a keyset-paged query and a cache may hand
        /// the same set over in any order and the display order must not move.
        #[test]
        fn the_caller_read_order_never_decides_the_list(
            episodes in series_strategy(),
            scheme in scheme_strategy(),
            rows in rows_strategy(),
        ) {
            let forward = EpisodeOrderingStrategy::order(scheme, &episodes, &rows);
            let mut reversed_episodes = episodes.clone();
            reversed_episodes.reverse();
            let mut reversed_rows = rows.clone();
            reversed_rows.reverse();
            prop_assert_eq!(
                forward,
                EpisodeOrderingStrategy::order(scheme, &reversed_episodes, &reversed_rows)
            );
        }

        /// The overlay renumbers the display, never the episode: a placed episode carries the
        /// canonical position it was handed, and only the active scheme's row beside it.
        #[test]
        fn no_canonical_position_is_ever_rewritten(
            episodes in series_strategy(),
            scheme in scheme_strategy(),
            rows in rows_strategy(),
        ) {
            let ordered = EpisodeOrderingStrategy::order(scheme, &episodes, &rows);
            for placement in ordered.placements() {
                let source = episodes
                    .iter()
                    .find(|episode| episode.id == placement.episode_id)
                    .expect("a placement comes from an episode that was handed in");
                prop_assert_eq!(placement.canonical, source.position());
                prop_assert!(placement.overlay.is_none_or(|overlay| overlay.scheme == scheme));
            }
        }

        /// The list is in the scheme's order, and no placement contradicts the row it was ranked
        /// by: the key is exactly `EpisodePlacement::position`, which is what `next_episode` reads.
        #[test]
        fn the_list_is_sorted_by_the_scheme_position(
            episodes in series_strategy(),
            scheme in scheme_strategy(),
            rows in rows_strategy(),
        ) {
            let ordered = EpisodeOrderingStrategy::order(scheme, &episodes, &rows);
            let positions: Vec<_> = ordered
                .placements()
                .iter()
                .map(EpisodePlacement::position)
                .collect();
            prop_assert!(positions.windows(2).all(|pair| pair[0] <= pair[1]));
            for placement in ordered.placements() {
                if let Some(overlay) = placement.overlay {
                    prop_assert_eq!(placement.position(), overlay.position());
                }
            }
            prop_assert!(
                ordered
                    .unplaced()
                    .windows(2)
                    .all(|pair| pair[0].episode_id <= pair[1].episode_id)
            );
        }

        /// The rule that separates the schemes: a scheme other than `aired` places nothing
        /// without a row of its own, so it can never fall back to the aired number.
        #[test]
        fn a_non_aired_scheme_never_falls_back_to_the_aired_number(
            episodes in series_strategy(),
            scheme in scheme_strategy(),
            rows in rows_strategy(),
        ) {
            prop_assume!(scheme != OrderingScheme::Aired);
            let ordered = EpisodeOrderingStrategy::order(scheme, &episodes, &rows);
            for placement in ordered.placements() {
                prop_assert!(placement.overlay.is_some_and(|overlay| overlay.scheme == scheme));
            }
            for episode in ordered.unplaced() {
                prop_assert!(
                    !matches!(episode.reason, UnplacedReason::UnnumberedSpecial)
                        || scheme == OrderingScheme::Anime,
                    "only anime refuses an unnumbered special"
                );
            }
        }
    }
}
