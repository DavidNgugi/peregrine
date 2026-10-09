//! Ports for the two things a pure domain may not reach for itself: the current
//! time and fresh identifiers.
//!
//! Adapters own the implementations that touch a real clock or a random source;
//! everything here is injected, so a test can pin both without a machine.

use jiff::Timestamp;
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
}
