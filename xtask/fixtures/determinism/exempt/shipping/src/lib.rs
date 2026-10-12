//! Shipping code that measures durations and leaves the wall clock to its tests.
//!
//! If the gate read test-only code or reported the monotonic clock, this crate
//! would be a violation.

use std::time::Instant;

pub fn elapsed_ms(start: Instant) -> u128 {
    Instant::now().duration_since(start).as_millis()
}

pub fn tokio_elapsed_ms(start: tokio::time::Instant) -> u128 {
    tokio::time::Instant::now().duration_since(start).as_millis()
}

#[cfg(test)]
mod pinned {
    #[test]
    fn a_test_may_read_the_wall_clock() {
        let _ = jiff::Timestamp::now();
    }
}

#[cfg(test)]
mod tests;
