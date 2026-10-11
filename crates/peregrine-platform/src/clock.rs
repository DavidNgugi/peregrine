//! The production clock.
//!
//! `peregrine-domain` defines [`Clock`] and forbids itself from reading a real time, so
//! the one place that reads the machine's clock is here, behind the port. Everything
//! else — services, jobs, repositories — takes `dyn Clock` and is therefore testable
//! against a fixed instant.

use jiff::Timestamp;
use peregrine_domain::Clock;

/// Reads the system's wall clock.
///
/// Wall-clock time is what the journal and the UI need; anything that measures a
/// *duration* waits on the monotonic clock instead, which is why the scheduler's retry
/// timer is a `tokio::time::Instant` and not this.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_clock_returns_a_plausible_instant() {
        let now = SystemClock.now();
        // The project's first commit is well after this, and no machine running the tests
        // has a clock set before it; the lower bound is what catches a unit mistake such
        // as seconds returned as milliseconds.
        let floor = Timestamp::from_millisecond(1_600_000_000_000).unwrap();
        assert!(now > floor, "{now} is before the project existed");
    }

    /// Two reads must not go backwards, which is the property a journal's ordering
    /// depends on even though nothing here guarantees a monotonic clock.
    #[test]
    fn two_reads_do_not_go_backwards() {
        let first = SystemClock.now();
        let second = SystemClock.now();
        assert!(second >= first, "{second} is before {first}");
    }
}
