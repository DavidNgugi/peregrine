//! Does a release name name the audio language the account prefers?
//!
//! A release title is the one place a language is stated before a byte is fetched, and it is
//! written for machines rather than people: separators vary, tags are abbreviated inconsistently
//! (`en`, `eng`, `english`, `vff`, `vostfr`), and a release that names no language at all is
//! common. The rank is deliberately coarse because it only orders candidates the quality rules
//! have already accepted; it is not a parser and it never rejects a release.
//!
//! - [`RANK_MATCH`] names the preferred language, or says `MULTi` / `DUAL`.
//! - [`RANK_UNNAMED`] names no language token this build recognises.
//! - [`RANK_OTHER`] names a different explicit language.
//!
//! A re-rank is stable (see the acquire job), so two releases that tie keep the quality ranker's
//! order.

/// The release names the preferred language, or carries more than one audio language.
pub const RANK_MATCH: u8 = 0;
/// The release names no language this build recognises.
pub const RANK_UNNAMED: u8 = 1;
/// The release names a different explicit language.
pub const RANK_OTHER: u8 = 2;

/// The canonical family a token belongs to, when the token names a language.
///
/// English, French, German, Spanish, Italian, Portuguese, Japanese, Korean, Chinese, Russian,
/// Arabic, and Hindi are the languages this build's providers license; the aliases are the ones
/// scene releases actually write, including the French `VF` / `VOSTFR` vocabulary.
fn family(token: &str) -> Option<&'static str> {
    match token {
        "en" | "eng" | "english" => Some("en"),
        "fr" | "fre" | "fra" | "french" | "vf" | "vff" | "vfi" | "truefrench" | "vostfr" => {
            Some("fr")
        }
        "de" | "ger" | "deu" | "german" => Some("de"),
        "es" | "spa" | "esp" | "spanish" | "latino" | "castellano" => Some("es"),
        "it" | "ita" | "italian" => Some("it"),
        "pt" | "por" | "portuguese" | "ptbr" | "dublado" => Some("pt"),
        "ja" | "jpn" | "japanese" => Some("ja"),
        "ko" | "kor" | "korean" => Some("ko"),
        "zh" | "chi" | "zho" | "chinese" | "mandarin" | "cantonese" => Some("zh"),
        "ru" | "rus" | "russian" => Some("ru"),
        "ar" | "ara" | "arabic" => Some("ar"),
        "hi" | "hin" | "hindi" => Some("hi"),
        _ => None,
    }
}

/// Whether a token says the release carries more than one audio language.
fn is_multi(token: &str) -> bool {
    matches!(token, "multi" | "dual")
}

/// The lower-case ASCII words in a name, split on every other character.
///
/// Separators are punctuation, spaces, and underscores alike, so `TrueFrench`, `TRUEFRENCH`,
/// `true.french` and `true_french` all reach the matcher as words. Non-ASCII text is a separator
/// rather than a word: a tag this build accepts is ASCII, and a title in another script must not
/// be mistaken for one.
fn tokens(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_ascii_lowercase)
}

/// Ranks a release name against one preferred audio language.
///
/// The answer is always one of [`RANK_MATCH`], [`RANK_UNNAMED`], or [`RANK_OTHER`]. The preferred
/// tag may be abbreviated or spelled out (`en`, `eng`, `english`); a tag this build does not know
/// still matches itself literally, which is what keeps an uncommon language usable.
#[must_use]
pub fn rank(release_name: &str, preferred: &str) -> u8 {
    let preferred: Vec<String> = tokens(preferred).collect();
    let preferred_families: Vec<&'static str> =
        preferred.iter().filter_map(|tag| family(tag)).collect();
    let mut named_other = false;
    for token in tokens(release_name) {
        if is_multi(&token) {
            return RANK_MATCH;
        }
        match family(&token) {
            Some(found) if preferred_families.contains(&found) => return RANK_MATCH,
            Some(_) => named_other = true,
            None if preferred.contains(&token) => return RANK_MATCH,
            None => {}
        }
    }
    if named_other {
        RANK_OTHER
    } else {
        RANK_UNNAMED
    }
}

#[cfg(test)]
mod tests {
    use super::{RANK_MATCH, RANK_OTHER, RANK_UNNAMED, rank};
    use proptest::prelude::*;

    #[test]
    fn a_release_naming_the_preferred_language_wins() {
        for (name, preferred) in [
            ("A.Film.2024.1080p.BluRay.ENG.x264", "en"),
            ("A Film 2024 2160p ENGLISH DTS", "en"),
            ("A.Film.2024.1080p.FRENCH.x265", "fr"),
            ("A.Film.2024.TRUEFRENCH.1080p", "fr"),
            ("A.Film.2024.VOSTFR.1080p", "fr"),
            ("A.Film.2024.VFF.1080p", "fr"),
            ("A.Film.2024.GERMAN.720p", "de"),
            ("A.Film.2024.SPANISH.720p", "es"),
            ("A.Film.2024.ITA.720p", "it"),
            ("A.Film.2024.PORTUGUESE.720p", "pt"),
            ("A.Film.2024.JPN.720p", "ja"),
            ("A.Film.2024.KOR.720p", "ko"),
            ("A.Film.2024.CHINESE.720p", "zh"),
            ("A.Film.2024.RUSSIAN.720p", "ru"),
            ("A.Film.2024.ARABIC.720p", "ar"),
            ("A.Film.2024.HINDI.720p", "hi"),
        ] {
            assert_eq!(rank(name, preferred), RANK_MATCH, "{name} / {preferred}");
        }
    }

    #[test]
    fn a_multi_or_dual_release_is_a_match_for_any_preference() {
        assert_eq!(rank("A.Film.2024.MULTI.1080p", "de"), RANK_MATCH);
        assert_eq!(rank("A.Film.2024.DUAL.1080p", "ja"), RANK_MATCH);
        assert_eq!(rank("A.Film.2024.MULTi.1080p", "fr"), RANK_MATCH);
    }

    #[test]
    fn a_release_that_names_no_language_is_unnamed() {
        assert_eq!(
            rank("A.Film.2024.1080p.BluRay.x264-GROUP", "en"),
            RANK_UNNAMED
        );
        assert_eq!(rank("", "en"), RANK_UNNAMED);
        assert_eq!(rank("1080p", ""), RANK_UNNAMED);
    }

    #[test]
    fn a_release_naming_another_language_is_ranked_last() {
        assert_eq!(rank("A.Film.2024.GERMAN.1080p", "en"), RANK_OTHER);
        assert_eq!(rank("A.Film.2024.VOSTFR.1080p", "en"), RANK_OTHER);
        assert_eq!(rank("A.Film.2024.ENG.1080p", "fr"), RANK_OTHER);
    }

    #[test]
    fn case_and_separators_do_not_matter() {
        assert_eq!(rank("A_FILM_2024_Eng_1080p", "EN"), RANK_MATCH);
        assert_eq!(rank("a-film-2024-truefrench-1080p", "fr"), RANK_MATCH);
        assert_eq!(rank("A.FILM.2024.MULTI.1080p", "en"), RANK_MATCH);
    }

    #[test]
    fn an_uncommon_preferred_tag_still_matches_itself() {
        assert_eq!(rank("A.Film.2024.SW.1080p", "sw"), RANK_MATCH);
        assert_eq!(rank("A.Film.2024.SW.1080p", "en"), RANK_UNNAMED);
    }

    proptest! {
        #[test]
        fn the_rank_is_always_one_of_the_three(name in ".*", preferred in ".*") {
            let ranked = rank(&name, &preferred);
            prop_assert!(ranked <= RANK_OTHER);
        }

        #[test]
        fn the_rank_never_panics_on_arbitrary_unicode(
            name in proptest::collection::vec(any::<char>(), 0..200),
            preferred in proptest::collection::vec(any::<char>(), 0..20),
        ) {
            let name: String = name.into_iter().collect();
            let preferred: String = preferred.into_iter().collect();
            let _ = rank(&name, &preferred);
        }
    }
}
