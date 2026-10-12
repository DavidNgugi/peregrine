//! A deterministic source of identifiers.

use std::sync::atomic::{AtomicUsize, Ordering};

use peregrine_domain::IdGenerator;
use ulid::Ulid;

use crate::clock::DEFAULT_EPOCH_MILLISECONDS;

/// Identifiers handed out in a predictable order, with the count readable.
///
/// The Nth draw is `Ulid::from_parts(DEFAULT_EPOCH_MILLISECONDS, N)`, so a test can write
/// the identifier it expects rather than compare against one it captured earlier, and the
/// timestamp half is the same constant the default [`FakeClock`](crate::FakeClock) reads.
/// Two runs of the same test therefore produce the same identifiers, which is what makes a
/// recorded identifier stable enough to assert on (AGENTS R14).
///
/// [`CountingIds::issued`] is the observability the port lacks: production has no reason
/// to report how many identifiers it has minted, and a test asserting "three submissions
/// were given three distinct identities" does.
#[derive(Debug, Default)]
pub struct CountingIds {
    issued: AtomicUsize,
}

impl CountingIds {
    /// How many identifiers this generator has handed out so far.
    pub fn issued(&self) -> usize {
        self.issued.load(Ordering::Relaxed)
    }
}

impl IdGenerator for CountingIds {
    fn next_id(&self) -> Ulid {
        let issued = self.issued.fetch_add(1, Ordering::Relaxed);
        id_for(issued as u128)
    }
}

/// The identifier the generator mints for the `issued`-th draw.
fn id_for(issued: u128) -> Ulid {
    Ulid::from_parts(
        u64::try_from(DEFAULT_EPOCH_MILLISECONDS).unwrap_or_default(),
        issued,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sequence_is_predictable_and_observable() {
        let generator = CountingIds::default();
        assert_eq!(
            generator.issued(),
            0,
            "nothing is issued before the first draw"
        );
        assert_eq!(generator.next_id(), id_for(0));
        assert_eq!(generator.next_id(), id_for(1));
        assert_eq!(generator.next_id(), id_for(2));
        assert_eq!(
            generator.issued(),
            3,
            "the count is readable after the draws"
        );
    }

    /// Two generators built the same way replay the same sequence, which is what lets one
    /// test assert on an identifier another test recorded.
    #[test]
    fn two_generators_draw_the_same_sequence() {
        let first = CountingIds::default();
        let second = CountingIds::default();
        let from_first: Vec<Ulid> = (0..4).map(|_| first.next_id()).collect();
        let from_second: Vec<Ulid> = (0..4).map(|_| second.next_id()).collect();
        assert_eq!(from_first, from_second);
        assert_eq!(from_first, (0..4).map(id_for).collect::<Vec<_>>());
    }

    /// The fake's extra promise, and the reason it mints a monotonic sequence rather than
    /// a random one: identifiers sort by issue order, so a test can assert on ordering.
    #[test]
    fn issued_identifiers_sort_by_issue_order() {
        let generator = CountingIds::default();
        let first = generator.next_id();
        let second = generator.next_id();
        let third = generator.next_id();
        assert!(
            first < second && second < third,
            "got {first}, {second}, {third}"
        );
    }
}
