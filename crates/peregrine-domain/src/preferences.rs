//! The language preferences a player chooses tracks by.
//!
//! The two tags are BCP-47-ish rather than a closed enum: a provider's subtitle track is labelled
//! with whatever tag it wrote, and a preference that could only name the languages this build
//! happened to know would not be able to choose one. Validation therefore bounds the shape of the
//! tag — trimmed, lower case, two to thirty-five ASCII alphanumerics or hyphens — and leaves the
//! vocabulary open.
//!
//! A [`PlaybackPreference`] can only be built through [`PlaybackPreference::new`], so a value
//! that reached storage is one this build accepted, and a row that cannot be read back is reported
//! rather than silently replaced by a default.

use thiserror::Error;

use crate::stored_enum;

stored_enum! {
    /// How a player with no explicit track choice picks a subtitle track.
    ///
    /// The three values are the ones `playback_preference.subtitles` accepts, and they are the
    /// wire values the web client's contract names.
    SubtitleMode {
        /// The track whose language matches the preference, else the row's own default.
        Auto => "auto",
        /// No subtitle track is shown until the viewer asks for one.
        Off => "off",
        /// The forced narrative track, if the title has one.
        Forced => "forced",
    }
}

/// The shortest language tag this build accepts.
const MIN_TAG_LENGTH: usize = 2;
/// The longest language tag this build accepts.
const MAX_TAG_LENGTH: usize = 35;

/// Why a playback preference was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PlaybackPreferenceError {
    /// A language tag is blank, too short, too long, or carries a character a tag may not.
    #[error("`{value}` is not a language tag")]
    InvalidLanguage {
        /// The value as the caller supplied it.
        value: String,
    },
}

/// The languages and subtitle mode the account prefers for playback.
///
/// Fields are private because the constructor is the validation: a literal could carry a tag no
/// player could match, and a stored row that fails the same rule is a value this build should
/// report rather than reproduce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackPreference {
    audio_language: String,
    subtitle_language: String,
    subtitles: SubtitleMode,
}

impl PlaybackPreference {
    /// Builds a preference, normalising and validating both language tags.
    ///
    /// # Errors
    ///
    /// [`PlaybackPreferenceError::InvalidLanguage`] for a tag outside two to thirty-five ASCII
    /// alphanumerics and hyphens after trimming and lower-casing.
    pub fn new(
        audio_language: &str,
        subtitle_language: &str,
        subtitles: SubtitleMode,
    ) -> Result<Self, PlaybackPreferenceError> {
        Ok(Self {
            audio_language: language_tag(audio_language)?,
            subtitle_language: language_tag(subtitle_language)?,
            subtitles,
        })
    }

    /// The preferred audio language, as a lower-case tag.
    #[must_use]
    pub fn audio_language(&self) -> &str {
        &self.audio_language
    }

    /// The preferred subtitle language, as a lower-case tag.
    #[must_use]
    pub fn subtitle_language(&self) -> &str {
        &self.subtitle_language
    }

    /// How a player should choose a subtitle track by default.
    #[must_use]
    pub const fn subtitles(&self) -> SubtitleMode {
        self.subtitles
    }
}

/// Whether a track's tag names the wanted language, case-insensitively and by prefix.
///
/// A prefix either way makes `en` match a container's `eng` and a provider's `en-US`, without
/// requiring the two sides to agree on how much of the tag they wrote. It is deliberately not a
/// full language table: an unknown tag that shares no prefix does not match, which is the honest
/// answer for a track whose language this build cannot establish.
#[must_use]
pub fn tag_matches(tag: &str, wanted: &str) -> bool {
    let tag = tag.trim().to_ascii_lowercase();
    let wanted = wanted.trim().to_ascii_lowercase();
    !tag.is_empty() && !wanted.is_empty() && (tag.starts_with(&wanted) || wanted.starts_with(&tag))
}

impl Default for PlaybackPreference {
    fn default() -> Self {
        Self {
            audio_language: "en".to_owned(),
            subtitle_language: "en".to_owned(),
            subtitles: SubtitleMode::Auto,
        }
    }
}

/// Trims and lower-cases one tag, refusing a value that is not a tag.
fn language_tag(raw: &str) -> Result<String, PlaybackPreferenceError> {
    let tag = raw.trim().to_ascii_lowercase();
    let shape_ok = (MIN_TAG_LENGTH..=MAX_TAG_LENGTH).contains(&tag.len())
        && tag
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    if shape_ok {
        Ok(tag)
    } else {
        Err(PlaybackPreferenceError::InvalidLanguage {
            value: raw.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{PlaybackPreference, PlaybackPreferenceError, SubtitleMode, tag_matches};

    #[test]
    fn a_tag_matches_by_case_and_prefix() {
        assert!(tag_matches("ENG", "en"));
        assert!(tag_matches("en", "eng"));
        assert!(tag_matches("en-GB", "en"));
        assert!(tag_matches("en", "en-US"));
        assert!(!tag_matches("ita", "en"));
        assert!(!tag_matches("", "en"));
        assert!(!tag_matches("und", ""));
    }

    #[test]
    fn the_default_is_english_english_and_automatic() {
        let preference = PlaybackPreference::default();
        assert_eq!(preference.audio_language(), "en");
        assert_eq!(preference.subtitle_language(), "en");
        assert_eq!(preference.subtitles(), SubtitleMode::Auto);
    }

    #[test]
    fn a_tag_is_trimmed_and_lower_cased() {
        let preference =
            PlaybackPreference::new(" EN-us ", "Fr-CA", SubtitleMode::Forced).expect("valid tags");
        assert_eq!(preference.audio_language(), "en-us");
        assert_eq!(preference.subtitle_language(), "fr-ca");
        assert_eq!(preference.subtitles(), SubtitleMode::Forced);
    }

    #[test]
    fn a_tag_that_is_not_a_tag_is_refused() {
        for bad in ["", " ", "e", "-", "en us", "en_us", "é", "en!"] {
            assert!(
                matches!(
                    PlaybackPreference::new(bad, "en", SubtitleMode::Auto),
                    Err(PlaybackPreferenceError::InvalidLanguage { .. })
                ),
                "`{bad}` must be refused"
            );
        }
        assert!(PlaybackPreference::new("en", &"a".repeat(36), SubtitleMode::Auto).is_err());
        assert!(PlaybackPreference::new("en", &"a".repeat(35), SubtitleMode::Auto).is_ok());
    }

    #[test]
    fn the_stored_spellings_are_the_web_contracts() {
        assert_eq!(SubtitleMode::Auto.as_str(), "auto");
        assert_eq!(SubtitleMode::Off.as_str(), "off");
        assert_eq!(SubtitleMode::Forced.as_str(), "forced");
        for mode in SubtitleMode::ALL {
            assert_eq!(SubtitleMode::parse(mode.as_str()), Some(*mode));
        }
    }
}
