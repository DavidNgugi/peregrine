//! What a provider's answer must be before anything reads it (PG-E5-05).
//!
//! `docs/ARCHITECTURE.md` §9 states the rule this module enforces: "every response
//! deserialized into a validating model; oversized payload caps; no HTML rendering of
//! provider strings", and §13 lists "malformed/oversized payload rejection" among the tests
//! every metadata adapter owes. The crate's own module doc claims the same thing — "every
//! response passes through a size-capped, validating decode" — and this is where that claim
//! is kept.
//!
//! It takes bytes rather than a `reqwest::Response` on purpose. A provider's answer is
//! untrusted input whatever carried it, the rules about reading it do not depend on the
//! client that fetched it, and a function over `&[u8]` can be tested against a fixture that
//! is a byte literal rather than against a server. The client (PG-E5-02) hands these bodies
//! over; nothing about a client type reaches this module.
//!
//! # The cap has no default here
//!
//! What is too large is a property of an endpoint and not of the port: a `/changes` page and
//! a single title's details differ by two orders of magnitude. So the cap is a parameter, the
//! adapter that knows its endpoints passes it, and no number is invented in this file — the
//! same division the [`crate::RatePolicy`] and [`crate::CachePolicy`] declarations follow.

use std::num::NonZeroU64;

use serde::de::DeserializeOwned;

use peregrine_domain::id::Provider;

use crate::error::{ProviderError, TransportFailure};

/// The largest body this build will read from one endpoint.
///
/// A newtype rather than a `u64` because zero is not a cap — it is an endpoint that can never
/// answer — and because a length compared against a bare integer at each call site is a
/// comparison nobody can find later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct BodyCap(NonZeroU64);

impl BodyCap {
    /// Caps a body at `bytes`.
    #[must_use]
    pub const fn new(bytes: NonZeroU64) -> Self {
        Self(bytes)
    }

    /// The cap in bytes.
    #[must_use]
    pub const fn bytes(self) -> u64 {
        self.0.get()
    }

    /// Whether a body of `len` bytes is over the cap.
    ///
    /// Inclusive: a body exactly at the cap is accepted, because a cap is a limit and not a
    /// strict inequality, and an off-by-one here would reject the one page the cap was chosen
    /// for.
    #[must_use]
    pub const fn exceeded_by(self, len: u64) -> bool {
        len > self.0.get()
    }
}

/// Whether a media type is one this build will read as JSON.
///
/// Parameters are ignored (`application/json; charset=utf-8` is JSON), and the `+json`
/// structured suffix is accepted because it is the standard way a JSON-based format says so
/// (`application/vnd.api+json`).
fn is_json_media_type(content_type: &str) -> bool {
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    media_type == "application/json" || media_type == "text/json" || media_type.ends_with("+json")
}

/// A provider's answer, before anything has read it.
///
/// The content type is optional because it is optional on the wire, and a provider that omits
/// it is still answering: refusing a body for a missing header would refuse data this build
/// can read perfectly well. What is refused is a header that says the body is something other
/// than JSON, because that is the shape of an error page — a proxy, a captive portal, or a
/// maintenance page answering `200` — and decoding it as data is how a "no such title" becomes
/// a corrupted row.
#[derive(Debug)]
pub struct Body<'a> {
    provider: Provider,
    bytes: &'a [u8],
    content_type: Option<&'a str>,
}

impl<'a> Body<'a> {
    /// Wraps a response body and the `Content-Type` it arrived with, if any.
    #[must_use]
    pub const fn new(provider: Provider, bytes: &'a [u8], content_type: Option<&'a str>) -> Self {
        Self {
            provider,
            bytes,
            content_type,
        }
    }

    /// How many bytes arrived.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Whether nothing arrived.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Reads the body as `T`.
    ///
    /// Three things are checked, in the order that costs least: the length against `cap`, the
    /// media type if one was sent, and then the shape of what is there.
    ///
    /// # Errors
    ///
    /// - [`ProviderError::TooLarge`] when the body is over `cap`. It reports how large it was
    ///   as far as it was read, which is what tells an operator whether a cap is one page too
    ///   low or the endpoint has started returning something else entirely.
    /// - [`ProviderError::Transport`] with [`TransportFailure::Decode`] when the body is not
    ///   JSON at all — an HTML page, a truncated read, an empty answer, bytes that are not
    ///   UTF-8, or a structure nested deeply enough that parsing it is the attack. These are
    ///   "we could not read what arrived", and the network can produce every one of them.
    /// - [`ProviderError::SchemaDrift`] when the body *is* JSON but is not the shape this
    ///   build expects: a field has gone, or changed type. That is a different fact from a
    ///   decode failure — the provider changed how it answers — and ADR 0007 rule 7 keeps
    ///   serving cached data when it happens rather than reporting an error to the user.
    ///
    /// Unknown fields are not an error and never will be: a provider adding a field is a
    /// provider improving, and `deny_unknown_fields` in an adapter would turn every addition
    /// into an outage. What is asserted here is that tolerance, so that an adapter cannot
    /// quietly take it away.
    pub fn decode<T: DeserializeOwned>(&self, cap: BodyCap) -> Result<T, ProviderError> {
        let provider = self.provider;

        if cap.exceeded_by(self.bytes.len() as u64) {
            return Err(ProviderError::TooLarge {
                provider,
                bytes: self.bytes.len() as u64,
                cap: cap.bytes(),
            });
        }

        if let Some(content_type) = self.content_type
            && !is_json_media_type(content_type)
        {
            return Err(ProviderError::Transport {
                provider,
                kind: TransportFailure::Decode,
            });
        }

        serde_json::from_slice::<T>(self.bytes).map_err(|error| match error.classify() {
            // The bytes are JSON; the model is not what they say.
            serde_json::error::Category::Data => ProviderError::SchemaDrift { provider },
            // The bytes are not JSON, or not all of it arrived.
            serde_json::error::Category::Io
            | serde_json::error::Category::Syntax
            | serde_json::error::Category::Eof => ProviderError::Transport {
                provider,
                kind: TransportFailure::Decode,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use serde::Deserialize;

    use super::*;

    /// The shape one endpoint is expected to answer with, written the way an adapter would.
    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct Page {
        page: u32,
        results: Vec<Hit>,
    }

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct Hit {
        id: String,
    }

    fn cap(bytes: u64) -> BodyCap {
        BodyCap::new(NonZeroU64::new(bytes).expect("the tests pass a non-zero cap"))
    }

    fn page() -> Vec<u8> {
        br#"{"page":1,"results":[{"id":"1396"}]}"#.to_vec()
    }

    fn decode(bytes: &[u8], content_type: Option<&str>, limit: u64) -> Result<Page, ProviderError> {
        Body::new(Provider::Tmdb, bytes, content_type).decode(cap(limit))
    }

    #[test]
    fn a_well_formed_body_decodes() {
        let decoded = decode(&page(), Some("application/json"), 1024).expect("it is JSON and fits");

        assert_eq!(decoded.page, 1);
        assert_eq!(decoded.results.len(), 1);
        assert_eq!(decoded.results[0].id, "1396");
    }

    #[test]
    fn a_body_exactly_at_the_cap_is_read() {
        // The boundary, from the accepting side: a cap is a limit, not a strict inequality,
        // and the page the cap was chosen for must not be the one it rejects.
        let bytes = page();
        let decoded = decode(&bytes, Some("application/json"), bytes.len() as u64);

        assert!(decoded.is_ok(), "a body at the cap must be read");
    }

    #[test]
    fn a_body_one_byte_over_the_cap_is_refused() {
        // The other side of the same boundary, one byte away.
        let bytes = page();
        let refused = decode(&bytes, Some("application/json"), bytes.len() as u64 - 1);

        assert_eq!(
            refused,
            Err(ProviderError::TooLarge {
                provider: Provider::Tmdb,
                bytes: bytes.len() as u64,
                cap: bytes.len() as u64 - 1,
            })
        );
    }

    #[test]
    fn a_body_far_over_the_cap_is_refused_with_its_size() {
        // The size is what tells an operator whether the cap is one page too low or the
        // endpoint has started answering with something else entirely.
        let huge = vec![b'x'; 10_000];
        let refused = decode(&huge, Some("application/json"), 64);

        match refused {
            Err(ProviderError::TooLarge { bytes, cap: at, .. }) => {
                assert_eq!(bytes, 10_000);
                assert_eq!(at, 64);
            }
            other => panic!("expected a cap refusal, got {other:?}"),
        }
    }

    #[test]
    fn an_html_error_page_is_refused_even_though_it_says_it_is_json() {
        // The case that matters: a proxy, a captive portal, or a maintenance page answering
        // `200 application/json` with HTML. Decoding it as data is how a "no such title"
        // becomes a corrupted row, so the header is not trusted over the bytes.
        let html = b"<html><body>Service unavailable</body></html>";
        let refused = decode(html, Some("application/json"), 1024);

        assert_eq!(
            refused,
            Err(ProviderError::Transport {
                provider: Provider::Tmdb,
                kind: TransportFailure::Decode,
            })
        );
    }

    #[test]
    fn a_body_the_provider_labels_as_html_is_refused_without_being_read() {
        let refused = decode(&page(), Some("text/html; charset=utf-8"), 1024);

        assert!(matches!(
            refused,
            Err(ProviderError::Transport {
                kind: TransportFailure::Decode,
                ..
            })
        ));
    }

    #[test]
    fn a_provider_that_sends_no_content_type_is_still_read() {
        // The header is optional on the wire. Refusing a body for a missing header would
        // refuse data this build can read perfectly well.
        assert!(decode(&page(), None, 1024).is_ok());
    }

    #[test]
    fn content_type_parameters_and_structured_suffixes_are_both_json() {
        // `charset` is a parameter on the same media type; `+json` is the standard suffix by
        // which a JSON-based format says what it is.
        assert!(decode(&page(), Some("application/json; charset=utf-8"), 1024).is_ok());
        assert!(decode(&page(), Some(" APPLICATION/JSON "), 1024).is_ok());
        assert!(decode(&page(), Some("application/vnd.api+json"), 1024).is_ok());
    }

    #[test]
    fn json_that_is_not_the_expected_shape_is_drift_and_not_a_decode_failure() {
        // The provider changed how it answers. That is a different fact from "we could not
        // read what arrived": ADR 0007 rule 7 keeps serving cached data for this one, and the
        // diagnostics screen says the provider changed rather than that the network failed.
        let missing = br#"{"page":1}"#;
        let refused = decode(missing, Some("application/json"), 1024);

        assert_eq!(
            refused,
            Err(ProviderError::SchemaDrift {
                provider: Provider::Tmdb
            })
        );
    }

    #[test]
    fn a_field_of_the_wrong_type_is_drift_too() {
        let wrong_type = br#"{"page":"one","results":[]}"#;
        let refused = decode(wrong_type, Some("application/json"), 1024);

        assert_eq!(
            refused,
            Err(ProviderError::SchemaDrift {
                provider: Provider::Tmdb
            })
        );
    }

    #[test]
    fn a_field_the_provider_added_is_not_an_error() {
        // A provider adding a field is a provider improving. `deny_unknown_fields` in an
        // adapter would turn every addition into an outage, and this asserts the tolerance
        // cannot be taken away without a test failing.
        let added = br#"{"page":1,"total_pages":9,"results":[{"id":"1396","vote":8.4}]}"#;
        let decoded = decode(added, Some("application/json"), 1024).expect("extra fields are fine");

        assert_eq!(decoded.page, 1);
    }

    #[test]
    fn an_empty_body_is_a_decode_failure_and_not_an_empty_page() {
        // The distinction matters: an empty answer is a transport fact, and treating it as
        // "no results" would write an empty page over a full one.
        let refused = decode(b"", Some("application/json"), 1024);

        assert!(matches!(
            refused,
            Err(ProviderError::Transport {
                kind: TransportFailure::Decode,
                ..
            })
        ));
    }

    #[test]
    fn a_truncated_body_is_a_decode_failure() {
        let refused = decode(
            br#"{"page":1,"results":[{"id":"13"#,
            Some("application/json"),
            1024,
        );

        assert!(matches!(
            refused,
            Err(ProviderError::Transport {
                kind: TransportFailure::Decode,
                ..
            })
        ));
    }

    #[test]
    fn bytes_that_are_not_text_are_a_decode_failure() {
        let refused = decode(&[0xff, 0xfe, 0x00, 0x01], Some("application/json"), 1024);

        assert!(matches!(
            refused,
            Err(ProviderError::Transport {
                kind: TransportFailure::Decode,
                ..
            })
        ));
    }

    #[test]
    fn a_body_nested_deeply_enough_to_be_an_attack_is_refused_rather_than_parsed() {
        // AGENTS §4's "deeply nested structure" case. The body is small and so passes the cap;
        // what must not happen is a stack overflow inside the decoder, which would be a crash
        // rather than an error and would take the whole app with it.
        let depth = 10_000;
        let nested = format!("{}{}{}", "[".repeat(depth), "1", "]".repeat(depth));
        let refused = decode(nested.as_bytes(), Some("application/json"), 1024 * 1024);

        assert!(refused.is_err(), "a 10 000-deep body must be refused");
    }

    #[test]
    fn the_cap_boundary_is_inclusive_rather_than_an_off_by_one() {
        let exact = cap(16);
        assert!(!exact.exceeded_by(16));
        assert!(exact.exceeded_by(17));
        assert!(!exact.exceeded_by(0));
        assert_eq!(exact.bytes(), 16);
    }

    #[test]
    fn a_body_reports_its_own_length_without_reading_it() {
        let bytes = page();
        let body = Body::new(Provider::Tmdb, &bytes, None);

        assert_eq!(body.len(), bytes.len());
        assert!(!body.is_empty());
        assert!(Body::new(Provider::Tmdb, b"", None).is_empty());
    }
}
