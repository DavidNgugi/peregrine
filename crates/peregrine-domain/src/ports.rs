//! Ports for the things a pure domain may not reach for itself: the current time, fresh
//! identifiers, and unguessable session tokens.
//!
//! Adapters own the implementations that touch a real clock or a random source;
//! everything here is injected, so a test can pin all three without a machine.

use jiff::Timestamp;
use thiserror::Error;
use ulid::Ulid;

/// The application's source of wall-clock time.
pub trait Clock {
    /// Returns the current instant.
    fn now(&self) -> Timestamp;
}

/// The application's source of fresh identifiers.
pub trait IdGenerator {
    /// Returns the next identifier.
    fn next_id(&self) -> Ulid;
}

/// The application's source of unguessable session tokens.
///
/// Separate from [`IdGenerator`] although both draw on the same entropy pool, because the two are
/// asked for different things: an identifier must *sort* by issue order and is therefore allowed to
/// leak when it was minted, while a token must not (AGENTS R7 — the vocabulary lives where the rule
/// does). Reusing the ULID generator here would put the session's start time in the URL of every
/// loopback request.
pub trait TokenSource {
    /// Returns a token with at least 128 bits of entropy behind it.
    fn next_token(&self) -> SessionToken;
}

/// The shortest token this build will accept, in characters.
///
/// 32 characters of the alphabet below is 160 bits if every one is drawn uniformly, and the check is
/// a floor on the *text* rather than a measurement of entropy: what this refuses is a fixture, a
/// placeholder, or a forwarded value that is obviously not a minted token, and no type can do better
/// than that from the outside.
pub const MIN_TOKEN_CHARS: usize = 32;

/// The longest token this build will accept, in characters.
///
/// A token is a path segment, so it is bounded for the same reason the alphabet is: an unbounded
/// value is a value a caller can be made to build a very large URL out of, and the server would
/// compare all of it before answering.
pub const MAX_TOKEN_CHARS: usize = 128;

/// Why a value could not be a session token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SessionTokenError {
    /// Shorter than [`MIN_TOKEN_CHARS`], or longer than [`MAX_TOKEN_CHARS`].
    #[error("a session token is between {MIN_TOKEN_CHARS} and {MAX_TOKEN_CHARS} characters")]
    Length,
    /// A character that is not an ASCII letter or digit.
    #[error("a session token is letters and digits only")]
    Alphabet,
}

/// An unguessable value that names one playback session.
///
/// **The only thing that authorises a read from the loopback media server** (SECURITY.md T10). It
/// travels in the request path rather than a header, because the client is a `<video>` element and a
/// media element cannot set one (ADR 0023 §2); what makes that acceptable is bounded by the server —
/// loopback, a validated `Host`, one file, `no-store`, and a socket that dies with the session.
///
/// **It does not print itself, and that is not an oversight.** A bearer credential with a derived
/// `Debug` reaches a log line the first time somebody writes `info!(?token)`, and the type system is
/// the only place that can refuse it. It is read through [`SessionToken::as_str`] at exactly two
/// kinds of site — building the URL, and comparing a request — and never logged.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SessionToken(String);

impl SessionToken {
    /// Wraps a value a [`TokenSource`] produced.
    ///
    /// The shape check is about the *text* rather than the entropy: a token is a path segment, so it
    /// is restricted to an alphabet that needs no percent-encoding, and a length that cannot make an
    /// oversized URL.
    ///
    /// # Errors
    ///
    /// [`SessionTokenError`] when the length or the alphabet is wrong.
    pub fn new(value: impl Into<String>) -> Result<Self, SessionTokenError> {
        let value = value.into();
        if value.len() < MIN_TOKEN_CHARS || value.len() > MAX_TOKEN_CHARS {
            return Err(SessionTokenError::Length);
        }
        if !value.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
            return Err(SessionTokenError::Alphabet);
        }
        Ok(Self(value))
    }

    /// The value, as the URL carries it and as a request is compared against it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether `candidate` is this token.
    ///
    /// **Every byte is compared whatever the first one says.** At 160 bits the timing channel is not
    /// exploitable, so this is not the defence — it is that a comparison on a credential is the one
    /// place where the cheap answer and the careful answer cost the same, and an early return here
    /// would be a deliberate choice to leak a prefix length for no gain.
    #[must_use]
    pub fn matches(&self, candidate: &str) -> bool {
        let expected = self.0.as_bytes();
        let offered = candidate.as_bytes();
        // The length is not secret — it is fixed by the mint and visible in the URL — so folding it
        // into the accumulator rather than returning early leaks nothing and keeps one exit.
        let mut differing = expected.len() ^ offered.len();
        for index in 0..expected.len().max(offered.len()) {
            let left = expected.get(index).copied().unwrap_or(0);
            let right = offered.get(index).copied().unwrap_or(0);
            differing |= usize::from(left ^ right);
        }
        differing == 0
    }
}

impl std::fmt::Debug for SessionToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SessionToken(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedClock {
        at: Timestamp,
    }

    impl Clock for FixedClock {
        fn now(&self) -> Timestamp {
            self.at
        }
    }

    struct SequenceIdGenerator {
        issued: std::sync::atomic::AtomicU64,
    }

    impl SequenceIdGenerator {
        const fn new() -> Self {
            Self {
                issued: std::sync::atomic::AtomicU64::new(0),
            }
        }
    }

    impl IdGenerator for SequenceIdGenerator {
        fn next_id(&self) -> Ulid {
            let issued = self
                .issued
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ulid::from_parts(1_700_000_000_000, u128::from(issued))
        }
    }

    #[test]
    fn clock_returns_the_injected_instant() {
        let pinned = Timestamp::from_millisecond(1_700_000_000_000).expect("in range");
        let clock = FixedClock { at: pinned };
        assert_eq!(clock.now(), pinned);
        assert_eq!(clock.now().as_millisecond(), 1_700_000_000_000);
    }

    #[test]
    fn id_generator_is_deterministic_and_ordered() {
        let generator = SequenceIdGenerator::new();
        let ids: Vec<Ulid> = (0..3).map(|_| generator.next_id()).collect();
        assert_eq!(
            ids,
            vec![
                Ulid::from_parts(1_700_000_000_000, 0),
                Ulid::from_parts(1_700_000_000_000, 1),
                Ulid::from_parts(1_700_000_000_000, 2),
            ]
        );
        assert!(
            ids[0] < ids[1] && ids[1] < ids[2],
            "ids sort by issue order"
        );
    }

    /// A token of the minimum shape, for the checks below.
    fn token(text: &str) -> Result<SessionToken, SessionTokenError> {
        SessionToken::new(text.to_owned())
    }

    #[test]
    fn a_token_is_letters_and_digits_of_a_bounded_length() {
        // The alphabet is what keeps the value a path segment without percent-encoding, and the
        // bounds are what stop a caller being made to build an oversized URL.
        assert_eq!(
            token(&"a".repeat(MIN_TOKEN_CHARS - 1)),
            Err(SessionTokenError::Length)
        );
        assert!(token(&"a".repeat(MIN_TOKEN_CHARS)).is_ok());
        assert!(token(&"a".repeat(MAX_TOKEN_CHARS)).is_ok());
        assert_eq!(
            token(&"a".repeat(MAX_TOKEN_CHARS + 1)),
            Err(SessionTokenError::Length)
        );
        assert_eq!(token(""), Err(SessionTokenError::Length));

        // Every one of these would need escaping, or would break the comparison.
        for hostile in [
            format!("{}/", "a".repeat(MIN_TOKEN_CHARS)),
            "/".repeat(MIN_TOKEN_CHARS),
        ] {
            assert_eq!(
                token(&hostile),
                Err(SessionTokenError::Alphabet),
                "{hostile}"
            );
        }
        assert_eq!(
            token(&"-".repeat(MIN_TOKEN_CHARS)),
            Err(SessionTokenError::Alphabet)
        );
        assert_eq!(
            token(&"é".repeat(MIN_TOKEN_CHARS)),
            Err(SessionTokenError::Alphabet)
        );
        assert_eq!(
            token(&format!("{}\0", "a".repeat(MIN_TOKEN_CHARS))),
            Err(SessionTokenError::Alphabet)
        );
    }

    #[test]
    fn a_token_matches_itself_and_nothing_else() {
        let minted = token(&"a".repeat(MIN_TOKEN_CHARS)).expect("a fixture token");

        assert!(minted.matches(minted.as_str()));
        // A prefix of the right length, a sibling, and the empty string are all refused — the last
        // one by the length fold rather than by an early return, which is the property the loop keeps.
        let mut wrong = "a".repeat(MIN_TOKEN_CHARS);
        wrong.replace_range(MIN_TOKEN_CHARS - 1.., "b");
        assert!(!minted.matches(&wrong));
        assert!(!minted.matches(&"a".repeat(MIN_TOKEN_CHARS - 1)));
        assert!(!minted.matches(""));
        assert!(!minted.matches(&"a".repeat(MAX_TOKEN_CHARS)));
    }

    #[test]
    fn a_token_does_not_print_itself() {
        // The assertion that makes the manual `Debug` worth having: the type system is the only thing
        // that can stop a bearer credential reaching a log line, and this is what would notice if
        // somebody derived it.
        let minted = token(&"abc123".repeat(6)).expect("a fixture token");

        assert_eq!(format!("{minted:?}"), "SessionToken(<redacted>)");
        assert!(
            !format!("{minted:?}").contains(minted.as_str()),
            "the value must not survive a formatting call"
        );
    }
}
