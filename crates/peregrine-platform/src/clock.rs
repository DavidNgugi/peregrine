//! The production clock.
//!
//! `peregrine-domain` defines [`Clock`] and forbids itself from reading a real time, so
//! the one place in shipping code that reads the machine's clock is here, behind the
//! port; `cargo xtask check-determinism` fails the build on a wall-clock read anywhere
//! else. Everything else — services, jobs, repositories — takes `dyn Clock` and is
//! therefore testable against a fixed instant.

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

    /// The adapter passes the same suite the fake passes, which is what makes the fake a
    /// stand-in for it rather than a second opinion (PG-E4-19). What the suite asserts —
    /// the floor that catches a unit mistake such as seconds returned as milliseconds, and
    /// that repeated reads never go backwards — is stated once, in `peregrine-testkit`.
    #[test]
    fn the_system_clock_satisfies_the_port() {
        peregrine_testkit::clock_satisfies_the_port(|| SystemClock.now());
    }
}
