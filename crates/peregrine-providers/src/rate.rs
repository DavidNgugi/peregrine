//! Per-host rate limiting, built from the policy a provider declares (PG-E5-03).
//!
//! `docs/research/data-sources.md` records what each provider actually allows, and AGENTS R24
//! turns it into a rule: per-host rate limits that back off on `429`. This module is the
//! mechanism behind both. A caller asks [`RateLimiter::reserve`] how long to wait before its
//! next request to a host and gets a [`Duration`] back — the limiter computes, the caller
//! waits.
//!
//! # Why it answers with a duration instead of sleeping
//!
//! Sleeping would make this an asynchronous type and put a runtime in a crate that has none:
//! the port is synchronous and object safe (PG-E5-01), the scheduler already owns the async
//! side (PG-E6-01), and a limiter whose state can be advanced by an injected instant is one a
//! test can drive through an hour of traffic in microseconds. It also keeps
//! `cargo xtask check-determinism` satisfied without an exemption: nothing here reads a clock,
//! because the caller passes the instant it already has.
//!
//! # The arithmetic is integer, and rounds the safe way
//!
//! Token accrual is computed in whole milliseconds and the remainder of a division is
//! discarded, which always *under*-credits. A limiter that under-credits is a limiter that
//! asks the caller to wait slightly longer than the provider requires; one that over-credits
//! is a limiter that gets the user rate limited. Only one of those is a failure.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use jiff::Timestamp;

use crate::descriptor::RatePolicy;

/// What one host's bucket remembers.
#[derive(Debug, Clone, Copy)]
struct Bucket {
    /// Tokens available right now.
    tokens: u32,
    /// The instant the tokens were last credited, which is what accrual is measured from.
    ///
    /// It advances by exactly the time the credited tokens were worth, so the fraction of a
    /// token's worth of time left over stays in the elapsed-time term rather than being lost.
    credited_at_ms: i64,
    /// A provider's own instruction to stay away until this instant, from a `Retry-After`.
    ///
    /// Kept separately from the tokens because it is not a rate: the provider said when it
    /// will answer again, and no amount of elapsed time changes what it said.
    parked_until_ms: i64,
}

/// The token buckets, one per host.
///
/// One limiter covers the whole application rather than one per provider, because a provider
/// may answer from several hosts — a JSON API and an image CDN are the documented case, and
/// they have different terms — and because the limit belongs to the host that enforces it.
#[derive(Debug, Default)]
pub struct RateLimiter {
    hosts: Mutex<BTreeMap<String, Bucket>>,
}

impl RateLimiter {
    /// A limiter that has seen no traffic.
    #[must_use]
    pub fn new() -> Self {
        Self {
            hosts: Mutex::new(BTreeMap::new()),
        }
    }

    /// How long the caller must wait before its next request to `host`.
    ///
    /// [`Duration::ZERO`] means the request may start now. A host seen for the first time
    /// starts with a full burst, so the first request to a new provider is never delayed.
    ///
    /// The reservation is *not* held against a future instant: a caller that is told to wait
    /// asks again when it wakes. Two callers that ask at the same instant are therefore told
    /// the same thing and one of them will be told to wait again — which loses no rate
    /// compliance and needs no request identity to cancel, and the client that serialises
    /// them is PG-E5-02's.
    pub fn reserve(&self, host: &str, policy: &RatePolicy, now: Timestamp) -> Duration {
        let now_ms = now.as_millisecond();
        let mut hosts = self.lock();

        let bucket = hosts.entry(host.to_owned()).or_insert(Bucket {
            tokens: policy.burst().get(),
            credited_at_ms: now_ms,
            parked_until_ms: i64::MIN,
        });

        if now_ms < bucket.parked_until_ms {
            return Duration::from_millis(elapsed_millis(now_ms, bucket.parked_until_ms));
        }

        credit(bucket, policy, now_ms);

        if bucket.tokens > 0 {
            bucket.tokens -= 1;
            return Duration::ZERO;
        }

        // No token: the wait is what is left of the current token's worth of time. The accrual
        // above either credited something (and then a token would be available) or credited
        // nothing, so the elapsed time here is strictly less than one token's spacing.
        // Ceiling division, because a token is credited when a *whole* token's worth of time
        // has passed: with 90 requests a minute the spacing is 667 ms and not 666, and using
        // the truncated figure would answer "go now" at 666 ms — an instant at which the
        // accrual above credited nothing. That answer would be a permission the limiter has
        // not granted, and it is the one failure this type exists to prevent.
        let window_ms = duration_millis(policy.window());
        let spacing_ms = window_ms.div_ceil(u64::from(policy.requests().get()));
        let elapsed_ms = elapsed_millis(bucket.credited_at_ms, now_ms);

        // The floor is a second guard on the same invariant: the refusing path may never
        // answer zero, whatever the arithmetic says.
        Duration::from_millis(spacing_ms.saturating_sub(elapsed_ms)).max(MINIMUM_WAIT)
    }

    /// Parks `host` until `until`, because the provider said so.
    ///
    /// This is what a `429` with `Retry-After` means: the provider named an instant, and no
    /// amount of elapsed time before it changes the answer. A provider that sends no
    /// `Retry-After` is not parked here — the bucket's own refill is what makes the caller
    /// wait, which is why [`RatePolicy::honours_retry_after`] is a declaration the client
    /// reads rather than something this type decides.
    ///
    /// A park that is already in the past changes nothing, so a stale header cannot shorten a
    /// longer park that is already in force.
    pub fn park(&self, host: &str, policy: &RatePolicy, until: Timestamp) {
        let until_ms = until.as_millisecond();
        let mut hosts = self.lock();

        let bucket = hosts.entry(host.to_owned()).or_insert(Bucket {
            // A host parked before it was ever used starts as a fresh one, credited from the
            // instant the park ends: a park delays the next request, and it must not also
            // spend the burst on it, or a single `Retry-After` would cost the client the
            // allowance the provider's own policy grants it.
            tokens: policy.burst().get(),
            credited_at_ms: until_ms,
            parked_until_ms: until_ms,
        });

        bucket.parked_until_ms = bucket.parked_until_ms.max(until_ms);
    }

    /// How many hosts have been seen, which is what the diagnostics screen lists (PG-E5-22).
    #[must_use]
    pub fn tracked_hosts(&self) -> usize {
        self.lock().len()
    }

    /// Takes the lock, ignoring poisoning.
    ///
    /// The state behind it is a map of counters with no invariant that a panic could break: a
    /// limiter that recovered from another thread's panic is a limiter that still answers
    /// correctly, and refusing to answer would turn one thread's bug into every request
    /// failing.
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Bucket>> {
        self.hosts.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The shortest wait the limiter will name.
///
/// The refusing path is never allowed to answer [`Duration::ZERO`], because zero means "go
/// now" to every caller and a busy loop is the mildest thing that produces. A millisecond
/// floor is also a rate statement — at most a thousand requests a second per host — which is
/// far above anything `docs/research/data-sources.md` records a provider allowing.
const MINIMUM_WAIT: Duration = Duration::from_millis(1);

/// Whole milliseconds from `from` to `to`, and zero for a clock that went backwards.
///
/// `to - from` is negative when the caller's clock stepped back — an NTP correction, a
/// resumed laptop — and treating that as elapsed time would *mint* tokens for a client that
/// has waited for none of them. Saturating at zero is the only safe reading.
fn elapsed_millis(from_ms: i64, to_ms: i64) -> u64 {
    u64::try_from(to_ms.saturating_sub(from_ms)).unwrap_or(0)
}

/// Credits the tokens `policy` has earned since the bucket was last credited.
fn credit(bucket: &mut Bucket, policy: &RatePolicy, now_ms: i64) {
    let elapsed_ms = elapsed_millis(bucket.credited_at_ms, now_ms);
    if elapsed_ms == 0 {
        return;
    }

    let window_ms = duration_millis(policy.window());
    let requests = u64::from(policy.requests().get());
    // Whole tokens only, and the division discards the remainder: see the module doc.
    let earned = elapsed_ms.saturating_mul(requests) / window_ms;
    if earned == 0 {
        return;
    }

    let earned = u32::try_from(earned).unwrap_or(u32::MAX);
    bucket.tokens = bucket
        .tokens
        .saturating_add(earned)
        .min(policy.burst().get());
    // Advance the credit instant by exactly what was credited, so the time that earned no
    // token stays in the elapsed term.
    bucket.credited_at_ms = bucket.credited_at_ms.saturating_add(
        i64::try_from(u64::from(earned) * window_ms / requests).unwrap_or(i64::MAX),
    );
}

/// A `Duration` in whole milliseconds, saturating at the largest representable value.
fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::time::Duration;

    use jiff::Timestamp;

    use super::*;

    const HOST: &str = "api.themoviedb.org";

    fn at(milliseconds: i64) -> Timestamp {
        Timestamp::from_millisecond(milliseconds).expect("the tests stay inside jiff's range")
    }

    fn non_zero(value: u32) -> NonZeroU32 {
        NonZeroU32::new(value).expect("the tests pass a non-zero value")
    }

    /// TMDB's documented shape: a sustained rate with a burst allowance.
    fn policy(requests: u32, window: Duration, burst: u32) -> RatePolicy {
        RatePolicy::new(non_zero(requests), window, non_zero(burst), true)
            .expect("the tests pass a well-formed policy")
    }

    fn per_second(requests: u32, burst: u32) -> RatePolicy {
        policy(requests, Duration::from_secs(1), burst)
    }

    #[test]
    fn a_new_host_starts_with_a_full_burst_and_is_never_delayed() {
        // The first request to a provider must not wait: nothing has been asked of it yet.
        let limiter = RateLimiter::new();
        let policy = per_second(5, 3);

        for _ in 0..3 {
            assert_eq!(limiter.reserve(HOST, &policy, at(0)), Duration::ZERO);
        }
    }

    #[test]
    fn the_burst_is_a_limit_and_not_a_starting_balance() {
        // The fourth call at the same instant has nothing left. The boundary is exact: three
        // go, the fourth waits.
        let limiter = RateLimiter::new();
        let policy = per_second(5, 3);
        for _ in 0..3 {
            limiter.reserve(HOST, &policy, at(0));
        }

        let fourth = limiter.reserve(HOST, &policy, at(0));

        assert_eq!(fourth, Duration::from_millis(200));
    }

    #[test]
    fn a_token_comes_back_when_its_time_has_passed() {
        let limiter = RateLimiter::new();
        let policy = per_second(5, 1);
        assert_eq!(limiter.reserve(HOST, &policy, at(0)), Duration::ZERO);

        // Five a second is one every 200 ms.
        assert!(limiter.reserve(HOST, &policy, at(199)) > Duration::ZERO);
        assert_eq!(limiter.reserve(HOST, &policy, at(200)), Duration::ZERO);
    }

    #[test]
    fn a_synthetic_burst_never_exceeds_the_configured_rate() {
        // The acceptance, driven the way a test can: one call every millisecond for ten
        // seconds against five requests per second with a burst of three. The number that
        // start immediately can be at most the burst plus the tokens the second earned.
        let limiter = RateLimiter::new();
        let policy = per_second(5, 3);
        let mut immediate = 0_u32;

        for millisecond in 0..10_000 {
            if limiter.reserve(HOST, &policy, at(millisecond)) == Duration::ZERO {
                immediate += 1;
            }
        }

        // 10 seconds at 5/s is 50 tokens, plus the initial burst of 3.
        assert!(
            immediate <= 53,
            "{immediate} requests were allowed in ten seconds at five per second with a burst of three"
        );
        // And not wildly under, or the limiter would be refusing traffic the provider allows.
        assert!(immediate >= 50, "only {immediate} requests were allowed");
    }

    #[test]
    fn hosts_are_limited_independently() {
        // The reason the buckets are keyed by host at all: an image CDN's terms are not the
        // JSON API's, and exhausting one must not stop the other.
        let limiter = RateLimiter::new();
        let policy = per_second(1, 1);

        assert_eq!(
            limiter.reserve("api.themoviedb.org", &policy, at(0)),
            Duration::ZERO
        );
        assert_eq!(
            limiter.reserve("image.tmdb.org", &policy, at(0)),
            Duration::ZERO
        );
        assert!(limiter.reserve("api.themoviedb.org", &policy, at(0)) > Duration::ZERO);
        assert_eq!(limiter.tracked_hosts(), 2);
    }

    #[test]
    fn a_clock_that_goes_backwards_does_not_mint_tokens() {
        // The instant is the caller's, and a caller with a clock that jitters — an NTP step, a
        // resumed laptop — must not be able to spend tokens it never earned.
        let limiter = RateLimiter::new();
        let policy = per_second(1, 1);
        limiter.reserve(HOST, &policy, at(1_000_000));
        assert!(limiter.reserve(HOST, &policy, at(1_000_000)) > Duration::ZERO);

        let backwards = limiter.reserve(HOST, &policy, at(0));

        assert!(backwards > Duration::ZERO, "time going back is not a token");
    }

    #[test]
    fn a_retry_after_parks_the_host_until_the_instant_it_named() {
        // What respecting a `429` means: the provider named an instant, and the limiter holds
        // it even though the bucket's own refill would have allowed a request long before.
        let limiter = RateLimiter::new();
        let policy = per_second(100, 100);
        limiter.park(HOST, &policy, at(5_000));

        assert_eq!(
            limiter.reserve(HOST, &policy, at(1_000)),
            Duration::from_secs(4)
        );
        assert_eq!(limiter.reserve(HOST, &policy, at(5_000)), Duration::ZERO);
    }

    #[test]
    fn a_park_already_in_the_past_cannot_shorten_one_that_is_in_force() {
        // A stale header — a retry, a replayed response — must not let a client talk its way
        // out of a park it is already serving.
        let limiter = RateLimiter::new();
        limiter.park(HOST, &per_second(100, 100), at(10_000));
        limiter.park(HOST, &per_second(100, 100), at(1_000));

        assert!(limiter.reserve(HOST, &per_second(100, 100), at(2_000)) > Duration::ZERO);
    }

    #[test]
    fn parking_a_host_that_was_never_used_still_parks_it() {
        // A `Retry-After` on the first response is the one instruction that must not be
        // ignored, so a park creates the bucket rather than needing one to exist.
        let limiter = RateLimiter::new();
        limiter.park("image.tmdb.org", &per_second(100, 100), at(2_000));

        let wait = limiter.reserve("image.tmdb.org", &per_second(100, 100), at(1_000));

        assert_eq!(wait, Duration::from_secs(1));
    }

    #[test]
    fn a_wait_is_never_zero_when_a_token_is_unavailable() {
        // A wait of zero on the refusing path would be a busy loop: the caller would be told
        // to go now, be refused, and be told to go now again.
        let limiter = RateLimiter::new();
        let policy = policy(1000, Duration::from_secs(300), 1);
        limiter.reserve(HOST, &policy, at(0));

        assert!(limiter.reserve(HOST, &policy, at(0)) > Duration::ZERO);
    }

    #[test]
    fn a_rate_whose_spacing_is_not_a_whole_number_of_milliseconds_rounds_towards_waiting() {
        // Ninety a minute is one every 666.67 ms. The limiter credits only whole tokens over
        // whole milliseconds, so it under-credits rather than over-crediting — the direction
        // that asks the provider's forgiveness rather than its rate limiter.
        let limiter = RateLimiter::new();
        let policy = policy(90, Duration::from_secs(60), 1);
        assert_eq!(limiter.reserve(HOST, &policy, at(0)), Duration::ZERO);

        assert!(limiter.reserve(HOST, &policy, at(666)) > Duration::ZERO);
        assert_eq!(limiter.reserve(HOST, &policy, at(667)), Duration::ZERO);
    }

    #[test]
    fn a_very_high_rate_still_never_credits_more_than_the_burst() {
        // A long idle period must not bank an unbounded balance, or a client that has been
        // quiet for an hour would be allowed an hour's worth of requests at once.
        let limiter = RateLimiter::new();
        let policy = per_second(1000, 5);
        let hour = 60 * 60 * 1_000;

        for _ in 0..5 {
            assert_eq!(limiter.reserve(HOST, &policy, at(hour)), Duration::ZERO);
        }

        assert!(limiter.reserve(HOST, &policy, at(hour)) > Duration::ZERO);
    }
}
