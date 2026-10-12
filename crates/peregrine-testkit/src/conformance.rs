//! The conformance suite: what any `Clock` and any `IdGenerator` must satisfy.
//!
//! A fake is only worth sharing if it and the adapter it stands in for answer to the same
//! rules, so the rules are written here once and run twice. The fake runs them in this
//! crate's tests; `peregrine-platform` runs them against `SystemClock` and `UlidGenerator`
//! in its own, which is what `Cargo.toml`'s dev-dependency on this crate is for. The edge
//! is one-way — this crate knows `peregrine-domain`, never an adapter — so there is no
//! cycle and `peregrine-platform`'s shipping build is unchanged.
//!
//! The suite observes only the port. It reads no wall clock, takes no lock, and sleeps
//! nowhere, because a suite that needed real time to pass could not be run against a
//! frozen fake — and the fake is the only implementation whose instant a test can pin.

use std::collections::BTreeSet;

use jiff::Timestamp;
use ulid::Ulid;

/// Identifiers the suite draws from an [`IdGenerator`](peregrine_domain::IdGenerator) before it is satisfied.
///
/// Four would catch a generator that returns a constant; this many also catches one that
/// cycles through a short list, and 64 ULIDs are nothing to hold.
const CONFORMING_DRAWS: usize = 64;

/// A set of the identifiers a run drew, which is how the suite reports what it saw.
type Set<T> = BTreeSet<T>;

/// The project's first commit, in milliseconds. A clock reporting seconds, microseconds,
/// or a different epoch lands below it.
const PROJECT_FLOOR_MILLISECONDS: i64 = 1_600_000_000_000;

/// Asserts that `now` satisfies the [`Clock`](peregrine_domain::Clock) contract, and
/// returns the first instant read.
///
/// Every read must land after the project's first commit and no read may go backwards,
/// which is the property a journal's ordering rests on. The suite is silent about whether
/// two reads *differ*: a coarse clock may legitimately answer twice from the same tick, so
/// requiring real time to have passed would make the suite flaky for the one
/// implementation that reads a real clock.
///
/// # Panics
///
/// Panics with the offending instant when the contract is broken.
pub fn clock_satisfies_the_port(now: impl Fn() -> Timestamp) -> Timestamp {
    let first = now();
    assert!(
        first.as_millisecond() > PROJECT_FLOOR_MILLISECONDS,
        "{first} is before the project existed, which a clock returning the wrong unit \
         would look like"
    );
    let second = now();
    assert!(
        second >= first,
        "a later read went backwards: {first} then {second}"
    );
    let third = now();
    assert!(
        third >= second,
        "a later read went backwards: {second} then {third}"
    );
    first
}

/// Asserts that `next_id` satisfies the [`IdGenerator`](peregrine_domain::IdGenerator)
/// contract.
///
/// Successive draws must differ: the scheduler asks for an identifier per submission, and
/// a repeat collides in its dedupe index. Ordering is deliberately *not* asserted — a
/// ULID's leading bits are its millisecond, so two of them minted in the same millisecond
/// order arbitrarily, and a suite demanding a strict order would be asserting something
/// the production generator does not promise.
///
/// # Panics
///
/// Panics naming the repeated identifier when the contract is broken.
pub fn id_generator_satisfies_the_port(next_id: impl Fn() -> Ulid) -> Set<Ulid> {
    let drawn: Vec<Ulid> = (0..CONFORMING_DRAWS).map(|_| next_id()).collect();
    let unique: Set<Ulid> = drawn.iter().copied().collect();
    assert_eq!(
        unique.len(),
        drawn.len(),
        "a repeated identifier would collide in the dedupe index"
    );
    unique
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{CountingIds, FakeClock};
    use peregrine_domain::{Clock, IdGenerator};

    /// The fake must pass the same suite the production adapter passes. If this fails, the
    /// fake is not a stand-in for the port and every test built on it is measuring
    /// something else.
    #[test]
    fn the_fake_clock_satisfies_the_port() {
        let clock = FakeClock::default();
        let observed = clock_satisfies_the_port(|| clock.now());
        assert_eq!(observed.as_millisecond(), clock.now_millis());
    }

    /// Determinism is what makes the suite itself runnable against the fake: the pinned
    /// instant survives the whole suite, so nothing here needed a real clock.
    #[test]
    fn the_fake_clock_leaves_time_where_it_was() {
        let clock = FakeClock::at(1_700_000_000_123);
        let observed = clock_satisfies_the_port(|| clock.now());
        assert_eq!(observed.as_millisecond(), 1_700_000_000_123);
        assert_eq!(clock.now_millis(), 1_700_000_000_123);
    }

    #[test]
    fn the_fake_ids_satisfy_the_port() {
        let generator = CountingIds::default();
        let drawn = id_generator_satisfies_the_port(|| generator.next_id());
        assert_eq!(drawn.len(), CONFORMING_DRAWS);
        assert_eq!(generator.issued(), CONFORMING_DRAWS);
    }

    /// The suite is only worth running if a broken implementation fails it, so both halves
    /// are pinned against one that is broken on purpose.
    #[test]
    fn a_repeating_id_generator_fails_the_suite() {
        let outcome = std::panic::catch_unwind(|| {
            id_generator_satisfies_the_port(|| Ulid::from_parts(1_700_000_000_000, 0));
        });
        assert!(outcome.is_err(), "a constant generator must not conform");
    }

    #[test]
    fn a_clock_repeating_the_same_instant_conforms() {
        // A coarse clock is not a broken clock: repeating an instant is permitted, and the
        // suite must not be the thing that forbids it.
        let outcome = std::panic::catch_unwind(|| {
            clock_satisfies_the_port(|| Timestamp::from_millisecond(1_700_000_000_000).unwrap())
        });
        assert!(outcome.is_ok(), "a coarse clock is still a clock");
    }
}
