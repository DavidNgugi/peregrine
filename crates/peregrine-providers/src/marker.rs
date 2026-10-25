//! The marker-provider port: one question, one closed error vocabulary.
//!
//! A marker provider answers "what are this title's intro, recap, credits, and preview windows?"
//! It is separate from `MetadataProvider` because the subject is different: a metadata provider
//! enriches a catalogue row, while a marker provider is asked at playback time with a duration it
//! uses to pick the right release version. Folding the two together would make every metadata
//! adapter declare a capability it does not have and every marker adapter carry a catalogue it
//! never reads.
//!
//! **No provider type escapes its adapter.** The request and the answer are domain values; the
//! adapter owns its JSON and its HTTP client (AGENTS R8). The error carries the source and a
//! closed kind, never a URL or a body: `TheIntroDB` and `AniSkip` both take their identity in the
//! query string, and a URL in an error is a route into a log (SECURITY.md T13).

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use peregrine_domain::{MarkerSource, PlaybackMarker, TitleKind};

use crate::error::TransportFailure;

/// What a marker provider is asked about.
///
/// The ids are optional because a title may carry TMDB but not TVDB, and the adapter that knows
/// which identity it prefers is the one that decides what to send. `duration_ms` is the file's own
/// runtime, which `TheIntroDB` uses to choose the closest release version; a provider that does not
/// need it ignores it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkerRequest {
    /// Film or series.
    pub kind: TitleKind,
    /// The season number, when the subject is an episode.
    pub season: Option<u32>,
    /// The episode number, when the subject is an episode.
    pub episode: Option<u32>,
    /// `TheTVDB`'s id, `TheIntroDB`'s preferred identity.
    pub tvdb_id: Option<u64>,
    /// TMDB's id, accepted by `TheIntroDB` as a slower alternate.
    pub tmdb_id: Option<u64>,
    /// `IMDb`'s id, accepted by `TheIntroDB` as a slower alternate.
    pub imdb_id: Option<String>,
    /// `MyAnimeList`'s id, which is the only identity `AniSkip` accepts.
    pub mal_id: Option<u64>,
    /// The file's runtime in milliseconds, when the player knows it.
    pub duration_ms: Option<i64>,
    /// Whether the title is anime, which is the only case `AniSkip` may answer.
    pub is_anime: bool,
}

impl MarkerRequest {
    /// Whether this request names a TV episode rather than a film.
    #[must_use]
    pub const fn is_episode(&self) -> bool {
        matches!((self.season, self.episode), (Some(_), Some(_)))
    }
}

/// The future a marker provider returns.
pub type MarkerFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, MarkerError>> + Send + 'a>>;

/// One source of skip markers.
///
/// Object-safe by construction, for the same reason `MetadataProvider` is: the registry holds
/// different adapters in one list, and an `async fn` in a trait rules that out on this toolchain.
pub trait MarkerProvider: Send + Sync {
    /// Which source this adapter speaks for.
    fn source(&self) -> MarkerSource;

    /// Whether a request is refused until a credential is stored.
    ///
    /// False for `TheIntroDB` and `AniSkip`, whose public reads need no key. `TheIntroDB`'s key is for
    /// submissions only and is therefore not this flag.
    fn requires_credential(&self) -> bool;

    /// Asks the provider for this subject's markers.
    fn fetch(&self, request: MarkerRequest) -> MarkerFuture<'_, Vec<PlaybackMarker>>;
}

/// Why a marker provider could not answer.
///
/// No free text and no URL: the source identifies the adapter, and the variant is what a caller
/// can act on. `retry_after` is the provider's own number and only when it sent one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MarkerError {
    /// The provider was asked something it does not serve, or lacks the identity it needs.
    #[error("{marker_source:?} cannot answer that request")]
    Unsupported {
        /// The source that refused.
        marker_source: MarkerSource,
    },
    /// The provider has no markers for this subject, which is not a failure.
    #[error("{marker_source:?} has no markers for that subject")]
    NotFound {
        /// The source that answered.
        marker_source: MarkerSource,
    },
    /// The credential was refused, or one was needed and absent.
    #[error("{marker_source:?} refused the credential")]
    Unauthorized {
        /// The source that refused.
        marker_source: MarkerSource,
    },
    /// The provider asked to be left alone for a while.
    #[error("{marker_source:?} is rate limiting this client")]
    RateLimited {
        /// The source that refused.
        marker_source: MarkerSource,
        /// The delay the provider asked for, when it named one.
        retry_after: Option<Duration>,
    },
    /// The answer was JSON this build does not recognise.
    #[error("{marker_source:?} answered with a shape this build does not recognise")]
    SchemaDrift {
        /// The source whose answer drifted.
        marker_source: MarkerSource,
    },
    /// The answer was over the cap this adapter allows.
    #[error("{marker_source:?} answered with {bytes} bytes, over the {cap} byte cap")]
    TooLarge {
        /// The source whose answer was oversized.
        marker_source: MarkerSource,
        /// How large the answer was.
        bytes: u64,
        /// The cap it exceeded.
        cap: u64,
    },
    /// The request never reached the provider, or the answer never arrived whole.
    #[error("{marker_source:?} could not be reached")]
    Transport {
        /// The source that could not be reached.
        marker_source: MarkerSource,
        /// What kind of failure it was.
        kind: TransportFailure,
    },
}

impl MarkerError {
    /// The source this failure is about.
    #[must_use]
    pub const fn source(&self) -> MarkerSource {
        match self {
            Self::Unsupported {
                marker_source: source,
            }
            | Self::NotFound {
                marker_source: source,
            }
            | Self::Unauthorized {
                marker_source: source,
            }
            | Self::RateLimited {
                marker_source: source,
                ..
            }
            | Self::SchemaDrift {
                marker_source: source,
            }
            | Self::TooLarge {
                marker_source: source,
                ..
            }
            | Self::Transport {
                marker_source: source,
                ..
            } => *source,
        }
    }

    /// Whether asking again later could succeed.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. } | Self::Transport { .. } | Self::SchemaDrift { .. }
        )
    }
}

impl From<(MarkerSource, crate::ProviderError)> for MarkerError {
    fn from((source, error): (MarkerSource, crate::ProviderError)) -> Self {
        match error {
            crate::ProviderError::Unsupported { .. } => Self::Unsupported {
                marker_source: source,
            },
            crate::ProviderError::NotFound { .. } => Self::NotFound {
                marker_source: source,
            },
            crate::ProviderError::Unauthorized { .. } => Self::Unauthorized {
                marker_source: source,
            },
            crate::ProviderError::RateLimited { retry_after, .. } => Self::RateLimited {
                marker_source: source,
                retry_after,
            },
            crate::ProviderError::SchemaDrift { .. } => Self::SchemaDrift {
                marker_source: source,
            },
            crate::ProviderError::TooLarge { bytes, cap, .. } => Self::TooLarge {
                marker_source: source,
                bytes,
                cap,
            },
            crate::ProviderError::Transport { kind, .. } => Self::Transport {
                marker_source: source,
                kind,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_failure_names_the_source_it_is_about() {
        let failures = [
            MarkerError::Unsupported {
                marker_source: MarkerSource::IntroDb,
            },
            MarkerError::NotFound {
                marker_source: MarkerSource::AniSkip,
            },
            MarkerError::Unauthorized {
                marker_source: MarkerSource::IntroHater,
            },
            MarkerError::RateLimited {
                marker_source: MarkerSource::IntroDb,
                retry_after: Some(Duration::from_secs(2)),
            },
            MarkerError::SchemaDrift {
                marker_source: MarkerSource::AniSkip,
            },
            MarkerError::TooLarge {
                marker_source: MarkerSource::IntroDb,
                bytes: 2_000_000,
                cap: 1_000_000,
            },
            MarkerError::Transport {
                marker_source: MarkerSource::IntroDb,
                kind: TransportFailure::Timeout,
            },
        ];

        assert_eq!(failures[0].source(), MarkerSource::IntroDb);
        assert_eq!(failures[1].source(), MarkerSource::AniSkip);
        assert_eq!(failures[2].source(), MarkerSource::IntroHater);
        for failure in &failures {
            let rendered = failure.to_string();
            for scheme in ["http://", "https://", "Bearer", "api_key"] {
                assert!(!rendered.contains(scheme), "{rendered} names {scheme}");
            }
        }
    }

    #[test]
    fn only_the_answers_worth_retrying_say_so() {
        assert!(
            !MarkerError::Unauthorized {
                marker_source: MarkerSource::IntroDb
            }
            .retryable()
        );
        assert!(
            !MarkerError::NotFound {
                marker_source: MarkerSource::IntroDb
            }
            .retryable()
        );
        assert!(
            MarkerError::RateLimited {
                marker_source: MarkerSource::IntroDb,
                retry_after: None
            }
            .retryable()
        );
        assert!(
            MarkerError::Transport {
                marker_source: MarkerSource::IntroDb,
                kind: TransportFailure::Decode
            }
            .retryable()
        );
    }

    #[test]
    fn a_provider_error_keeps_its_kind_when_it_crosses_the_port() {
        let mapped: MarkerError = (
            MarkerSource::AniSkip,
            crate::ProviderError::Unauthorized {
                provider: peregrine_domain::Provider::Tmdb,
            },
        )
            .into();
        assert_eq!(
            mapped,
            MarkerError::Unauthorized {
                marker_source: MarkerSource::AniSkip
            }
        );
    }
}
