//! External identities and torrent info-hashes.
//!
//! An [`ExternalId`] is the pair `(provider, value)` that the `external_id`
//! table stores, with the canonical text form `provider:value` used wherever a
//! single string is needed. An [`InfoHash`] is a torrent's twenty-byte identity,
//! which `download_job.info_hash` stores and `download_job_hash_uq` keeps unique
//! among live jobs. Construction is the only way into either, so an unknown
//! provider, an empty value, or a malformed hash cannot become a value.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A provider that identifies titles, people, seasons, and episodes.
// The variants mirror the `external_id.provider` CHECK in `DATA_MODEL.md` §2.2;
// a test asserts the two sets are equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// The Movie Database.
    Tmdb,
    /// `IMDb`.
    Imdb,
    /// `TheTVDB`.
    Tvdb,
    /// `AniDB`.
    Anidb,
    /// `MyAnimeList`.
    Mal,
    /// Trakt.
    Trakt,
    /// Wikidata.
    Wikidata,
}

impl Provider {
    /// Every provider, in the order the database constraint lists them.
    pub const ALL: [Self; 7] = [
        Self::Tmdb,
        Self::Imdb,
        Self::Tvdb,
        Self::Anidb,
        Self::Mal,
        Self::Trakt,
        Self::Wikidata,
    ];

    /// The lowercase prefix used in an id string and in the database.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tmdb => "tmdb",
            Self::Imdb => "imdb",
            Self::Tvdb => "tvdb",
            Self::Anidb => "anidb",
            Self::Mal => "mal",
            Self::Trakt => "trakt",
            Self::Wikidata => "wikidata",
        }
    }
}

impl FromStr for Provider {
    type Err = ExternalIdError;

    fn from_str(prefix: &str) -> Result<Self, Self::Err> {
        // Prefixes are accepted in any case because they arrive from user
        // input, but the value is always the lowercase database form.
        Self::ALL
            .into_iter()
            .find(|provider| prefix.eq_ignore_ascii_case(provider.as_str()))
            .ok_or_else(|| ExternalIdError::UnknownProvider {
                prefix: prefix.to_owned(),
            })
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A provider plus that provider's own identifier for one entity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ExternalId {
    provider: Provider,
    value: String,
}

impl ExternalId {
    /// Builds an id from a provider and a non-empty provider value.
    pub fn new(provider: Provider, value: &str) -> Result<Self, ExternalIdError> {
        if value.is_empty() {
            return Err(ExternalIdError::EmptyValue { provider });
        }
        if let Some(invalid) = value
            .chars()
            .find(|c| !(c.is_ascii_alphanumeric() || *c == '-' || *c == '_'))
        {
            return Err(ExternalIdError::InvalidValue { provider, invalid });
        }
        Ok(Self {
            provider,
            value: value.to_owned(),
        })
    }

    /// Parses the canonical `provider:value` text form.
    pub fn parse(input: &str) -> Result<Self, ExternalIdError> {
        if input.is_empty() {
            return Err(ExternalIdError::Empty);
        }
        let Some((prefix, value)) = input.split_once(':') else {
            return Err(ExternalIdError::MissingSeparator {
                input: input.to_owned(),
            });
        };
        let provider = Provider::from_str(prefix)?;
        if value.contains(':') {
            return Err(ExternalIdError::ExtraSeparator { provider });
        }
        Self::new(provider, value)
    }

    /// The provider that owns this identifier.
    pub const fn provider(&self) -> Provider {
        self.provider
    }

    /// The provider's own identifier value.
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl fmt::Display for ExternalId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.provider, self.value)
    }
}

impl FromStr for ExternalId {
    type Err = ExternalIdError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input)
    }
}

impl TryFrom<String> for ExternalId {
    type Error = ExternalIdError;

    fn try_from(input: String) -> Result<Self, Self::Error> {
        Self::parse(&input)
    }
}

impl From<ExternalId> for String {
    fn from(id: ExternalId) -> Self {
        id.to_string()
    }
}

/// Why an [`ExternalId`] could not be built.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExternalIdError {
    /// The input string was empty.
    #[error("external id is empty")]
    Empty,
    /// No `:` separated the provider from the value.
    #[error("external id `{input}` has no `:` between the provider and the value")]
    MissingSeparator {
        /// The rejected input.
        input: String,
    },
    /// The prefix is not one of the known providers.
    #[error(
        "unknown provider prefix `{prefix}`; expected one of tmdb, imdb, tvdb, anidb, mal, trakt, wikidata"
    )]
    UnknownProvider {
        /// The unrecognized prefix.
        prefix: String,
    },
    /// The value after the `:` was empty.
    #[error("`{provider}` id has an empty value after `:`")]
    EmptyValue {
        /// The provider whose value was empty.
        provider: Provider,
    },
    /// The value contained a second `:`, so it is not a single value.
    #[error("`{provider}` id contains more than one `:`")]
    ExtraSeparator {
        /// The provider the value was being built for.
        provider: Provider,
    },
    /// The value contained a character no provider value uses.
    #[error("`{provider}` id contains {invalid:?}; values are ASCII letters, digits, `-`, and `_`")]
    InvalidValue {
        /// The provider the value was being built for.
        provider: Provider,
        /// The first character that is not allowed.
        invalid: char,
    },
}

/// The characters a `BitTorrent` v1 info-hash is spelled in.
const INFO_HASH_HEX_LENGTH: usize = 40;

/// The twenty-byte identity of one torrent.
///
/// `download_job.info_hash` stores it, `download_job_hash_uq` keeps it unique among live jobs,
/// and PG-E12-06 de-duplicates indexer results on it, so it is domain vocabulary and no adapter
/// owns it (AGENTS R8). Construction is the only way in: twenty raw bytes, or the forty
/// hexadecimal characters a `btih` spells in either case.
///
/// **`Debug` deliberately prints none of the hash, and there is no `Display`.** A bare 40-hex
/// run and a `magnet:` prefix are forbidden shapes in text — [`crate::redaction::check_text`]
/// refuses both with [`crate::redaction::RedactionError::Magnet`] — because a validated
/// infohash is "not retained, logged, or emitted" (`PG-E1-11`, SECURITY T13). Closing the
/// accidental path in the type rather than at each call site is the whole point: the only way
/// to spell the hash out is [`InfoHash::to_hex`], which a caller reaches for on purpose when it
/// is writing the column the value came from.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InfoHash([u8; 20]);

impl InfoHash {
    /// Wraps the twenty bytes a `BitTorrent` info dictionary hashes to.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 20]) -> Self {
        Self(bytes)
    }

    /// Parses the forty hexadecimal characters a `btih` spells, in either case.
    ///
    /// # Errors
    ///
    /// [`InfoHashError::WrongLength`] when the text is not exactly forty characters, and
    /// [`InfoHashError::NotHex`] naming the position of the first character that is not a hex
    /// digit.
    pub fn from_hex(text: &str) -> Result<Self, InfoHashError> {
        let characters = text.chars().count();
        if characters != INFO_HASH_HEX_LENGTH {
            return Err(InfoHashError::WrongLength { characters });
        }
        let mut characters = text.chars();
        let mut bytes = [0_u8; 20];
        for (index, byte) in bytes.iter_mut().enumerate() {
            let high = nibble(&mut characters, index * 2)?;
            let low = nibble(&mut characters, index * 2 + 1)?;
            *byte = (high << 4) | low;
        }
        Ok(Self(bytes))
    }

    /// The twenty bytes, for the engine and for hashing.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }

    /// The lowercase hex spelling `download_job.info_hash` stores.
    ///
    /// Lowercase rather than as given, because this is the canonical form the unique index
    /// compares: two spellings of one torrent must not be two rows.
    #[must_use]
    pub fn to_hex(&self) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut hex = String::with_capacity(INFO_HASH_HEX_LENGTH);
        for byte in self.0 {
            // Both indices are provably in range: a nibble is four bits.
            hex.push(char::from(DIGITS[usize::from(byte >> 4)]));
            hex.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
        }
        hex
    }
}

impl fmt::Debug for InfoHash {
    /// Prints the type and nothing of the value.
    ///
    /// A derive would put forty hex characters into every log line, panic message, and assertion
    /// that formatted one — the shape [`crate::redaction`] exists to keep out of user-visible
    /// text (SECURITY T13). The length is deliberately not printed either: the only infohash this
    /// type holds is forty characters, so the length says nothing a reader needs.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("InfoHash(..)")
    }
}

/// One hex digit, or the error naming the position that was not one.
fn nibble(characters: &mut std::str::Chars<'_>, position: usize) -> Result<u8, InfoHashError> {
    characters
        .next()
        .and_then(hex_value)
        .ok_or(InfoHashError::NotHex { position })
}

/// The value of one ASCII hex digit, or `None` for anything else.
///
/// `char::to_digit` knows only ASCII letters and digits, which is what keeps a fullwidth `２` or
/// any other confusable digit out of a value the unique index compares.
fn hex_value(character: char) -> Option<u8> {
    character
        .to_digit(16)
        .and_then(|value| u8::try_from(value).ok())
}

/// Why hex text is not an [`InfoHash`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum InfoHashError {
    /// The text was not exactly forty characters.
    #[error("an info-hash is {INFO_HASH_HEX_LENGTH} hexadecimal characters, not {characters}")]
    WrongLength {
        /// How many characters the rejected text had.
        characters: usize,
    },
    /// A character in the text is not a hex digit.
    ///
    /// It names the position rather than the character: one character of a hash is a fragment of
    /// the value this type exists to keep out of a message, and a position is enough to find it.
    #[error("the info-hash has a character that is not a hex digit, at position {position}")]
    NotHex {
        /// The zero-based character position of the first character that is not a hex digit.
        position: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const PROVIDERS: [Provider; 7] = Provider::ALL;

    fn provider_strategy() -> impl Strategy<Value = Provider> {
        prop::sample::select(PROVIDERS.to_vec())
    }

    #[test]
    fn provider_strings_match_the_external_id_check_constraint() {
        let documented = ["tmdb", "imdb", "tvdb", "anidb", "mal", "trakt", "wikidata"];
        let actual: Vec<&str> = Provider::ALL.into_iter().map(Provider::as_str).collect();
        assert_eq!(documented.as_slice(), actual.as_slice());
    }

    #[test]
    fn provider_round_trips_through_its_prefix_regardless_of_case() {
        for provider in Provider::ALL {
            assert_eq!(Provider::from_str(provider.as_str()), Ok(provider));
            assert_eq!(
                Provider::from_str(&provider.as_str().to_uppercase()),
                Ok(provider)
            );
        }
    }

    #[test]
    fn provider_rejects_an_unknown_prefix() {
        assert_eq!(
            Provider::from_str("nonsense"),
            Err(ExternalIdError::UnknownProvider {
                prefix: "nonsense".to_owned()
            })
        );
    }

    #[test]
    fn provider_rejects_surrounding_whitespace() {
        assert!(Provider::from_str(" tmdb").is_err());
        assert!(Provider::from_str("tmdb ").is_err());
    }

    #[test]
    fn external_id_formats_and_parses_its_canonical_text_form() {
        let id = ExternalId::new(Provider::Imdb, "tt0111161").expect("valid id");
        assert_eq!(id.to_string(), "imdb:tt0111161");
        assert_eq!(ExternalId::parse("imdb:tt0111161"), Ok(id.clone()));
        assert_eq!(id.provider(), Provider::Imdb);
        assert_eq!(id.value(), "tt0111161");
    }

    #[test]
    fn external_id_normalizes_an_uppercase_prefix() {
        assert_eq!(
            ExternalId::parse("TMDB:278").map(|id| id.to_string()),
            Ok("tmdb:278".to_owned())
        );
    }

    #[test]
    fn external_id_rejects_empty_input() {
        assert_eq!(ExternalId::parse(""), Err(ExternalIdError::Empty));
    }

    #[test]
    fn external_id_rejects_a_missing_separator() {
        assert_eq!(
            ExternalId::parse("tmdb"),
            Err(ExternalIdError::MissingSeparator {
                input: "tmdb".to_owned()
            })
        );
    }

    #[test]
    fn external_id_rejects_a_missing_value() {
        assert_eq!(
            ExternalId::parse("tmdb:"),
            Err(ExternalIdError::EmptyValue {
                provider: Provider::Tmdb
            })
        );
    }

    #[test]
    fn external_id_rejects_extra_separators() {
        assert_eq!(
            ExternalId::parse("tmdb:278:extra"),
            Err(ExternalIdError::ExtraSeparator {
                provider: Provider::Tmdb
            })
        );
    }

    #[test]
    fn external_id_rejects_an_unknown_prefix() {
        assert!(matches!(
            ExternalId::parse("nonsense:1"),
            Err(ExternalIdError::UnknownProvider { .. })
        ));
    }

    #[test]
    fn external_id_rejects_non_ascii_values() {
        assert_eq!(
            ExternalId::parse("tmdb:２７８"),
            Err(ExternalIdError::InvalidValue {
                provider: Provider::Tmdb,
                invalid: '２'
            })
        );
    }

    #[test]
    fn external_id_rejects_whitespace_inside_a_value() {
        assert!(matches!(
            ExternalId::parse("tmdb:27 8"),
            Err(ExternalIdError::InvalidValue { .. })
        ));
    }

    #[test]
    fn external_id_accepts_trakt_slugs() {
        let id = ExternalId::new(Provider::Trakt, "the-matrix-1999").expect("valid slug");
        assert_eq!(id.to_string(), "trakt:the-matrix-1999");
    }

    #[test]
    fn external_id_survives_a_very_long_value() {
        let value = "a".repeat(100_000);
        let id = ExternalId::new(Provider::Tmdb, &value).expect("no length limit is imposed");
        assert_eq!(id.value().len(), 100_000);
    }

    #[test]
    fn external_id_round_trips_through_json() {
        let id = ExternalId::new(Provider::Wikidata, "Q42").expect("valid id");
        let json = serde_json::to_string(&id).expect("serializes");
        assert_eq!(json, "\"wikidata:Q42\"");
        let parsed: ExternalId = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(parsed, id);
    }

    #[test]
    fn external_id_json_rejects_a_malformed_string() {
        assert!(serde_json::from_str::<ExternalId>("\"tmdb:\"").is_err());
    }

    proptest! {
        #[test]
        fn external_id_round_trips_through_its_text_form(
            provider in provider_strategy(),
            value in "[A-Za-z0-9_-]{1,32}",
        ) {
            let id = ExternalId::new(provider, &value).expect("generated value is valid");
            let text = id.to_string();
            prop_assert_eq!(ExternalId::parse(&text), Ok(id));
        }

        #[test]
        fn external_id_parse_never_panics_on_arbitrary_input(input in any::<String>()) {
            let _ = ExternalId::parse(&input);
        }
    }

    /// A validated hash, in the lowercase spelling the stored column holds.
    const HASH_HEX: &str = "0123456789abcdef0123456789abcdef01234567";

    fn hash() -> InfoHash {
        InfoHash::from_hex(HASH_HEX).expect("the fixture is a valid hash")
    }

    #[test]
    fn an_info_hash_round_trips_through_its_hex_spelling() {
        let hash = hash();
        assert_eq!(hash.to_hex(), HASH_HEX);
        assert_eq!(
            InfoHash::from_hex(&hash.to_hex()).expect("its own spelling parses"),
            hash
        );
        assert_eq!(hash.as_bytes().len(), 20);
        assert_eq!(hash.as_bytes()[0], 0x01);
        assert_eq!(hash.as_bytes()[19], 0x67);
    }

    #[test]
    fn an_info_hash_round_trips_through_its_bytes() {
        let bytes = [0xab_u8; 20];
        let hash = InfoHash::from_bytes(bytes);
        assert_eq!(hash.as_bytes(), &bytes);
        assert_eq!(hash.to_hex(), "ab".repeat(20));
        assert_eq!(
            InfoHash::from_hex(&hash.to_hex()).expect("the spelling of those bytes parses"),
            hash
        );
    }

    #[test]
    fn an_info_hash_parses_either_case_and_spells_one_canonical_form() {
        let lower = InfoHash::from_hex(HASH_HEX).expect("valid");
        let upper = InfoHash::from_hex(&HASH_HEX.to_uppercase()).expect("valid in any case");
        let mixed = InfoHash::from_hex("0123456789ABCDEF0123456789abcdef01234567").expect("valid");

        assert_eq!(lower, upper);
        assert_eq!(lower, mixed);
        assert_eq!(upper.to_hex(), HASH_HEX);
        assert_eq!(mixed.to_hex(), HASH_HEX, "the stored form is one spelling");
    }

    #[test]
    fn an_info_hash_that_is_not_forty_characters_is_refused() {
        assert_eq!(
            InfoHash::from_hex(""),
            Err(InfoHashError::WrongLength { characters: 0 })
        );
        assert_eq!(
            InfoHash::from_hex(&"0".repeat(39)),
            Err(InfoHashError::WrongLength { characters: 39 })
        );
        assert_eq!(
            InfoHash::from_hex(&"0".repeat(41)),
            Err(InfoHashError::WrongLength { characters: 41 })
        );
        // A leading and a trailing space are characters like any other: a hash read out of a
        // configuration file has to be exactly the hash, not a trimmed guess at one.
        assert_eq!(
            InfoHash::from_hex(&format!(" {HASH_HEX}")),
            Err(InfoHashError::WrongLength { characters: 41 })
        );
    }

    #[test]
    fn an_info_hash_with_a_non_hex_character_names_the_position() {
        assert_eq!(
            InfoHash::from_hex(&"g".repeat(40)),
            Err(InfoHashError::NotHex { position: 0 })
        );
        let last_wrong = format!("{}z", &HASH_HEX[..39]);
        assert_eq!(
            InfoHash::from_hex(&last_wrong),
            Err(InfoHashError::NotHex { position: 39 })
        );
        let middle_wrong = format!("{}-{}", &HASH_HEX[..20], &HASH_HEX[21..]);
        assert_eq!(
            InfoHash::from_hex(&middle_wrong),
            Err(InfoHashError::NotHex { position: 20 })
        );
        // A fullwidth digit is a character, not a hex digit: `char::to_digit` knows only ASCII,
        // so no lookalike reaches the value the unique index compares.
        let fullwidth = format!("２{}", &HASH_HEX[1..]);
        assert_eq!(
            InfoHash::from_hex(&fullwidth),
            Err(InfoHashError::NotHex { position: 0 })
        );
    }

    #[test]
    fn an_info_hash_error_names_no_part_of_the_hash() {
        // The messages are what a caller or a log line shows; a wrong-length or non-hex refusal
        // must not quote the value that was refused.
        let wrong_length = InfoHashError::WrongLength { characters: 39 };
        let not_hex = InfoHashError::NotHex { position: 20 };

        assert_eq!(
            wrong_length.to_string(),
            "an info-hash is 40 hexadecimal characters, not 39"
        );
        assert_eq!(
            not_hex.to_string(),
            "the info-hash has a character that is not a hex digit, at position 20"
        );
    }

    #[test]
    fn an_info_hash_never_reaches_a_log_line_or_a_message() {
        // SECURITY T13 and PG-E1-11: the accidental path is closed by the type. `Debug` prints
        // nothing of the value, and the hex spelling is refused by the leak scanner every
        // user-visible string passes through.
        use crate::redaction::{RedactionError, check_text};

        let hash = hash();
        let debug = format!("{hash:?}");

        assert_eq!(debug, "InfoHash(..)");
        assert!(
            !debug.contains(&hash.to_hex()),
            "a debug line carried the hash: {debug}"
        );
        assert_eq!(check_text(&hash.to_hex(), 512), Err(RedactionError::Magnet));
    }

    proptest! {
        #[test]
        fn an_info_hash_round_trips_through_hex_for_any_bytes(bytes in any::<[u8; 20]>()) {
            let hash = InfoHash::from_bytes(bytes);
            prop_assert_eq!(
                InfoHash::from_hex(&hash.to_hex()).expect("its own spelling parses"),
                hash
            );
        }

        #[test]
        fn info_hash_parse_never_panics_on_arbitrary_input(input in any::<String>()) {
            let _ = InfoHash::from_hex(&input);
        }
    }
}
