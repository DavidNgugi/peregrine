//! The image cache's eviction policy: what to forget when the cache is over its cap.
//!
//! The store is content-addressed and every entry can be fetched again, so the question is not
//! whether something may be deleted but *which* — and the answer is a rule, which is why it is here
//! rather than in the statement that reads the rows.
//!
//! Three properties the rule has to have, in the order they matter:
//!
//! - **An in-use entry is never evicted.** `pinned` is the mark a caller sets while an image is
//!   being displayed or is part of a running operation, and the policy skips those entirely. A cache
//!   that evicted what was on screen would be a cache whose cap the user cannot meet.
//! - **Least recently used goes first.** `last_used_at` is the order, and it is the only thing that
//!   distinguishes an entry somebody is still coming back to from one nobody has wanted in a month.
//! - **The result is deterministic.** Two entries touched at the same instant are ordered by key, so
//!   the same cache and the same cap always evict the same set: a policy that depended on row order
//!   would make a test that passes today fail tomorrow.

/// One entry of the cache, as the policy sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEntry {
    /// The content-addressed key, which is what a removal names.
    pub key: String,
    /// What the entry's blob weighs.
    pub bytes: i64,
    /// When it was last used, in Unix milliseconds.
    pub last_used_at: i64,
    /// Whether something is using it, in which case it is never evicted.
    pub pinned: bool,
}

/// What one eviction run would remove.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvictionPlan {
    /// The keys to remove, in the order they were chosen.
    pub keys: Vec<String>,
    /// What those entries weigh together.
    pub bytes: i64,
    /// Whether the cap cannot be met without evicting a pinned entry.
    ///
    /// Reported rather than acted on: pinned entries are in use, and a policy that broke that promise
    /// to satisfy a number would be trading a correctness guarantee for a target. A caller that sees
    /// this knows the cap is too small for what the application is holding.
    pub cap_unreachable: bool,
}

/// Chooses what to evict so the cache fits `cap_bytes`.
///
/// `entries` is the whole cache, and `cap_bytes` is what it is allowed to weigh. The plan removes the
/// least recently used unpinned entries until the total fits, and reports `cap_unreachable` when it
/// cannot — which is what a cache of pinned entries larger than its cap looks like.
///
/// A cap of zero or less means "nothing may be cached", which is a real setting rather than a
/// mistake: the plan then removes every unpinned entry.
#[must_use]
pub fn plan_eviction(entries: &[CacheEntry], cap_bytes: i64) -> EvictionPlan {
    let total: i64 = entries.iter().map(|entry| entry.bytes).sum();
    if total <= cap_bytes {
        return EvictionPlan::default();
    }

    let mut candidates: Vec<&CacheEntry> = entries.iter().filter(|entry| !entry.pinned).collect();
    // Oldest use first, and the key breaks a tie so the same input always yields the same plan.
    candidates.sort_by(|left, right| {
        left.last_used_at
            .cmp(&right.last_used_at)
            .then_with(|| left.key.cmp(&right.key))
    });

    let mut plan = EvictionPlan::default();
    let mut remaining = total;
    for entry in candidates {
        if remaining <= cap_bytes {
            break;
        }
        plan.keys.push(entry.key.clone());
        plan.bytes += entry.bytes;
        remaining -= entry.bytes;
    }
    plan.cap_unreachable = remaining > cap_bytes;
    plan
}

#[cfg(test)]
mod tests {
    use super::{CacheEntry, plan_eviction};

    fn entry(key: &str, bytes: i64, last_used_at: i64, pinned: bool) -> CacheEntry {
        CacheEntry {
            key: key.to_owned(),
            bytes,
            last_used_at,
            pinned,
        }
    }

    #[test]
    fn a_cache_inside_its_cap_evicts_nothing() {
        let entries = [entry("a", 100, 10, false), entry("b", 100, 20, false)];

        assert_eq!(plan_eviction(&entries, 200), super::EvictionPlan::default());
        assert_eq!(
            plan_eviction(&entries, 1_000),
            super::EvictionPlan::default()
        );
    }

    #[test]
    fn the_least_recently_used_entry_goes_first() {
        // The order is the whole point: the entry nobody has wanted since the beginning is the one to
        // lose, and the one used a moment ago is the one to keep.
        let entries = [
            entry("old", 100, 10, false),
            entry("middle", 100, 20, false),
            entry("new", 100, 30, false),
        ];

        let plan = plan_eviction(&entries, 250);

        assert_eq!(plan.keys, vec!["old"]);
        assert_eq!(plan.bytes, 100);
        assert!(!plan.cap_unreachable);
    }

    #[test]
    fn eviction_stops_as_soon_as_the_cache_fits() {
        let entries = [
            entry("oldest", 100, 10, false),
            entry("middle", 100, 20, false),
            entry("newest", 100, 30, false),
        ];

        let plan = plan_eviction(&entries, 100);

        assert_eq!(plan.keys, vec!["oldest", "middle"], "two, not three");
    }

    #[test]
    fn an_entry_in_use_is_never_evicted() {
        // The acceptance's second half. The pinned entry is the oldest in the cache and is left
        // alone, and the plan says so by evicting the unpinned ones instead.
        let entries = [
            entry("in-use", 500, 1, true),
            entry("idle", 100, 10, false),
            entry("idle-too", 100, 20, false),
        ];

        let plan = plan_eviction(&entries, 200);

        assert!(!plan.keys.contains(&"in-use".to_owned()));
        assert_eq!(plan.keys, vec!["idle", "idle-too"]);
    }

    #[test]
    fn a_cap_the_pinned_entries_alone_exceed_is_reported_rather_than_broken() {
        // Pinned entries are in use, and evicting one to reach a number would trade a guarantee for a
        // target. What the caller gets is the truth: this cap cannot be met while that image is up.
        let entries = [entry("in-use", 500, 1, true), entry("idle", 100, 10, false)];

        let plan = plan_eviction(&entries, 100);

        assert_eq!(plan.keys, vec!["idle"], "everything evictable went");
        assert!(plan.cap_unreachable, "and the cap is still not met");
    }

    #[test]
    fn a_cache_of_nothing_but_pinned_entries_evicts_nothing() {
        let entries = [entry("a", 500, 1, true), entry("b", 500, 2, true)];

        let plan = plan_eviction(&entries, 100);

        assert_eq!(plan.keys, Vec::<String>::new());
        assert_eq!(plan.bytes, 0);
        assert!(plan.cap_unreachable);
    }

    #[test]
    fn a_tie_on_last_use_is_broken_by_key() {
        // Determinism, which is what makes this testable at all: the same cache and cap always evict
        // the same set, whatever order the rows arrived in.
        let first = [entry("b", 100, 5, false), entry("a", 100, 5, false)];
        let second = [entry("a", 100, 5, false), entry("b", 100, 5, false)];

        assert_eq!(plan_eviction(&first, 100).keys, vec!["a"]);
        assert_eq!(
            plan_eviction(&first, 100).keys,
            plan_eviction(&second, 100).keys
        );
    }

    #[test]
    fn a_cap_of_zero_empties_the_unpinned_cache() {
        // "Cache nothing" is a setting somebody can choose, not a mistake: every unpinned entry goes
        // and the pinned ones are reported as unreachable.
        let entries = [entry("a", 100, 5, false), entry("b", 100, 6, true)];

        let plan = plan_eviction(&entries, 0);

        assert_eq!(plan.keys, vec!["a"]);
        assert!(plan.cap_unreachable);
    }

    #[test]
    fn an_empty_cache_with_a_cap_it_can_meet_does_nothing() {
        assert_eq!(plan_eviction(&[], 100), super::EvictionPlan::default());
        assert_eq!(plan_eviction(&[], 0), super::EvictionPlan::default());
    }

    #[test]
    fn a_negative_cap_is_unreachable_even_for_an_empty_cache() {
        // Written because the first version of this policy got it wrong and this test said the honest
        // thing instead of matching the code: nothing can weigh less than zero bytes, so a cap below
        // zero is unsatisfiable by definition, and reporting it is not noise — it is the caller being
        // told its configuration is nonsense.
        let plan = plan_eviction(&[], -1);

        assert_eq!(plan.keys, Vec::<String>::new());
        assert!(plan.cap_unreachable);
    }
}
