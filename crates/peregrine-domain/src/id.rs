//! External identities.
//!
//! An [`ExternalId`] is the pair `(provider, value)` that the `external_id`
//! table stores, with the canonical text form `provider:value` used wherever a
//! single string is needed. Construction is the only way in, so an unknown
//! provider or an empty value cannot become a value.

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
}
