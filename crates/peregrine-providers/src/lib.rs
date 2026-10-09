#![forbid(unsafe_code)]
//! `MetadataProvider` and `SubtitleProvider` adapters, response normalization,
//! matching and scoring, rate limiting, and conditional requests.
//!
//! No third-party client type escapes the adapter module, and every response
//! passes through a size-capped, validating decode.
