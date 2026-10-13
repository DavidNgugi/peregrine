//! The conformance suite: what any `Clock` and any `IdGenerator` must satisfy.
//!
//! A fake is only worth sharing if it and the adapter it stands in for answer to the same
//! rules, so the rules are written here once and run twice. The fake runs them in this
//! crate's tests; `peregrine-platform` runs them against `SystemClock` and `UlidGenerator`
//! in its own, which is what `Cargo.toml`'s dev-dependency on this crate is for. The edge
//! is one-way — this crate knows `peregrine-domain`, never an adapter — so there is no
//! cycle and `peregrine-platform`'s shipping build is unchanged.
//!
//! The suite observes only the port. It reads no wall clock, takes no lock, and sleeps
//! nowhere, because a suite that needed real time to pass could not be run against a
//! frozen fake — and the fake is the only implementation whose instant a test can pin.

use std::collections::BTreeSet;

use jiff::Timestamp;
use peregrine_providers::{
    Capability, MetadataProvider, ProviderError, SearchCandidate, SearchQuery,
};
use ulid::Ulid;

/// Identifiers the suite draws from an [`IdGenerator`](peregrine_domain::IdGenerator) before it is satisfied.
///
/// Four would catch a generator that returns a constant; this many also catches one that
/// cycles through a short list, and 64 ULIDs are nothing to hold.
const CONFORMING_DRAWS: usize = 64;

/// A set of the identifiers a run drew, which is how the suite reports what it saw.
type Set<T> = BTreeSet<T>;

/// The project's first commit, in milliseconds. A clock reporting seconds, microseconds,
/// or a different epoch lands below it.
const PROJECT_FLOOR_MILLISECONDS: i64 = 1_600_000_000_000;

/// Asserts that `now` satisfies the [`Clock`](peregrine_domain::Clock) contract, and
/// returns the first instant read.
///
/// Every read must land after the project's first commit and no read may go backwards,
/// which is the property a journal's ordering rests on. The suite is silent about whether
/// two reads *differ*: a coarse clock may legitimately answer twice from the same tick, so
/// requiring real time to have passed would make the suite flaky for the one
/// implementation that reads a real clock.
///
/// # Panics
///
/// Panics with the offending instant when the contract is broken.
pub fn clock_satisfies_the_port(now: impl Fn() -> Timestamp) -> Timestamp {
    let first = now();
    assert!(
        first.as_millisecond() > PROJECT_FLOOR_MILLISECONDS,
        "{first} is before the project existed, which a clock returning the wrong unit \
         would look like"
    );
    let second = now();
    assert!(
        second >= first,
        "a later read went backwards: {first} then {second}"
    );
    let third = now();
    assert!(
        third >= second,
        "a later read went backwards: {second} then {third}"
    );
    first
}

/// Asserts that `next_id` satisfies the [`IdGenerator`](peregrine_domain::IdGenerator)
/// contract.
///
/// Successive draws must differ: the scheduler asks for an identifier per submission, and
/// a repeat collides in its dedupe index. Ordering is deliberately *not* asserted — a
/// ULID's leading bits are its millisecond, so two of them minted in the same millisecond
/// order arbitrarily, and a suite demanding a strict order would be asserting something
/// the production generator does not promise.
///
/// # Panics
///
/// Panics naming the repeated identifier when the contract is broken.
pub fn id_generator_satisfies_the_port(next_id: impl Fn() -> Ulid) -> Set<Ulid> {
    let drawn: Vec<Ulid> = (0..CONFORMING_DRAWS).map(|_| next_id()).collect();
    let unique: Set<Ulid> = drawn.iter().copied().collect();
    assert_eq!(
        unique.len(),
        drawn.len(),
        "a repeated identifier would collide in the dedupe index"
    );
    unique
}

/// Asserts that `provider` satisfies the [`MetadataProvider`] contract, and returns the
/// candidates the probe drew.
///
/// The suite observes only the port, and it takes a `dyn` reference on purpose: that the port
/// is object safe is itself part of the contract, because the registry holds one list of
/// providers of different types (PG-E5-12) and a signature that quietly stopped being
/// `dyn`-compatible would only be discovered there.
///
/// What it asserts, for a provider that either declares `search` or does not:
///
/// - if it declares `search`, the probe is answered with `Ok` — a provider that claims the
///   capability and then fails the call is a claim the registry would have repeated to a user;
/// - if it does not, the call is refused with [`ProviderError::Unsupported`] naming this
///   provider and the capability, rather than falling through to a request;
/// - a refusal is the *only* thing an undeclared capability produces, so a provider cannot
///   spend a rate-limit token finding out what it already said about itself;
/// - no more candidates come back than the query asked for, because the number travels to the
///   UI and the IPC payload budget is a contract rather than a hope (AGENTS R11);
/// - every candidate carries **this** provider's identifier. A provider returning another
///   provider's ids is the coupling ADR 0007 rule 2 exists to prevent — the reference
///   implementation's TVDB adapter called TMDB internally and married the two forever — and
///   it is the one fact about a candidate the suite can check without knowing the provider's
///   fixtures.
///
/// It deliberately says nothing about *which* candidates come back: that is what the
/// provider's recorded fixtures are for (PG-E5-17), and a suite that guessed at them would be
/// asserting its own guess.
///
/// # Panics
///
/// Panics naming what the provider did when any of the above is broken.
pub async fn metadata_provider_satisfies_the_port(
    provider: &dyn MetadataProvider,
    probe: &SearchQuery,
) -> Vec<SearchCandidate> {
    let descriptor = provider.descriptor();
    let named = descriptor.provider();
    let declared = descriptor.can(Capability::Search);
    let answer = provider.search(probe).await;

    if !declared {
        match answer {
            Err(ProviderError::Unsupported {
                provider: reported,
                capability,
            }) => {
                assert_eq!(
                    reported, named,
                    "a refusal must name the provider that refused"
                );
                assert_eq!(
                    capability,
                    Capability::Search.as_str(),
                    "a refusal must name the capability as it is declared"
                );
            }
            other => panic!(
                "{named} does not declare search, so it must refuse it rather than {other:?}"
            ),
        }
        return Vec::new();
    }

    let candidates = match answer {
        Ok(candidates) => candidates,
        Err(error) => panic!("{named} declares search but answered {error}"),
    };

    assert!(
        candidates.len() <= probe.limit().get() as usize,
        "{named} answered {} candidates for a limit of {}",
        candidates.len(),
        probe.limit()
    );

    for candidate in &candidates {
        assert_eq!(
            candidate.external_id().provider(),
            named,
            "{named} answered a candidate identified by {}",
            candidate.external_id().provider()
        );
    }

    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    use futures::executor::block_on;
    use jiff::civil::Date;
    use peregrine_domain::id::{ExternalId, Provider};
    use peregrine_domain::text::NonEmptyText;
    use peregrine_domain::{Clock, IdGenerator, TitleKind};
    use std::num::NonZeroU32;

    use crate::{CountingIds, FakeClock, FakeMetadataProvider, fake_descriptor};

    /// A candidate identified by `provider`, which is what the suite's identity rule is about.
    fn candidate(provider: Provider, id: &str, name: &str) -> SearchCandidate {
        SearchCandidate::new(
            ExternalId::new(provider, id).expect("the test passes a valid id"),
            NonEmptyText::new(name).expect("the test passes real text"),
            TitleKind::Show,
            Some(Date::constant(2008, 1, 20)),
        )
    }

    /// The fake must pass the same suite the production adapter passes. If this fails, the
    /// fake is not a stand-in for the port and every test built on it is measuring
    /// something else.
    #[test]
    fn the_fake_clock_satisfies_the_port() {
        let clock = FakeClock::default();
        let observed = clock_satisfies_the_port(|| clock.now());
        assert_eq!(observed.as_millisecond(), clock.now_millis());
    }

    /// Determinism is what makes the suite itself runnable against the fake: the pinned
    /// instant survives the whole suite, so nothing here needed a real clock.
    #[test]
    fn the_fake_clock_leaves_time_where_it_was() {
        let clock = FakeClock::at(1_700_000_000_123);
        let observed = clock_satisfies_the_port(|| clock.now());
        assert_eq!(observed.as_millisecond(), 1_700_000_000_123);
        assert_eq!(clock.now_millis(), 1_700_000_000_123);
    }

    #[test]
    fn the_fake_ids_satisfy_the_port() {
        let generator = CountingIds::default();
        let drawn = id_generator_satisfies_the_port(|| generator.next_id());
        assert_eq!(drawn.len(), CONFORMING_DRAWS);
        assert_eq!(generator.issued(), CONFORMING_DRAWS);
    }

    /// The suite is only worth running if a broken implementation fails it, so both halves
    /// are pinned against one that is broken on purpose.
    #[test]
    fn a_repeating_id_generator_fails_the_suite() {
        let outcome = std::panic::catch_unwind(|| {
            id_generator_satisfies_the_port(|| Ulid::from_parts(1_700_000_000_000, 0));
        });
        assert!(outcome.is_err(), "a constant generator must not conform");
    }

    #[test]
    fn a_clock_repeating_the_same_instant_conforms() {
        // A coarse clock is not a broken clock: repeating an instant is permitted, and the
        // suite must not be the thing that forbids it.
        let outcome = std::panic::catch_unwind(|| {
            clock_satisfies_the_port(|| Timestamp::from_millisecond(1_700_000_000_000).unwrap())
        });
        assert!(outcome.is_ok(), "a coarse clock is still a clock");
    }

    /// A provider that declares `search` and answers the probe.
    fn searching_fake(candidates: Vec<SearchCandidate>) -> FakeMetadataProvider {
        FakeMetadataProvider::searching(Provider::Tmdb, candidates)
    }

    fn probe() -> SearchQuery {
        SearchQuery::new(
            NonEmptyText::new("breaking bad").expect("real text"),
            NonZeroU32::new(10).expect("ten is not zero"),
            None,
        )
    }

    fn catch_async<F: std::future::Future>(
        future: F,
    ) -> Result<F::Output, Box<dyn std::any::Any + Send>> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| block_on(future)))
    }

    #[test]
    fn the_fake_provider_satisfies_the_port() {
        // The fake must pass the same suite every adapter passes. If this fails, the fake is
        // not a stand-in for the port and every test built on it is measuring something else.
        let fake = searching_fake(vec![candidate(Provider::Tmdb, "1396", "Breaking Bad")]);
        let answered = block_on(metadata_provider_satisfies_the_port(&fake, &probe()));

        assert_eq!(answered.len(), 1);
    }

    #[test]
    fn a_provider_without_the_capability_satisfies_the_port_by_refusing_it() {
        // The other branch of the same contract: a provider that does not declare search must
        // refuse it, and that refusal is what the suite accepts in place of an answer.
        let silent = FakeMetadataProvider::new(
            fake_descriptor(
                Provider::Tvdb,
                peregrine_providers::Capabilities::EMPTY.with(Capability::Trending),
            ),
            Ok(vec![candidate(Provider::Tvdb, "81189", "Breaking Bad")]),
        );

        let answered = block_on(metadata_provider_satisfies_the_port(&silent, &probe()));

        assert_eq!(answered, Vec::new());
    }

    #[test]
    fn the_suite_is_only_worth_running_if_a_broken_provider_fails_it() {
        // Three ways a provider can break the contract, each pinned against one that does.
        let liar = searching_fake(vec![candidate(Provider::Tvdb, "81189", "Breaking Bad")]);
        assert!(
            catch_async(metadata_provider_satisfies_the_port(&liar, &probe())).is_err(),
            "a provider answering with another provider's ids must not conform"
        );

        let over_answering = searching_fake(
            (0..11)
                .map(|index| candidate(Provider::Tmdb, &index.to_string(), "Breaking Bad"))
                .collect(),
        );
        assert!(
            catch_async(metadata_provider_satisfies_the_port(
                &over_answering,
                &probe()
            ))
            .is_err(),
            "a provider ignoring the limit must not conform"
        );

        let failing = FakeMetadataProvider::failing(
            Provider::Tmdb,
            ProviderError::Transport {
                provider: Provider::Tmdb,
                kind: peregrine_providers::TransportFailure::Timeout,
            },
        );
        assert!(
            catch_async(metadata_provider_satisfies_the_port(&failing, &probe())).is_err(),
            "a provider that declares search and then fails must not conform"
        );
    }

    #[test]
    fn a_refusal_that_names_the_wrong_provider_fails_the_suite() {
        // The refusal is checked, not merely accepted: an `Unsupported` naming a provider
        // that was not asked would send a user to the wrong settings pane. The fake cannot
        // produce this — it refuses with its own name, which is the behaviour the suite
        // requires — so the misnaming is written out here.
        struct Misnaming {
            descriptor: peregrine_providers::ProviderDescriptor,
        }

        impl MetadataProvider for Misnaming {
            fn descriptor(&self) -> &peregrine_providers::ProviderDescriptor {
                &self.descriptor
            }

            fn title(
                &self,
                _id: &peregrine_domain::id::ExternalId,
                _kind: peregrine_domain::catalogue::TitleKind,
            ) -> peregrine_providers::ProviderFuture<'_, peregrine_domain::catalogue::TitleDetails>
            {
                Box::pin(async {
                    Err(ProviderError::Unsupported {
                        provider: Provider::Tmdb,
                        capability: "title",
                    })
                })
            }

            fn search(
                &self,
                _query: &SearchQuery,
            ) -> peregrine_providers::ProviderFuture<'_, Vec<SearchCandidate>> {
                Box::pin(async {
                    Err(ProviderError::Unsupported {
                        provider: Provider::Tmdb,
                        capability: Capability::Search.as_str(),
                    })
                })
            }
        }

        let wrong = Misnaming {
            descriptor: fake_descriptor(
                Provider::Tvdb,
                peregrine_providers::Capabilities::EMPTY.with(Capability::Trending),
            ),
        };

        assert!(catch_async(metadata_provider_satisfies_the_port(&wrong, &probe())).is_err());
    }
}
