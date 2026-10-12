//! A clock under the test's control.

use std::sync::atomic::{AtomicI64, Ordering};

use jiff::Timestamp;
use peregrine_domain::Clock;

/// The instant [`FakeClock::default`] starts at, matching the value the hand-rolled fake
/// clocks in `peregrine-sync` and `peregrine-app` were pinned to.
pub const DEFAULT_EPOCH_MILLISECONDS: i64 = 1_700_000_000_000;

/// A clock that reads the instant it was given and does not move on its own.
///
/// The port deliberately offers no way to control time, because a production clock must
/// not have one; this fake offers the two handles a test needs. `advance` decides when the
/// next reading differs, so no test has to wait for a real millisecond to pass, and
/// [`FakeClock::now_millis`] reads the current instant back as a plain number, so a test
/// can assert on it and on the instants a service recorded.
///
/// The state is an `AtomicI64` rather than a `Cell` or a `Mutex`: the scheduler holds its
/// clock as `Arc<dyn Clock + Send + Sync>`, so one fake is shared across the threads a
/// test's runtime runs work on, and a value that small costs nothing to move atomically.
/// `Relaxed` is enough here — this is a test fixture, and no ordering in a test depends on
/// the clock being a synchronisation point.
#[derive(Debug)]
pub struct FakeClock {
    now: AtomicI64,
}

impl FakeClock {
    /// A clock pinned to `milliseconds` since the Unix epoch.
    pub fn at(milliseconds: i64) -> Self {
        Self {
            now: AtomicI64::new(milliseconds),
        }
    }

    /// Reads the current instant as milliseconds since the Unix epoch.
    pub fn now_millis(&self) -> i64 {
        self.now.load(Ordering::Relaxed)
    }

    /// Moves the clock forward by `milliseconds`, returning the instant it now reads.
    ///
    /// Moving backwards would let a test observe a clock no production clock can produce,
    /// which would make the fake's record of what happened stop meaning anything.
    pub fn advance(&self, milliseconds: i64) -> Timestamp {
        let moved = self.now.fetch_add(milliseconds, Ordering::Relaxed) + milliseconds;
        Timestamp::from_millisecond(moved).expect("tests stay inside jiff's representable range")
    }
}

impl Default for FakeClock {
    /// A clock pinned to [`DEFAULT_EPOCH_MILLISECONDS`], which is not any machine's current
    /// time, so an assertion naming it cannot pass by reading the wall clock.
    fn default() -> Self {
        Self::at(DEFAULT_EPOCH_MILLISECONDS)
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_millisecond(self.now_millis())
            .expect("tests stay inside jiff's representable range")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_does_not_advance_on_its_own() {
        let clock = FakeClock::default();
        let first = clock.now();
        let second = clock.now();
        assert_eq!(first, second, "a frozen clock must not drift between reads");
        assert_eq!(first.as_millisecond(), DEFAULT_EPOCH_MILLISECONDS);
        assert_eq!(clock.now_millis(), DEFAULT_EPOCH_MILLISECONDS);
    }

    /// R14's hazard, pinned: a test that injects this clock reads the value it injected,
    /// never the machine's time. The instant is a date no machine running the suite
    /// reports, so a reading equal to it cannot have come from the wall clock.
    #[test]
    fn the_clock_cannot_serve_the_wall_clock() {
        let in_the_future = FakeClock::at(4_102_444_800_000);
        assert_eq!(in_the_future.now().as_millisecond(), 4_102_444_800_000);
        assert_eq!(in_the_future.now(), in_the_future.now());
        assert_eq!(in_the_future.advance(0).as_millisecond(), 4_102_444_800_000);
    }

    #[test]
    fn advance_is_the_only_thing_that_moves_it() {
        let clock = FakeClock::default();
        let moved = clock.advance(500);
        assert_eq!(moved.as_millisecond(), DEFAULT_EPOCH_MILLISECONDS + 500);
        assert_eq!(
            clock.now(),
            moved,
            "the clock reads the instant it moved to"
        );

        // Zero is the boundary: a no-op advance must leave the instant exactly where it
        // was, because a caller reporting "no time passed" must not move a deadline.
        clock.advance(0);
        assert_eq!(clock.now(), moved);
    }

    #[test]
    fn advances_accumulate() {
        let clock = FakeClock::at(1_000);
        clock.advance(1);
        clock.advance(2);
        assert_eq!(clock.now_millis(), 1_003);
    }
}
