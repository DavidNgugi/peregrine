//! The `peregrine://` links that arrive from *outside* the process (PG-E1-11).
//!
//! There are two `peregrine://` vocabularies and they are deliberately not the same
//! type. [`deep_link`](crate::notify::deep_link) builds the links this application
//! hands to the OS for a notification click: a closed route set, identifiers this
//! process chose. This module parses the links the OS hands *to* us, which are input
//! from a web page, a chat client, or a command line and therefore hostile
//! (SECURITY.md T12). An external link may only name a `title` or a `magnet`; every
//! other route is refused, including the ones the internal vocabulary knows.
//!
//! A refusal never carries the refused text. SECURITY.md T13 is about what the
//! diagnostics log exports, and a deep link can carry a magnet URI, so an error
//! variant holds no attacker-controlled string and no `Display` here can leak one.
//!
//! # Grammar
//!
//! ```text
//! title   peregrine://title/<provider>/<id>
//! magnet  peregrine://magnet/<40 hex infohash>
//!         peregrine://magnet?xt=urn:btih:<40 hex infohash>
//! ```
//!
//! The scheme is matched case-insensitively because RFC 3986 says a scheme is; the
//! route is not, because accepting `TITLE` would put two spellings of one route into
//! the routing table. The infohash is accepted in the canonical 40-character
//! hexadecimal form only: a 32-character base32 `btih` and a `BitTorrent` v2 `btmh`
//! multihash are refused rather than decoded, because nothing can act on either until
//! a torrent engine exists to consume one (PG-E10), and refusing is what fail-closed
//! means here.

use crate::notify::{DeepLink, DeepLinkError, MAX_DEEP_LINK_BYTES, deep_link};
use thiserror::Error;

/// The authority every external link starts with.
const SCHEME: &str = "peregrine://";

/// A link that arrived from outside the process, classified.
///
/// A magnet variant carries nothing. A magnet may not be added before the
/// confirmation gate that shows the inspection report (SECURITY.md T12), and no
/// torrent engine exists to inspect one yet, so there is no code that could act on
/// the infohash this parser validated. A value that only a log line or an event
/// payload could carry is a liability, not a result, so the validation is the
/// output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalLink {
    /// Open a title's page, as the canonical internal link.
    Title(DeepLink),
    /// Add a magnet. The route layer refuses this until PG-E10 exists; the parse
    /// exists so a well-formed magnet is refused for that reason and a malformed one
    /// for its own.
    Magnet,
}

/// Why an external link was refused.
///
/// Every variant is a fixed sentence: no variant holds the refused text, so logging
/// one cannot copy a magnet URI into the diagnostics tail (SECURITY.md T13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ExternalLinkError {
    /// The text is not a `peregrine://` link.
    #[error("the text is not a peregrine:// link")]
    NotOwned,
    /// The route is neither `title` nor `magnet`.
    #[error("the link names a route this application does not open from outside")]
    UnknownRoute,
    /// A `title` link that is not `peregrine://title/<provider>/<id>`.
    #[error("a title link must be peregrine://title/<provider>/<id>")]
    MalformedTitle,
    /// A `magnet` link whose infohash is missing or is not 40 hexadecimal characters.
    #[error("a magnet link must carry a 40-character hexadecimal btih infohash")]
    MalformedMagnet,
    /// The link is longer than the link budget, so it is refused unread.
    #[error("the link is longer than the deep-link budget")]
    TooLong,
}

/// Classifies one external link.
///
/// # Errors
///
/// [`ExternalLinkError`], whose variants name the shape that was refused without
/// carrying any part of the input.
pub fn parse(text: &str) -> Result<ExternalLink, ExternalLinkError> {
    if text.len() > MAX_DEEP_LINK_BYTES {
        return Err(ExternalLinkError::TooLong);
    }
    let rest = strip_scheme(text)?;
    let (route, remainder) = split_route(rest);
    match route {
        "title" => parse_title(remainder),
        "magnet" => parse_magnet(remainder),
        _ => Err(ExternalLinkError::UnknownRoute),
    }
}

/// The text after the scheme, with the scheme matched case-insensitively.
///
/// Slicing by length rather than splitting on `://` keeps the match anchored at the
/// start, so `notperegrine://...` is not owned, and a byte index that is not a
/// character boundary is a refusal rather than a panic.
fn strip_scheme(text: &str) -> Result<&str, ExternalLinkError> {
    let Some(prefix) = text.get(..SCHEME.len()) else {
        return Err(ExternalLinkError::NotOwned);
    };
    if !prefix.eq_ignore_ascii_case(SCHEME) {
        return Err(ExternalLinkError::NotOwned);
    }
    Ok(&text[SCHEME.len()..])
}

/// The route and everything after it.
///
/// The route is the authority of the URI, which is why the canonical link reads
/// `peregrine://title/...`: `title` is the host. Splitting here rather than treating
/// the first path segment as the route is what makes a link with a real host
/// (`peregrine://title.evil.example/tmdb/1`) fall out as an unknown route instead of
/// being silently accepted.
fn split_route(rest: &str) -> (&str, &str) {
    match rest.find(['/', '?', '#']) {
        Some(index) => rest.split_at(index),
        None => (rest, ""),
    }
}

/// `peregrine://title/<provider>/<id>`, canonicalized through the internal builder.
///
/// Rebuilding rather than passing the input on is the point: [`deep_link`] is the one
/// definition of what a title link looks like, so an external link and a notification
/// click produce the same string for the same title, and the identifier charset is
/// checked once, in the place the app already checks it.
fn parse_title(remainder: &str) -> Result<ExternalLink, ExternalLinkError> {
    let path = remainder
        .strip_prefix('/')
        .ok_or(ExternalLinkError::MalformedTitle)?;
    if path.contains(['?', '#']) {
        return Err(ExternalLinkError::MalformedTitle);
    }
    let mut segments = path.split('/');
    let (Some(provider), Some(id), None) = (segments.next(), segments.next(), segments.next())
    else {
        return Err(ExternalLinkError::MalformedTitle);
    };
    deep_link("title", &[provider, id])
        .map(ExternalLink::Title)
        .map_err(|error| match error {
            // Both of these mean "not the shape a title link has", and neither carries
            // the input, so collapsing them changes nothing a caller could act on.
            DeepLinkError::NotCanonical
            | DeepLinkError::NotAnIdentifier { .. }
            | DeepLinkError::UnknownRoute { .. }
            | DeepLinkError::NotOwned { .. }
            | DeepLinkError::Redaction(_) => ExternalLinkError::MalformedTitle,
        })
}

/// `peregrine://magnet/<hash>` or `peregrine://magnet?xt=urn:btih:<hash>`.
fn parse_magnet(remainder: &str) -> Result<ExternalLink, ExternalLinkError> {
    if remainder.is_empty() || remainder.contains('#') {
        return Err(ExternalLinkError::MalformedMagnet);
    }
    if let Some(path) = remainder.strip_prefix('/') {
        if path.contains(['/', '?']) {
            return Err(ExternalLinkError::MalformedMagnet);
        }
        return is_infohash(path)
            .then_some(ExternalLink::Magnet)
            .ok_or(ExternalLinkError::MalformedMagnet);
    }
    let query = remainder
        .strip_prefix('?')
        .ok_or(ExternalLinkError::MalformedMagnet)?;
    // Exactly one `xt`, because two would leave which one is the infohash to the
    // order of the query string, and the query string is the attacker's.
    let mut found = None;
    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(ExternalLinkError::MalformedMagnet);
        };
        if key == "xt" && found.replace(value).is_some() {
            return Err(ExternalLinkError::MalformedMagnet);
        }
    }
    let value = found.ok_or(ExternalLinkError::MalformedMagnet)?;
    let hash = value
        .strip_prefix("urn:btih:")
        .ok_or(ExternalLinkError::MalformedMagnet)?;
    is_infohash(hash)
        .then_some(ExternalLink::Magnet)
        .ok_or(ExternalLinkError::MalformedMagnet)
}

/// Whether `text` is exactly 40 hexadecimal characters.
///
/// Only the shape is asserted, in either case: nothing keeps the value, so there is
/// no canonical spelling to enforce here.
fn is_infohash(text: &str) -> bool {
    text.len() == 40 && text.chars().all(|character| character.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::{MAX_TEXT_BYTES, deep_link};
    use proptest::prelude::*;

    const HASH: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn a_title_link_becomes_the_canonical_internal_link() {
        let link = parse("peregrine://title/tmdb/123").expect("a well-formed title link");
        assert_eq!(
            link,
            ExternalLink::Title(deep_link("title", &["tmdb", "123"]).expect("a valid title route"))
        );
    }

    #[test]
    fn the_scheme_is_matched_case_insensitively_and_the_route_is_not() {
        assert!(parse("PEREGRINE://title/tmdb/123").is_ok());
        assert_eq!(
            parse("peregrine://TITLE/tmdb/123"),
            Err(ExternalLinkError::UnknownRoute),
            "two spellings of a route would be two entries in the routing table"
        );
    }

    #[test]
    fn the_canonical_form_of_a_title_link_is_what_the_router_emits() {
        let ExternalLink::Title(link) = parse("peregrine://title/tmdb/123").unwrap() else {
            panic!("a title link");
        };
        assert_eq!(link.as_str(), "peregrine://title/tmdb/123");
    }

    #[test]
    fn a_magnet_link_is_recognized_in_both_documented_shapes() {
        assert_eq!(
            parse(&format!("peregrine://magnet/{HASH}")),
            Ok(ExternalLink::Magnet)
        );
        assert_eq!(
            parse(&format!("peregrine://magnet?xt=urn:btih:{HASH}")),
            Ok(ExternalLink::Magnet)
        );
        assert_eq!(
            parse(&format!(
                "peregrine://magnet?dn=A&tr=http%3A%2F%2Ft&xt=urn:btih:{HASH}"
            )),
            Ok(ExternalLink::Magnet),
            "a real magnet's other parameters are ignored, not trusted"
        );
    }

    #[test]
    fn a_refused_link_never_carries_its_own_text() {
        // The whole point of the error type: a magnet deep link that is refused must
        // not be copied into the diagnostics tail by the message (SECURITY.md T13).
        let hostile = format!("peregrine://magnet?xt=urn:btih:{HASH}#dn=secret-name");
        let error = parse(&hostile).unwrap_err();
        assert!(!error.to_string().contains(HASH));
        assert!(!error.to_string().contains("secret-name"));
        assert!(!format!("{error:?}").contains(HASH));
    }

    #[test]
    fn every_shape_that_is_not_a_link_is_refused() {
        for refused in [
            "",
            "https://example.com",
            "peregrine:/title/tmdb/1",
            "peregrine://",
            "peregrine://settings",
            "peregrine://downloads",
            "peregrine://title",
            "peregrine://title/tmdb",
            "peregrine://title/tmdb/123/extra",
            "peregrine://title//123",
            "peregrine://title/tmdb/",
            "peregrine://title/tmdb/12 3",
            "peregrine://title/tmdb/12%2F3",
            "peregrine://title/tmdb/123?utm=1",
            "peregrine://title/tmdb/123#fragment",
            "peregrine://title.evil.example/tmdb/123",
            "peregrine://magnet",
            "peregrine://magnet/",
            "peregrine://magnet/0123456789abcdef0123456789abcdef0123456",
            "peregrine://magnet/0123456789abcdef0123456789abcdef012345678",
            "peregrine://magnet/MFRGGZDFMZTWQ2LKNNWG23TPOBYXE43U",
            "peregrine://magnet?xt=urn:btih:short",
            "peregrine://magnet?xt=urn:btmh:12200123456789abcdef0123456789abcdef01234567",
            "peregrine://magnet?dn=name",
            "peregrine://magnet?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&xt=urn:btih:0123456789abcdef0123456789abcdef01234567",
            "peregrine://magnet?xt",
            "peregrine://magnet?xt=urn:btih:0123456789abcdef0123456789abcdef01234567#x",
            "peregrine://magnet/0123456789abcdef0123456789abcdef01234567?xt=urn:btih:0123456789abcdef0123456789abcdef01234567",
        ] {
            assert!(
                parse(refused).is_err(),
                "{refused} should have been refused"
            );
        }
    }

    #[test]
    fn a_link_longer_than_the_budget_is_refused_unread() {
        let long = format!("peregrine://title/tmdb/{}", "1".repeat(MAX_TEXT_BYTES));
        assert_eq!(parse(&long), Err(ExternalLinkError::TooLong));
    }

    proptest! {
        /// The parser is fed whatever a web page or a shell hands it, so the property
        /// that matters most is that it answers rather than panics.
        #[test]
        fn arbitrary_text_is_classified_or_refused(text in "\\PC{0,300}") {
            let _ = parse(&text);
        }

        /// Anything accepted is within the budget and re-parses to itself, which is
        /// what makes the canonical form a fixed point of the parser.
        #[test]
        fn an_accepted_link_re_parses_identically(text in "\\PC{0,300}") {
            if let Ok(link) = parse(&text) {
                let canonical = match &link {
                    ExternalLink::Title(deep) => deep.as_str().to_owned(),
                    ExternalLink::Magnet => return Ok(()),
                };
                prop_assert!(canonical.len() <= MAX_DEEP_LINK_BYTES);
                prop_assert_eq!(parse(&canonical).unwrap(), link);
            }
        }
    }
}
