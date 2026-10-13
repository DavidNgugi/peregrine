//! The title matching rule: which local title a provider's candidate turns out to be.
//!
//! A provider answers a search with titles *it* knows; the catalogue holds titles *this* application
//! has. Deciding that the two are the same title is the step between a provider's answer and a row on
//! screen (`PG-E5-13`), and it is a rule rather than a lookup because the identifying facts disagree
//! more often than they agree: a provider's name carries punctuation the local one does not, a remake
//! shares a name with its original, and a title is often listed under several names at once.
//!
//! Three facts, in the order that settles the question:
//!
//! 1. **An external id is identity.** If a local title already carries the candidate's `(provider,
//!    value)`, that is the title — whatever either of them is called. This is why `external_id` exists.
//! 2. **A name and a year are evidence.** Among local titles of the same *kind* whose name (or one of
//!    whose aliases) reads the same, the one released in the candidate's year is the title. A name
//!    that matches with **no** year matching is a remake, and a remake is a different film: the answer
//!    is that this is a new title, not that this is the old one.
//! 3. **Anything else is new**, and anything **ambiguous is reported as ambiguous** rather than
//!    guessed. Two local titles with one name and no year to separate them is a question a person
//!    answers, and the wrong answer merges two titles or splits one — both of which lose data.
//!
//! Names are compared after [`sort_name`]: trimmed, with every run of whitespace collapsed to one
//! space, and lowercased. Punctuation is **not** stripped, because `Fast & Furious` and `Fast and
//! Furious` are different rows until a person says otherwise, and a rule that folded them would merge
//! two films nobody asked it to merge.

use jiff::civil::Date;

use crate::catalogue::TitleKind;
use crate::id::ExternalId;
use crate::text::NonEmptyText;

/// A title a provider offered, as the rule sees it.
///
/// Deliberately not `peregrine_providers::SearchCandidate`: that type lives in the adapter crate, which
/// depends on this one, and a rule that spoke the adapter's vocabulary could not be tested without it.
/// The layer that owns both (the sync service) is what translates one into the other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The provider's own identifier for the title.
    pub external_id: ExternalId,
    /// The name the provider lists it under.
    pub name: NonEmptyText,
    /// A film or a series.
    pub kind: TitleKind,
    /// The release date, or the first air date for a series, when the provider knows it.
    pub released: Option<Date>,
}

/// A title the catalogue already holds, as the rule sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalTitle {
    /// The row id, which is what a decision names.
    pub id: i64,
    /// A film or a series.
    pub kind: TitleKind,
    /// The name this catalogue stores.
    pub name: NonEmptyText,
    /// Its release date, or first air date, when the catalogue has one.
    pub released: Option<Date>,
    /// Every external id already attached to it.
    pub external_ids: Vec<ExternalId>,
    /// The other names it is known by, from `title_alias`.
    pub aliases: Vec<NonEmptyText>,
}

/// What settled a match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchBasis {
    /// The candidate's external id was already attached to the title.
    ExternalId,
    /// The name and the year agreed.
    NameAndYear,
    /// The name agreed, and the candidate had no year to check. The weakest of the three, and the
    /// reason a caller may want to record which basis a row was matched on.
    Name,
}

/// What the rule decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchDecision {
    /// This candidate is this local title.
    Matched {
        /// The local row.
        local_id: i64,
        /// What settled it.
        basis: MatchBasis,
    },
    /// Nothing local is this title: it is new to the catalogue.
    New,
    /// Several local titles fit equally well, so none of them is the answer. The ids are sorted, so
    /// one catalogue always yields one decision and a test can compare them.
    Ambiguous {
        /// The local rows that fit equally well, ascending.
        local_ids: Vec<i64>,
    },
}

/// The form two names are compared in: trimmed, whitespace collapsed, lowercased, and stripped of the
/// invisible spacing characters.
///
/// This is the one normalization in the application — the library indexer stores a title's `sort_name`
/// with the same rule, and both go through here so a name typed by a person, read from a filename, and
/// returned by a provider cannot differ in a way that matters. Takes any string: an all-whitespace
/// input has no sort name and answers with an empty one, which the callers that *store* a sort name
/// never pass, because their input is a parsed title.
///
/// **Zero-width characters are removed and joiners are kept.** U+200B, U+2060, and U+FEFF are invisible
/// spacing: a title carrying one looks identical to the same title without it and would never match,
/// which is the unicode-spoofing hazard AGENTS §4 lists. U+200C and U+200D are *not* removed, because
/// Arabic and several Indic scripts use them inside words to mean something, and stripping those would
/// change a name rather than clean it.
///
/// **What this does not do, and it is worth knowing:** no Unicode normalization and no locale-aware
/// casing. `İSTANBUL` does not fold to `istanbul` (Turkish dotted I is a different letter under the
/// default casing rules), `STRASSE` does not fold to `Straße`, and a full-width spelling does not fold
/// to its ASCII one. Each is two names a person reads as one, and each needs a normalization decision
/// that belongs to a task of its own rather than to a first version of this rule.
#[must_use]
pub fn sort_name(name: &str) -> String {
    let visible: String = name
        .chars()
        .filter(|character| !is_invisible_spacing(*character))
        .collect();
    visible
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Whether a character is invisible spacing that carries no meaning inside a name.
const fn is_invisible_spacing(character: char) -> bool {
    matches!(character, '\u{200b}' | '\u{2060}' | '\u{feff}')
}

/// Decides which local title `candidate` is.
///
/// See the module documentation for the rule and the order it is applied in.
#[must_use]
pub fn match_candidate(candidate: &Candidate, locals: &[LocalTitle]) -> MatchDecision {
    // The same row twice is one row. A caller reading `title JOIN title_alias` emits a title once per
    // alias, and counting those as several candidates would report an ambiguity that is not there —
    // while the same duplicate *within* one title is already treated as decidable.
    let mut seen = std::collections::BTreeSet::new();
    let locals: Vec<&LocalTitle> = locals
        .iter()
        .filter(|local| seen.insert(local.id))
        .collect();
    // 1. Identity. An id already attached to a local title settles it even when the names have nothing
    //    in common, which is the point of recording ids at all.
    let claiming: Vec<i64> = locals
        .iter()
        .filter(|local| local.external_ids.contains(&candidate.external_id))
        .map(|local| local.id)
        .collect();
    match claiming.as_slice() {
        [only] => {
            return MatchDecision::Matched {
                local_id: *only,
                basis: MatchBasis::ExternalId,
            };
        }
        [] => {}
        // Two rows claiming one provider id is a data fault, and guessing which is right would hide it.
        many => return ambiguous(many),
    }

    // 2. Evidence. Only the same kind can match: a film and a series that share a name are two titles.
    let wanted = sort_name(candidate.name.as_str());
    let named: Vec<&LocalTitle> = locals
        .iter()
        .copied()
        .filter(|local| local.kind == candidate.kind && reads_as(local, &wanted))
        .collect();

    match (candidate.released, named.as_slice()) {
        (_, []) => MatchDecision::New,
        (Some(date), found) => {
            let same_year: Vec<i64> = found
                .iter()
                .filter(|local| local.released.map(jiff::civil::Date::year) == Some(date.year()))
                .map(|local| local.id)
                .collect();
            // A *known* year that differs is the remake. A year nobody recorded is not evidence of
            // anything, so it cannot disqualify a name — treating it as if it could made the rule
            // asymmetric: a candidate with no year matched a local with one, while a local with no year
            // could never match a candidate with one, and that second direction is how the library's own
            // stubs (indexed from a filename that stated no year) would be duplicated by the first sync
            // that found them at a provider.
            let known_different_year = found.iter().any(|local| {
                local
                    .released
                    .is_some_and(|released| released.year() != date.year())
            });
            match same_year.as_slice() {
                [only] => MatchDecision::Matched {
                    local_id: *only,
                    basis: MatchBasis::NameAndYear,
                },
                [] if known_different_year => MatchDecision::New,
                [] if found.len() == 1 => MatchDecision::Matched {
                    local_id: found[0].id,
                    basis: MatchBasis::Name,
                },
                [] => ambiguous(&found.iter().map(|local| local.id).collect::<Vec<_>>()),
                many => ambiguous(many),
            }
        }
        // With no year to separate them, one candidate name is one local title or a question.
        (None, [only]) => MatchDecision::Matched {
            local_id: only.id,
            basis: MatchBasis::Name,
        },
        (None, found) => ambiguous(&found.iter().map(|local| local.id).collect::<Vec<_>>()),
    }
}

/// Whether a local title reads as `wanted`, by its own name or by one of its aliases.
fn reads_as(local: &LocalTitle, wanted: &str) -> bool {
    sort_name(local.name.as_str()) == wanted
        || local
            .aliases
            .iter()
            .any(|alias| sort_name(alias.as_str()) == wanted)
}

/// An ambiguity, with its ids in the one order every catalogue agrees on.
fn ambiguous(ids: &[i64]) -> MatchDecision {
    let mut sorted = ids.to_vec();
    sorted.sort_unstable();
    MatchDecision::Ambiguous { local_ids: sorted }
}

#[cfg(test)]
mod tests {
    use super::{Candidate, LocalTitle, MatchBasis, MatchDecision, match_candidate, sort_name};
    use crate::catalogue::TitleKind;
    use crate::id::ExternalId;
    use crate::text::NonEmptyText;

    fn name(value: &str) -> NonEmptyText {
        NonEmptyText::new(value).expect("a non-empty name")
    }

    fn id(provider: crate::id::Provider, value: &str) -> ExternalId {
        ExternalId::new(provider, value).expect("a valid external id")
    }

    fn candidate(value: &str, released: Option<&str>) -> Candidate {
        Candidate {
            external_id: id(crate::id::Provider::Tmdb, value),
            name: name("The Matrix"),
            kind: TitleKind::Movie,
            released: released.map(|date| date.parse().expect("a date")),
        }
    }

    fn local(id: i64, name_of: &str, released: Option<&str>) -> LocalTitle {
        LocalTitle {
            id,
            kind: TitleKind::Movie,
            name: name(name_of),
            released: released.map(|date| date.parse().expect("a date")),
            external_ids: Vec::new(),
            aliases: Vec::new(),
        }
    }

    #[test]
    fn an_external_id_is_identity_whatever_either_side_is_called() {
        // The whole point of recording ids: a provider's name for a title and this catalogue's name
        // for it can differ completely, and the id still says they are one row.
        let mut held = local(7, "Matrix, The", None);
        held.external_ids = vec![id(crate::id::Provider::Tmdb, "603")];

        assert_eq!(
            match_candidate(&candidate("603", None), &[held]),
            MatchDecision::Matched {
                local_id: 7,
                basis: MatchBasis::ExternalId
            }
        );
    }

    #[test]
    fn two_titles_claiming_one_id_is_reported_rather_than_guessed() {
        let mut first = local(1, "The Matrix", None);
        first.external_ids = vec![id(crate::id::Provider::Tmdb, "603")];
        let mut second = local(2, "The Matrix", None);
        second.external_ids = vec![id(crate::id::Provider::Tmdb, "603")];

        assert_eq!(
            match_candidate(&candidate("603", None), &[second, first]),
            MatchDecision::Ambiguous {
                local_ids: vec![1, 2]
            }
        );
    }

    #[test]
    fn a_name_and_a_year_are_evidence() {
        let held = local(3, "The Matrix", Some("1999-03-31"));

        assert_eq!(
            match_candidate(&candidate("603", Some("1999-03-31")), &[held]),
            MatchDecision::Matched {
                local_id: 3,
                basis: MatchBasis::NameAndYear
            }
        );
    }

    #[test]
    fn a_remake_is_a_different_title() {
        // The same name and a different year is the one case where the conservative answer is also the
        // correct one: filing a remake onto its original would silently merge two films.
        let held = local(3, "The Matrix", Some("1999-03-31"));

        assert_eq!(
            match_candidate(&candidate("999", Some("2029-01-01")), &[held]),
            MatchDecision::New
        );
    }

    #[test]
    fn a_name_without_a_year_matches_when_only_one_title_reads_that_way() {
        let held = local(3, "The Matrix", None);

        assert_eq!(
            match_candidate(&candidate("603", None), &[held]),
            MatchDecision::Matched {
                local_id: 3,
                basis: MatchBasis::Name
            }
        );
    }

    #[test]
    fn two_titles_with_one_name_and_no_year_are_a_question() {
        let first = local(1, "The Matrix", None);
        let second = local(2, "The Matrix", None);

        assert_eq!(
            match_candidate(&candidate("603", None), &[first, second]),
            MatchDecision::Ambiguous {
                local_ids: vec![1, 2]
            }
        );
    }

    #[test]
    fn two_titles_with_one_name_are_separated_by_the_year() {
        let original = local(1, "The Matrix", Some("1999-03-31"));
        let remake = local(2, "The Matrix", Some("2029-01-01"));

        assert_eq!(
            match_candidate(&candidate("603", Some("2029-05-05")), &[original, remake]),
            MatchDecision::Matched {
                local_id: 2,
                basis: MatchBasis::NameAndYear
            }
        );
    }

    #[test]
    fn an_alias_is_a_name_the_title_is_also_known_by() {
        let mut held = local(4, "Matrix, The", None);
        held.aliases = vec![name("The Matrix")];

        assert_eq!(
            match_candidate(&candidate("603", None), &[held]),
            MatchDecision::Matched {
                local_id: 4,
                basis: MatchBasis::Name
            }
        );
    }

    #[test]
    fn a_different_kind_never_matches_by_name() {
        // A film and a series can share a name. Merging them would put episodes under a film.
        let mut show = local(5, "The Matrix", None);
        show.kind = TitleKind::Show;

        assert_eq!(
            match_candidate(&candidate("603", None), &[show]),
            MatchDecision::New
        );
    }

    #[test]
    fn a_name_that_matches_by_alias_and_by_name_in_the_same_year_is_a_question() {
        let mut aliased = local(1, "Matrix, The", Some("1999-03-31"));
        aliased.aliases = vec![name("The Matrix")];
        let named = local(2, "The Matrix", Some("1999-03-31"));

        assert_eq!(
            match_candidate(&candidate("603", Some("1999-03-31")), &[aliased, named]),
            MatchDecision::Ambiguous {
                local_ids: vec![1, 2]
            }
        );
    }

    #[test]
    fn nothing_local_is_a_new_title() {
        assert_eq!(
            match_candidate(&candidate("603", Some("1999-03-31")), &[]),
            MatchDecision::New
        );
    }

    #[test]
    fn the_one_normalization_drops_case_and_whitespace_but_not_punctuation() {
        assert_eq!(sort_name("  The   Matrix "), "the matrix");
        assert_eq!(sort_name("THE MATRIX"), "the matrix");
        assert_eq!(sort_name("The Matrix!"), "the matrix!");
        assert_ne!(sort_name("Fast & Furious"), sort_name("Fast and Furious"));
        assert_eq!(sort_name("七人の侍"), "七人の侍");
    }

    #[test]
    fn an_invisible_space_does_not_hide_one_name_from_another() {
        // The hostile case §4 lists: a zero-width space makes one title look like another and would
        // never match, which is a duplicate nobody can see the cause of.
        assert_eq!(sort_name("The\u{200b} Matrix"), sort_name("The Matrix"));
        assert_eq!(sort_name("\u{feff}Alien"), "alien");
    }

    #[test]
    fn a_joiner_is_kept_because_a_script_uses_it_to_mean_something() {
        // U+200C and U+200D are not spacing: Arabic and several Indic scripts write them inside words.
        assert_ne!(sort_name("a\u{200d}b"), sort_name("ab"));
    }

    #[test]
    fn a_local_whose_year_nobody_recorded_does_not_disqualify_its_name() {
        // The first version of this rule answered `New` here, which is how a library's own stub —
        // indexed from a filename that stated no year — would be duplicated by the first sync that
        // found the title at a provider. Only a *known* different year is a remake.
        let held = local(3, "The Matrix", None);

        assert_eq!(
            match_candidate(&candidate("603", Some("1999-03-31")), &[held]),
            MatchDecision::Matched {
                local_id: 3,
                basis: MatchBasis::Name
            }
        );
    }

    #[test]
    fn two_rows_with_one_id_are_one_title() {
        // A caller reading `title JOIN title_alias` emits a title once per alias. Counting those as
        // several candidates would report an ambiguity that is not there.
        let row = local(3, "The Matrix", Some("1999-03-31"));

        assert_eq!(
            match_candidate(&candidate("603", Some("1999-03-31")), &[row.clone(), row]),
            MatchDecision::Matched {
                local_id: 3,
                basis: MatchBasis::NameAndYear
            }
        );
    }
}
