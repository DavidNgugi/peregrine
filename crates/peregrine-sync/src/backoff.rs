//! The retry schedule a deferred job follows.
//!
//! The curve is deterministic and the jitter is derived from the job's own identity and
//! attempt number rather than from an entropy source. That is deliberate: a retry
//! schedule that no test can predict is a retry schedule nobody can assert, and
//! `PG-E6-01`'s acceptance is "deferred jobs retry on schedule". Swapping in an
//! entropy-backed jitter later changes one function.

use std::time::Duration;

use ulid::Ulid;

/// How a deferred job's delay grows with each attempt.
#[derive(Debug, Clone, Copy)]
pub struct Backoff {
    /// The delay before the second attempt.
    pub initial: Duration,
    /// The factor applied per additional attempt.
    pub multiplier: f64,
    /// The ceiling on the exponential part, before jitter.
    pub max: Duration,
    /// The fraction of the delay that jitter may add or remove, in `0.0..=1.0`.
    pub jitter_ratio: f64,
}

impl Default for Backoff {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Backoff {
    /// The default curve: 500 ms doubling to a minute, with ±20% jitter.
    pub(crate) const DEFAULT: Self = Self {
        initial: Duration::from_millis(500),
        multiplier: 2.0,
        max: Duration::from_mins(1),
        jitter_ratio: 0.2,
    };

    /// The delay before attempt number `attempt`, counted from one.
    ///
    /// The result is never zero, and never exceeds `max * (1 + jitter_ratio)`, so a
    /// scheduler can always arm a timer for it and a caller can always bound its wait.
    pub fn delay_for(&self, job: Ulid, attempt: u32) -> Duration {
        let exponent = attempt.saturating_sub(1);
        // `powi` saturates to infinity rather than panicking, and the `min` below pulls
        // that back under the ceiling, so a large attempt count cannot overflow here.
        // A saturating exponent: past the ceiling every further attempt is the same
        // delay, so the exact power stops mattering and cannot overflow an `i32`.
        let power = i32::try_from(exponent).unwrap_or(i32::MAX);
        let scaled = self.initial.as_secs_f64() * self.multiplier.powi(power);
        let capped = scaled.min(self.max.as_secs_f64());
        if !capped.is_finite() || capped <= 0.0 {
            return self.max;
        }
        let jitter = self.jitter_ratio.clamp(0.0, 1.0);
        let spread = jitter * (unit_interval(job, attempt) * 2.0 - 1.0);
        let seconds = (capped * (1.0 + spread)).max(0.001);
        Duration::from_secs_f64(seconds.min(self.max.as_secs_f64() * (1.0 + jitter)))
    }
}

/// A deterministic value in `[0, 1)` derived from the job and the attempt.
///
/// `SplitMix64` over the ULID's 128 bits: cheap, seeded entirely by `(job, attempt)`, and
/// without the correlation between consecutive seeds that a plain counter would show.
fn unit_interval(job: Ulid, attempt: u32) -> f64 {
    let mut state = job.0 ^ (u128::from(attempt).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let mut next = || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let high = next();
    let low = next();
    let combined = (high << 64) | (low & u128::from(u64::MAX));
    // The top 53 bits are the ones an `f64` can represent exactly, which keeps the
    // mapping onto `0.0..1.0` monotone and avoids rounding onto the closed end.
    ((combined >> 75) as f64) / ((1_u64 << 53) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job() -> Ulid {
        Ulid::from_parts(1_700_000_000_000, 7)
    }

    #[test]
    fn a_delay_grows_with_each_attempt_and_stops_at_the_ceiling() {
        let backoff = Backoff::default();
        let first = backoff.delay_for(job(), 1);
        let second = backoff.delay_for(job(), 2);
        let third = backoff.delay_for(job(), 3);
        assert!(
            first < second,
            "{first:?} should be shorter than {second:?}"
        );
        assert!(
            second < third,
            "{second:?} should be shorter than {third:?}"
        );

        // The jittered ceiling is the largest delay the curve can produce.
        let ceiling = Duration::from_secs_f64(60.0 * 1.2);
        for attempt in 1..=64 {
            let delay = backoff.delay_for(job(), attempt);
            assert!(
                delay >= Duration::from_millis(1) && delay <= ceiling,
                "attempt {attempt} produced {delay:?}, outside 1 ms..={ceiling:?}"
            );
        }
    }

    #[test]
    fn jitter_is_deterministic_per_job_and_attempt() {
        let backoff = Backoff::DEFAULT;
        assert_eq!(backoff.delay_for(job(), 3), backoff.delay_for(job(), 3));
        // Different jobs of the same attempt get different delays, or the schedule
        // would retry a thundering herd of deferred jobs at the same instant.
        let other = Ulid::from_parts(1_700_000_000_000, 8);
        assert_ne!(backoff.delay_for(job(), 3), backoff.delay_for(other, 3));
    }

    #[test]
    fn jitter_stays_within_the_declared_ratio_of_the_exponential_delay() {
        let backoff = Backoff {
            initial: Duration::from_secs(1),
            multiplier: 2.0,
            max: Duration::from_mins(1),
            jitter_ratio: 0.2,
        };
        let base = 1_000.0;
        for attempt in 1..=8 {
            let delay = backoff.delay_for(job(), attempt).as_secs_f64() * 1_000.0;
            // The ceiling clamps the exponential part first and jitter is applied to the
            // clamped value, so it is the *clamped* delay that the ratio bounds.
            let exponent = i32::try_from(attempt.saturating_sub(1)).unwrap_or(i32::MAX);
            let expected = (base * 2_f64.powi(exponent)).min(60_000.0);
            let low = expected * 0.8;
            let high = expected * 1.2;
            assert!(
                delay >= low - 1.0 && delay <= high + 1.0,
                "attempt {attempt}: {delay} outside {low}..{high}"
            );
        }
    }

    #[test]
    fn a_zero_jitter_ratio_is_the_plain_exponential_curve() {
        let backoff = Backoff {
            jitter_ratio: 0.0,
            ..Backoff::DEFAULT
        };
        for attempt in 1..=6 {
            let expected = Duration::from_millis(500) * 2_u32.pow(attempt - 1);
            assert_eq!(
                backoff.delay_for(job(), attempt),
                expected,
                "attempt {attempt}"
            );
        }
    }
}
