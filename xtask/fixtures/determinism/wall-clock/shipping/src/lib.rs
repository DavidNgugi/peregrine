//! A shipping crate that reaches for the wall clock and a random source itself.
//!
//! The two `Instant` readings are here on purpose: the gate must not treat the
//! monotonic clock as a wall clock.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use ulid::Ulid;

pub fn started_at() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub fn recorded_at() -> jiff::Timestamp {
    jiff::Timestamp::now()
}

pub fn fresh_id() -> Ulid {
    Ulid::generate()
}

pub fn elapsed_ms(start: Instant) -> u128 {
    Instant::now().duration_since(start).as_millis()
}

pub fn tokio_elapsed_ms(start: tokio::time::Instant) -> u128 {
    tokio::time::Instant::now().duration_since(start).as_millis()
}
