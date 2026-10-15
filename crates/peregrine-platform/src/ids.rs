//! The production identifier and session-token generators.
//!
//! `peregrine-domain` defines [`IdGenerator`] and [`TokenSource`] and forbids itself from touching a
//! random source, so the one place in shipping code that draws on one is here, behind the ports. It
//! sits beside [`SystemClock`](crate::SystemClock) because both are the machine-dependent half of the
//! same rule: everything else takes a `dyn IdGenerator` or a `dyn TokenSource` and is therefore
//! predictable in a test.

use peregrine_domain::{IdGenerator, SessionToken, TokenSource};
use ulid::Ulid;

/// Issues ULIDs from the process's random source and the system's wall clock.
///
/// A ULID's leading bits are its timestamp, so generated identifiers sort by issue order
/// to millisecond resolution. Anything that needs a strict ordering must not rely on that
/// resolution: two identifiers minted in the same millisecond order arbitrarily.
#[derive(Debug, Default, Clone, Copy)]
pub struct UlidGenerator;

impl IdGenerator for UlidGenerator {
    fn next_id(&self) -> Ulid {
        Ulid::generate()
    }
}

/// Issues session tokens from the process's random source.
///
/// **Two ULIDs end to end, and the redundancy is the point.** A single ULID carries 80 random bits,
/// which is not enough for a bearer credential; two carry 160, which is. The timestamps they also
/// carry reveal when the session was opened, and that is not a leak worth avoiding here: the socket
/// dies with the session (ADR 0023 §2), so "when was this token minted" is a question the session's
/// own lifetime already answers.
///
/// `ulid` is used rather than a direct `getrandom` call because it is the random source this crate is
/// already permitted (`arch-allowlist.toml`'s `[determinism]` section names it as the adapter), and a
/// second path to the same pool would be a second thing to audit.
#[derive(Debug, Default, Clone, Copy)]
pub struct TokenMinter;

impl TokenSource for TokenMinter {
    fn next_token(&self) -> SessionToken {
        // Infallible by construction rather than by hope: two ULIDs are 52 ASCII alphanumerics, and
        // the alphabet and the 32..=128 length bound are what `SessionToken::new` checks. There is no
        // input to get wrong, so a fallible signature here would only push an impossible error into
        // every caller (AGENTS §5's provably-infallible case).
        SessionToken::new(format!("{}{}", Ulid::generate(), Ulid::generate()))
            .expect("two ULIDs are 52 ASCII alphanumerics, which is a valid token by construction")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two draws must differ. The generator is called by the scheduler for every
    /// submission, and a repeated identifier would collide in the dedupe index. The
    /// assertion lives in `peregrine-testkit` because the fake generator answers to it
    /// too, so the two implementations cannot drift apart unobserved (PG-E4-19).
    #[test]
    fn the_ulid_generator_satisfies_the_port() {
        peregrine_testkit::id_generator_satisfies_the_port(|| UlidGenerator.next_id());
    }

    #[test]
    fn two_minted_tokens_are_different_and_usable_as_a_path_segment() {
        // The property the server depends on: a token is unique per session, and it can be put in a
        // URL without escaping. Both are checked here rather than in the server, because a minter
        // that produced one repeated value would be a hole in every session at once.
        let first = TokenMinter.next_token();
        let second = TokenMinter.next_token();

        assert_ne!(first, second);
        assert!(first.matches(first.as_str()));
        assert!(
            !first.matches(second.as_str()),
            "one session's token must not open another's"
        );
        assert!(
            first
                .as_str()
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric()),
            "a token is never escaped into a URL: {}",
            first.as_str()
        );
        assert!(first.as_str().len() >= 32, "enough text for 160 bits");
    }
}
