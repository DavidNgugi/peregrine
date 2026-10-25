//! Integrations: the external services a person can turn on, order, and credential.
//!
//! **This module owns the vocabulary, not the catalogue.** Which integrations this build ships is
//! a composition-root fact and lives in `peregrine-app`'s manifest; what a category, a capability,
//! a stored preference, and a health outcome *mean* is domain vocabulary that the settings screen,
//! the marker resolver, and the backup path all have to agree on.
//!
//! Three rules are enforced here rather than at a call site:
//!
//! - An [`IntegrationId`] is a stable, lower-case name. It is the key the settings row and every
//!   keyring entry are stored under, so changing its spelling silently orphans a person's
//!   credentials.
//! - A preference's `priority` is installation-level and per category; playback preferences are
//!   per profile and are not this type (ADR 0032).
//! - `submission_enabled` is off unless a person turned it on. Reading public timestamps and
//!   contributing new ones are different consents, and a provider that allows both must not infer
//!   the second from the first.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::stored_enum;

stored_enum! {
    /// What an integration is for, and therefore where its card sits in Settings.
    IntegrationCategory {
        /// Title, season, episode, and artwork metadata.
        Metadata => "metadata",
        /// Intro, recap, credits, and preview windows.
        SkipMarkers => "skipMarkers",
        /// Subtitle search and download.
        Subtitles => "subtitles",
        /// Watch-history scrobbling.
        Tracking => "tracking",
        /// Indexers and download sources.
        Acquisition => "acquisition",
    }
}

stored_enum! {
    /// One thing an integration can be asked for.
    ///
    /// A provider declares these; a caller reads them before asking, and a request for something
    /// that was never declared is refused rather than attempted. The set is deliberately small:
    /// it is the set of operations this build actually performs, not a description of each
    /// service's whole public API.
    IntegrationCapability {
        /// Search and enrich catalogue rows.
        Metadata => "metadata",
        /// Read skip markers.
        Markers => "markers",
        /// Search and download subtitles.
        Subtitles => "subtitles",
        /// Report playback progress to a tracking service.
        Scrobble => "scrobble",
        /// Search releases or talk to a download source.
        Acquisition => "acquisition",
    }
}

stored_enum! {
    /// What the last bounded probe of an integration answered.
    ///
    /// This is the health vocabulary the settings screen and the read-only TV status share. It is
    /// **not** the request error vocabulary: `ProviderError` is the adapter's closed set, and this
    /// is the one sentence a person can act on.
    IntegrationOutcome {
        /// The probe reached the service and it answered.
        Working => "working",
        /// The service refused the credential.
        Unauthorized => "unauthorized",
        /// The service asked to be left alone for a while.
        RateLimited => "rateLimited",
        /// The service could not be reached, or answered something unusable.
        Unreachable => "unreachable",
        /// The OS refused to read the keyring, so a credential may exist but cannot be used.
        KeychainDenied => "keychainDenied",
        /// No credential is stored and the integration needs one.
        NotConfigured => "notConfigured",
    }
}

impl IntegrationOutcome {
    /// Whether this outcome is a failure a person has to do something about.
    ///
    /// `NotConfigured` is the first-run state and `Working` is the goal; only the middle four are
    /// attention states, and the settings screen draws them differently for that reason.
    #[must_use]
    pub const fn needs_attention(self) -> bool {
        matches!(
            self,
            Self::Unauthorized | Self::RateLimited | Self::Unreachable | Self::KeychainDenied
        )
    }
}

/// A stable name for one integration.
///
/// The value is stored in `integration_setting.integration`, forms the keyring account name
/// `integration.<id>`, and is what an IPC caller names. It is lower-case ASCII, begins with a
/// letter, and may contain digits and hyphens — the same shape every registry entry and every
/// keyring account already uses.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct IntegrationId(String);

impl IntegrationId {
    /// The longest accepted id, in bytes.
    pub const MAX_LENGTH: usize = 32;

    /// Builds an id, refusing anything that could not round-trip through storage and a keyring.
    ///
    /// # Errors
    ///
    /// [`IntegrationIdError::Empty`], [`IntegrationIdError::TooLong`],
    /// [`IntegrationIdError::InvalidCharacter`], or [`IntegrationIdError::InvalidStart`].
    pub fn new(value: &str) -> Result<Self, IntegrationIdError> {
        if value.is_empty() {
            return Err(IntegrationIdError::Empty);
        }
        if value.len() > Self::MAX_LENGTH {
            return Err(IntegrationIdError::TooLong {
                length: value.len(),
            });
        }
        let mut characters = value.chars();
        let Some(first) = characters.next() else {
            return Err(IntegrationIdError::Empty);
        };
        if !first.is_ascii_lowercase() {
            return Err(IntegrationIdError::InvalidStart { character: first });
        }
        if let Some(character) = characters.find(|character| {
            !(character.is_ascii_lowercase() || character.is_ascii_digit() || *character == '-')
        }) {
            return Err(IntegrationIdError::InvalidCharacter { character });
        }
        Ok(Self(value.to_owned()))
    }

    /// The stored spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for IntegrationId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for IntegrationId {
    type Error = IntegrationIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl From<IntegrationId> for String {
    fn from(id: IntegrationId) -> Self {
        id.0
    }
}

/// Why an integration name was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum IntegrationIdError {
    /// The name was empty.
    #[error("an integration id cannot be empty")]
    Empty,
    /// The name was longer than [`IntegrationId::MAX_LENGTH`].
    #[error("an integration id is at most {MAX} bytes, not {length}", MAX = IntegrationId::MAX_LENGTH)]
    TooLong {
        /// The refused length.
        length: usize,
    },
    /// The first character was not a lower-case ASCII letter.
    #[error("an integration id starts with a lower-case letter, not {character:?}")]
    InvalidStart {
        /// The refused first character.
        character: char,
    },
    /// A character outside the accepted set.
    #[error(
        "an integration id contains {character:?}; use lower-case letters, digits, and hyphens"
    )]
    InvalidCharacter {
        /// The refused character.
        character: char,
    },
}

/// One installation-level integration choice.
///
/// One row per integration, whether it is enabled, its order within its category (lower runs
/// first), and whether this person has consented to contributing timestamps. Absence of a row is
/// the default, which is why [`Self::default_for`] is the only constructor a first run needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationPreference {
    /// Which integration this is.
    pub id: IntegrationId,
    /// Whether the integration may be asked at all.
    pub enabled: bool,
    /// Its order within its category; lower is earlier.
    pub priority: u32,
    /// Whether it may receive contributed data. Off unless a person turns it on.
    pub submission_enabled: bool,
}

impl IntegrationPreference {
    /// The state a never-configured integration starts in.
    ///
    /// Disabled rather than enabled: a network integration that starts on its own would be an
    /// egress nobody asked for (AGENTS R23, P8). Local sources are enabled by their manifest
    /// entry not by this default, so this constructor is the same for every id.
    #[must_use]
    pub const fn default_for(id: IntegrationId, priority: u32) -> Self {
        Self {
            id,
            enabled: false,
            priority,
            submission_enabled: false,
        }
    }

    /// The state a manifest entry that is always available starts in.
    ///
    /// Only a source with no network and no credential — local chapters — uses this, and it is a
    /// separate constructor so "on by default" is a visible decision at the manifest entry rather
    /// than a property every integration inherits.
    #[must_use]
    pub const fn always_on(id: IntegrationId, priority: u32) -> Self {
        Self {
            id,
            enabled: true,
            priority,
            submission_enabled: false,
        }
    }
}

/// Orders two preferences the way the resolver and the settings screen both read them.
///
/// Priority first, then the stable id, so two entries that share a priority still have one order
/// on every render and in every test. The function is total and the tie-break is not the
/// insertion order, which is what makes a settings screen's order survive a restart.
#[must_use]
pub fn order(left: &IntegrationPreference, right: &IntegrationPreference) -> std::cmp::Ordering {
    left.priority
        .cmp(&right.priority)
        .then_with(|| left.id.cmp(&right.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_accepts_the_shape_every_registry_entry_uses() {
        for value in ["tmdb", "theintrodb", "aniskip", "open-subtitles2", "a1"] {
            assert_eq!(
                IntegrationId::new(value)
                    .as_ref()
                    .map(IntegrationId::as_str),
                Ok(value),
                "{value} should be a valid id"
            );
        }
    }

    #[test]
    fn an_id_refuses_a_name_that_could_not_be_a_keyring_account() {
        assert_eq!(IntegrationId::new(""), Err(IntegrationIdError::Empty));
        assert_eq!(
            IntegrationId::new("TheIntroDB"),
            Err(IntegrationIdError::InvalidStart { character: 'T' })
        );
        assert_eq!(
            IntegrationId::new("2fast"),
            Err(IntegrationIdError::InvalidStart { character: '2' })
        );
        assert_eq!(
            IntegrationId::new("the_intro_db"),
            Err(IntegrationIdError::InvalidCharacter { character: '_' })
        );
        assert_eq!(
            IntegrationId::new("the intro db"),
            Err(IntegrationIdError::InvalidCharacter { character: ' ' })
        );
        assert_eq!(
            IntegrationId::new("é"),
            Err(IntegrationIdError::InvalidStart { character: 'é' })
        );
    }

    #[test]
    fn an_id_is_bounded_in_bytes() {
        let longest = "a".repeat(IntegrationId::MAX_LENGTH);
        assert!(IntegrationId::new(&longest).is_ok());
        assert_eq!(
            IntegrationId::new(&"a".repeat(IntegrationId::MAX_LENGTH + 1)),
            Err(IntegrationIdError::TooLong {
                length: IntegrationId::MAX_LENGTH + 1
            })
        );
    }

    #[test]
    fn an_id_round_trips_through_json_as_a_plain_string() {
        let id = IntegrationId::new("theintrodb").unwrap();
        let text = serde_json::to_string(&id).unwrap();

        assert_eq!(text, "\"theintrodb\"");
        assert_eq!(serde_json::from_str::<IntegrationId>(&text).unwrap(), id);
        assert!(serde_json::from_str::<IntegrationId>("\"TheIntroDB\"").is_err());
    }

    #[test]
    fn the_default_is_disabled_and_declines_submission() {
        let preference =
            IntegrationPreference::default_for(IntegrationId::new("theintrodb").unwrap(), 0);

        assert!(!preference.enabled);
        assert!(!preference.submission_enabled);
    }

    #[test]
    fn an_always_on_source_is_enabled_but_still_declines_submission() {
        let preference =
            IntegrationPreference::always_on(IntegrationId::new("chapters").unwrap(), 0);

        assert!(preference.enabled);
        assert!(
            !preference.submission_enabled,
            "a source with nothing to submit to cannot inherit consent"
        );
    }

    #[test]
    fn ordering_is_priority_then_id_and_never_insertion_order() {
        let mut entries = vec![
            IntegrationPreference::default_for(IntegrationId::new("b").unwrap(), 20),
            IntegrationPreference::default_for(IntegrationId::new("c").unwrap(), 10),
            IntegrationPreference::default_for(IntegrationId::new("a").unwrap(), 20),
        ];
        entries.sort_by(order);

        let ids: Vec<&str> = entries.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(ids, ["c", "a", "b"]);
    }

    #[test]
    fn only_the_four_failure_outcomes_need_attention() {
        assert!(!IntegrationOutcome::Working.needs_attention());
        assert!(!IntegrationOutcome::NotConfigured.needs_attention());
        assert!(IntegrationOutcome::Unauthorized.needs_attention());
        assert!(IntegrationOutcome::RateLimited.needs_attention());
        assert!(IntegrationOutcome::Unreachable.needs_attention());
        assert!(IntegrationOutcome::KeychainDenied.needs_attention());
    }

    #[test]
    fn every_stored_vocabulary_has_the_spelling_its_check_constraint_names() {
        assert_eq!(
            IntegrationCategory::ALL
                .iter()
                .map(|value| value.as_str())
                .collect::<Vec<_>>(),
            [
                "metadata",
                "skipMarkers",
                "subtitles",
                "tracking",
                "acquisition"
            ]
        );
        assert_eq!(
            IntegrationOutcome::ALL
                .iter()
                .map(|value| value.as_str())
                .collect::<Vec<_>>(),
            [
                "working",
                "unauthorized",
                "rateLimited",
                "unreachable",
                "keychainDenied",
                "notConfigured"
            ]
        );
    }
}
