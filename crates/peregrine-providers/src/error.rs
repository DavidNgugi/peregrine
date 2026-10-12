//! Why a provider could not answer.
//!
//! The vocabulary is typed and closed on purpose, and it carries no free text. A transport
//! error from `reqwest` holds the URL it failed on, and TMDB accepts its key as `?api_key=`,
//! so a `String` in this enum is a path by which a credential reaches a log tail or a
//! diagnostics screen the user copies out of the app (SECURITY.md T13). The adapter logs the
//! detail locally, where `peregrine-domain::redaction` can strip it; what travels is this.
//!
//! Every variant names the provider, because a failure the registry reports has to say which
//! one it was: a title with three configured sources and one broken is a different situation
//! from one with a single broken source, and the diagnostics screen draws them differently.

use thiserror::Error;

use peregrine_domain::id::Provider;

/// What went wrong at the provider, in the terms the caller can act on.
///
/// `Clone` because a failure is a value a caller may have to report more than once, and
/// because a test double hands the same scripted failure to every call it answers.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ProviderError {
    /// The provider does not declare the capability that was asked for.
    ///
    /// This is a programming error rather than a provider failure: the caller should have
    /// read the descriptor first, and refusing here is what keeps a rate-limit token from
    /// being spent on a request the provider never claimed it could serve.
    #[error("{provider} does not implement {capability}")]
    Unsupported {
        /// The provider that was asked.
        provider: Provider,
        /// The capability it does not declare, spelled as it is declared.
        capability: &'static str,
    },

    /// The provider answered that the resource does not exist.
    #[error("{provider} has no such title")]
    NotFound {
        /// The provider that was asked.
        provider: Provider,
    },

    /// The provider refused the credentials, or none were supplied.
    #[error("{provider} refused the credentials it was given")]
    Unauthorized {
        /// The provider that refused.
        provider: Provider,
    },

    /// The provider asked to be left alone for a while.
    ///
    /// `retry_after` is what the provider said, and only what it said: where a provider
    /// sends no `Retry-After`, the token bucket's own backoff decides and the caller is told
    /// nothing rather than a number this build made up.
    #[error("{provider} is rate limiting this client")]
    RateLimited {
        /// The provider that refused.
        provider: Provider,
        /// The delay the provider itself asked for, when it named one.
        retry_after: Option<std::time::Duration>,
    },

    /// The answer was not what the provider's own documentation describes.
    ///
    /// It is a state and not an error as far as the user is concerned: the row is marked
    /// stale and keeps rendering from cache (ADR 0007 rule 7).
    #[error("{provider} answered with a shape this build does not recognise")]
    SchemaDrift {
        /// The provider whose answer drifted.
        provider: Provider,
    },

    /// The answer exceeded the cap this port allows a response to reach.
    #[error("{provider} answered with {bytes} bytes, over the {cap} byte cap")]
    TooLarge {
        /// The provider whose answer was oversized.
        provider: Provider,
        /// How large the answer was, as far as it was read.
        bytes: u64,
        /// The cap it exceeded.
        cap: u64,
    },

    /// The request never reached the provider, or the answer never arrived whole.
    #[error("{provider} could not be reached")]
    Transport {
        /// The provider that could not be reached.
        provider: Provider,
        /// What kind of failure it was, without the URL it happened on.
        kind: TransportFailure,
    },
}

/// The kinds of transport failure, as a closed set.
///
/// A closed set is what keeps the URL — and any credential in it — out of the error while
/// still telling the diagnostics screen whether the network is down, the provider is slow,
/// or the answer was cut short.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportFailure {
    /// The connection could not be opened.
    Connect,
    /// The provider did not answer inside the timeout.
    Timeout,
    /// The connection was refused, reset, or closed mid-answer.
    Interrupted,
    /// The answer arrived but could not be decoded.
    Decode,
}

impl ProviderError {
    /// The provider this failure is about.
    ///
    /// The registry and the diagnostics screen group by it, and a variant without one would
    /// be a failure nobody can attribute.
    #[must_use]
    pub const fn provider(&self) -> Provider {
        match self {
            Self::Unsupported { provider, .. }
            | Self::NotFound { provider }
            | Self::Unauthorized { provider }
            | Self::RateLimited { provider, .. }
            | Self::SchemaDrift { provider }
            | Self::TooLarge { provider, .. }
            | Self::Transport { provider, .. } => *provider,
        }
    }

    /// Whether asking again later could succeed.
    ///
    /// This is the UI's retryability flag (AGENTS R1, docs/ARCHITECTURE.md §7 rule 3), and it
    /// is answered here rather than derived from the variant at each call site: a schema drift
    /// is worth retrying eventually and a missing title never is.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. } | Self::Transport { .. } | Self::SchemaDrift { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [ProviderError; 7] = [
        ProviderError::Unsupported {
            provider: Provider::Tmdb,
            capability: "search",
        },
        ProviderError::NotFound {
            provider: Provider::Tmdb,
        },
        ProviderError::Unauthorized {
            provider: Provider::Tmdb,
        },
        ProviderError::RateLimited {
            provider: Provider::Tmdb,
            retry_after: Some(std::time::Duration::from_secs(2)),
        },
        ProviderError::SchemaDrift {
            provider: Provider::Tvdb,
        },
        ProviderError::TooLarge {
            provider: Provider::Tmdb,
            bytes: 5_000_000,
            cap: 1_000_000,
        },
        ProviderError::Transport {
            provider: Provider::Tmdb,
            kind: TransportFailure::Timeout,
        },
    ];

    #[test]
    fn every_failure_names_the_provider_it_is_about() {
        // Not every variant names the same one: a failure the registry attributes to the
        // wrong provider is worse than no attribution at all.
        let named: Vec<Provider> = ALL.iter().map(ProviderError::provider).collect();

        assert_eq!(named[4], Provider::Tvdb);
        assert_eq!(
            named
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != 4)
                .map(|(_, provider)| *provider)
                .collect::<Vec<_>>(),
            vec![Provider::Tmdb; 6]
        );
    }

    #[test]
    fn the_kinds_worth_retrying_say_so_and_the_others_do_not() {
        // The split matters to the user: a missing title that is retried forever is a job
        // that never finishes, and a rate limit that is not retried is a library that never
        // fills.
        let retryable: Vec<bool> = ALL.iter().map(ProviderError::retryable).collect();

        assert_eq!(retryable, [false, false, false, true, true, false, true]);
    }

    #[test]
    fn no_variant_carries_free_text_that_could_hold_a_credential() {
        // Every field is a provider, a name, a number, or a closed set. A `String` anywhere
        // in this enum would be a route from a provider's error message to a screen the user
        // copies out of the app, and TMDB takes its key as a query parameter.
        for error in &ALL {
            let rendered = error.to_string();
            for scheme in ["http://", "https://", "api_key", "Bearer"] {
                assert!(!rendered.contains(scheme), "{rendered} names {scheme}");
            }
        }
    }
}
