#![forbid(unsafe_code)]
//! `MetadataProvider` and `SubtitleProvider` adapters, response normalization,
//! matching and scoring, rate limiting, and conditional requests.
//!
//! No third-party client type escapes the adapter module, and every response
//! passes through a size-capped, validating decode.
//!
//! # What is here and what is not
//!
//! The port is here: [`MetadataProvider`], the [`ProviderDescriptor`] every adapter
//! publishes, the [`Capabilities`] it declares, the [`RatePolicy`] and [`CachePolicy`] it
//! answers under, the [`Attribution`] its terms require, and the closed [`ProviderError`]
//! vocabulary. Those are what PG-E5-01 asks for, and the conformance suite that runs over
//! them lives in `peregrine-testkit` (PG-E5-18).
//!
//! Response hygiene is here too: [`Body`] is what a provider's answer has to be before
//! anything reads it — size-capped, content-type checked, and decoded into a validating model
//! (PG-E5-05, `docs/ARCHITECTURE.md` §9).
//!
//! The adapters are not: TMDB is PG-E5-06, TVDB PG-E5-10, `OMDb` PG-E5-11. Neither is the HTTP
//! stack (PG-E5-02), the token buckets (PG-E5-03), nor the conditional-request middleware
//! (PG-E5-04) — and the port's data operations arrive with the first adapter, for the reason
//! the [`provider`] module records.

pub mod capability;
pub mod descriptor;
pub mod error;
pub mod hygiene;
pub mod provider;

pub use capability::{Capabilities, Capability};
pub use descriptor::{Attribution, CachePolicy, DescriptorError, ProviderDescriptor, RatePolicy};
pub use error::{ProviderError, TransportFailure};
pub use hygiene::{Body, BodyCap};
pub use provider::{MetadataProvider, ProviderFuture, SearchCandidate, SearchQuery};
