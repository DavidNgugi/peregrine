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
use std::num::NonZeroU32;

use jiff::Timestamp;
use peregrine_domain::id::Provider;
use peregrine_providers::{
    Capability, MetadataProvider, ProviderError, RAIL_PAGE_LIMIT, RailKind, SearchCandidate,
    SearchQuery,
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
/// candidates the search probe drew.
///
/// The suite observes only the port, and it takes a `dyn` reference on purpose: that the port
/// is object safe is itself part of the contract, because the registry holds one list of
/// providers of different types (PG-E5-12) and a signature that quietly stopped being
/// `dyn`-compatible would only be discovered there.
///
/// `rail` and `rail_page` are the rail probe: a caller names one list and one page, because the
/// suite cannot invent either — a rail's answer is a statement about a provider's catalogue, and
/// only the provider's own fixtures know what is in it.
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
///   fixtures;
/// - a candidate that carries artwork carries *something*: an empty or whitespace-only path is a
///   request for an image host's root dressed as a poster, and the cache key it would build names
///   no image. A provider that has no poster for an item says so with `None`.
///
/// And the same shape for `rail`, with the two differences a rail has: the bound is
/// [`RAIL_PAGE_LIMIT`] rather than a caller's limit, and a provider that *declares* `trending` and
/// then refuses one rail is conforming — a provider may have a trending list and no "on the air"
/// one, and the port names that case. What is *not* conforming is declaring the capability and
/// refusing with [`ProviderError::Unsupported`] attributing it to another provider, which is the
/// same misattribution the search refusal is checked for.
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
    rail: RailKind,
    rail_page: NonZeroU32,
) -> Vec<SearchCandidate> {
    let descriptor = provider.descriptor();
    let named = descriptor.provider();

    // A probe is not one of §13's capabilities, so every provider must be able to answer it — that is
    // what makes it usable for the key-validation flow (PG-E5-20). It may *fail*, and for a fixture
    // that is the point, but a failure attributed to another provider is the same misattribution the
    // search refusal below is checked for, and a user sent to the wrong settings pane.
    if let Err(failure) = provider.check().await {
        assert_eq!(
            failure.provider(),
            named,
            "a probe failure must name the provider it is about"
        );
    }

    let declared = descriptor.can(Capability::Search);
    let answer = provider.search(probe).await;

    // The rail probe runs whatever the search probe did: the two capabilities are independent, and
    // a provider that cannot search may still publish a rail.
    check_rail(provider, rail, rail_page).await;

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
        check_candidate(candidate, named, "search");
    }

    candidates
}

/// The facts about one candidate the suite can check without knowing a provider's fixtures.
///
/// These are identity rules rather than taste: the id must name the provider that answered,
/// artwork that is present must address something, a synopsis that is present must say something,
/// and a vote count must not be the zero a provider uses for "nobody has voted". `operation` names
/// the call that answered, so a failure says which endpoint produced it.
fn check_candidate(candidate: &SearchCandidate, named: Provider, operation: &str) {
    assert_eq!(
        candidate.external_id().provider(),
        named,
        "{named} answered a {operation} candidate identified by {}",
        candidate.external_id().provider()
    );
    if let Some(path) = candidate.artwork_path() {
        assert!(
            !path.trim().is_empty(),
            "{named} answered a {operation} candidate whose artwork path is blank, which names no image"
        );
    }
    if let Some(overview) = candidate.overview() {
        assert!(
            !overview.trim().is_empty(),
            "{named} answered a {operation} candidate whose synopsis is blank, which says nothing"
        );
    }
    assert_ne!(
        candidate.votes(),
        Some(0),
        "{named} answered a {operation} candidate with a vote count of zero, which is a provider's way of saying nobody has voted"
    );
}

/// The rail half of the contract, which is the search half's shape with a rail's two differences.
///
/// Written as a panic rather than a `Result` because that is what the search half does and because
/// a suite that returned its failures would be a suite a caller could ignore.
async fn check_rail(provider: &dyn MetadataProvider, rail: RailKind, page: NonZeroU32) {
    let descriptor = provider.descriptor();
    let named = descriptor.provider();

    if !descriptor.can(Capability::Trending) {
        match provider.rail(rail, page).await {
            Err(ProviderError::Unsupported {
                provider: reported,
                capability,
            }) => {
                assert_eq!(
                    reported, named,
                    "a rail refusal must name the provider that refused"
                );
                assert_eq!(
                    capability,
                    Capability::Trending.as_str(),
                    "a rail refusal must name the capability as it is declared"
                );
            }
            other => panic!(
                "{named} does not declare trending, so it must refuse a rail rather than {other:?}"
            ),
        }
        return;
    }

    match provider.rail(rail, page).await {
        Ok(candidates) => {
            assert!(
                candidates.len() <= RAIL_PAGE_LIMIT,
                "{named} answered {} candidates for a page, which holds at most {RAIL_PAGE_LIMIT}",
                candidates.len()
            );
            for candidate in &candidates {
                check_candidate(candidate, named, "rail");
            }
        }
        // A provider with a trending list and no list for this particular rail is conforming, and
        // the refusal has to be about this provider — the same misattribution rule as everywhere.
        Err(ProviderError::Unsupported {
            provider: reported, ..
        }) => assert_eq!(
            reported, named,
            "{named} refused a rail in another provider's name"
        ),
        Err(error) => panic!("{named} declares trending but answered a rail with {error}"),
    }
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
        // The fixture carries the two fields a list answer adds, because a rule the suite states
        // about them has to be satisfied by the fake the suite is demonstrated with.
        let fake = searching_fake(vec![
            candidate(Provider::Tmdb, "1396", "Breaking Bad")
                .with_artwork_path(Some("/poster.jpg".to_owned()))
                .with_rating(Some(9.2)),
        ]);
        let answered = block_on(metadata_provider_satisfies_the_port(
            &fake,
            &probe(),
            RailKind::PopularMovies,
            page(1),
        ));

        assert_eq!(answered.len(), 1);
    }

    /// One page, for the rail probe. `NonZeroU32` because that is what the port takes.
    fn page(number: u32) -> NonZeroU32 {
        NonZeroU32::new(number).expect("the test passes a non-zero page")
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

        let answered = block_on(metadata_provider_satisfies_the_port(
            &silent,
            &probe(),
            RailKind::PopularMovies,
            page(1),
        ));

        assert_eq!(answered, Vec::new());
    }

    #[test]
    fn the_suite_is_only_worth_running_if_a_broken_provider_fails_it() {
        // Four ways a provider can break the contract, each pinned against one that does.
        let liar = searching_fake(vec![candidate(Provider::Tvdb, "81189", "Breaking Bad")]);
        assert!(
            catch_async(metadata_provider_satisfies_the_port(
                &liar,
                &probe(),
                RailKind::PopularMovies,
                page(1)
            ))
            .is_err(),
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
                &probe(),
                RailKind::PopularMovies,
                page(1)
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
            catch_async(metadata_provider_satisfies_the_port(
                &failing,
                &probe(),
                RailKind::PopularMovies,
                page(1)
            ))
            .is_err(),
            "a provider that declares search and then fails must not conform"
        );

        let blank_artwork = searching_fake(vec![
            candidate(Provider::Tmdb, "603", "The Matrix")
                .with_artwork_path(Some("   ".to_owned())),
        ]);
        assert!(
            catch_async(metadata_provider_satisfies_the_port(
                &blank_artwork,
                &probe(),
                RailKind::PopularMovies,
                page(1)
            ))
            .is_err(),
            "artwork that is only whitespace names no image and must not conform"
        );
    }

    #[test]
    fn a_provider_that_declares_trending_must_answer_a_rail() {
        let publishing = FakeMetadataProvider::publishing_rails(
            Provider::Tmdb,
            vec![candidate(Provider::Tmdb, "603", "The Matrix")],
        );

        block_on(metadata_provider_satisfies_the_port(
            &publishing,
            &probe(),
            RailKind::PopularMovies,
            page(1),
        ));
    }

    #[test]
    fn a_provider_without_trending_satisfies_the_port_by_refusing_a_rail() {
        let silent = FakeMetadataProvider::searching(Provider::Tvdb, Vec::new());

        block_on(metadata_provider_satisfies_the_port(
            &silent,
            &probe(),
            RailKind::PopularMovies,
            page(1),
        ));
    }

    #[test]
    fn the_rail_half_of_the_suite_fails_a_provider_that_misattributes_or_over_answers() {
        // Two ways to break the rail contract: a candidate identified by another provider — the
        // coupling ADR 0007 rule 2 exists to prevent, which the fake can be handed and cannot
        // produce by accident — and a page larger than a page.
        let liar = FakeMetadataProvider::publishing_rails(
            Provider::Tmdb,
            vec![candidate(Provider::Tvdb, "81189", "Breaking Bad")],
        );
        assert!(
            catch_async(metadata_provider_satisfies_the_port(
                &liar,
                &probe(),
                RailKind::PopularMovies,
                page(1)
            ))
            .is_err(),
            "a rail answering with another provider's ids must not conform"
        );

        let over_answering = FakeMetadataProvider::publishing_rails(
            Provider::Tmdb,
            (0..=RAIL_PAGE_LIMIT)
                .map(|index| candidate(Provider::Tmdb, &index.to_string(), "Breaking Bad"))
                .collect(),
        );
        assert!(
            catch_async(metadata_provider_satisfies_the_port(
                &over_answering,
                &probe(),
                RailKind::PopularMovies,
                page(1)
            ))
            .is_err(),
            "a rail answering more than a page must not conform"
        );
    }

    #[test]
    fn a_rail_refusal_that_names_the_wrong_provider_fails_the_suite() {
        // The same rule the search refusal is held to, applied to the rail: an `Unsupported`
        // naming a provider nobody asked is a user sent to the wrong settings pane. The fixture
        // declares `trending` — so it must answer — and refuses in another provider's name.
        struct MisnamingRail {
            descriptor: peregrine_providers::ProviderDescriptor,
        }

        impl MetadataProvider for MisnamingRail {
            fn descriptor(&self) -> &peregrine_providers::ProviderDescriptor {
                &self.descriptor
            }

            fn check(&self) -> peregrine_providers::ProviderFuture<'_, ()> {
                Box::pin(async { Ok(()) })
            }

            fn image(
                &self,
                _path: &str,
                _kind: peregrine_domain::catalogue::ImageKind,
            ) -> peregrine_providers::ProviderFuture<'_, Vec<u8>> {
                Box::pin(async { Ok(Vec::new()) })
            }

            fn title(
                &self,
                _id: &peregrine_domain::id::ExternalId,
                _kind: peregrine_domain::catalogue::TitleKind,
            ) -> peregrine_providers::ProviderFuture<'_, peregrine_domain::catalogue::TitleDetails>
            {
                Box::pin(async {
                    Ok(peregrine_domain::catalogue::TitleDetails {
                        kind: peregrine_domain::TitleKind::Movie,
                        name: NonEmptyText::new("A title").expect("the fixture is not empty"),
                        original_name: None,
                        overview: None,
                        tagline: None,
                        released: None,
                        runtime_minutes: None,
                        original_language: None,
                        origin_country: None,
                        rating: None,
                        votes: None,
                        popularity: None,
                        adult: None,
                        in_production: None,
                        season_count: None,
                        episode_count: None,
                        homepage: None,
                        poster_path: None,
                        backdrop_path: None,
                    })
                })
            }

            fn search(
                &self,
                _query: &SearchQuery,
            ) -> peregrine_providers::ProviderFuture<'_, Vec<SearchCandidate>> {
                Box::pin(async { Ok(Vec::new()) })
            }

            fn rail(
                &self,
                _rail: RailKind,
                _page: NonZeroU32,
            ) -> peregrine_providers::ProviderFuture<'_, Vec<SearchCandidate>> {
                Box::pin(async {
                    Err(ProviderError::Unsupported {
                        provider: Provider::Tvdb,
                        capability: Capability::Trending.as_str(),
                    })
                })
            }
        }

        let wrong = MisnamingRail {
            descriptor: fake_descriptor(
                Provider::Tmdb,
                peregrine_providers::Capabilities::EMPTY
                    .with(Capability::Search)
                    .with(Capability::Trending),
            ),
        };

        assert!(
            catch_async(metadata_provider_satisfies_the_port(
                &wrong,
                &probe(),
                RailKind::PopularMovies,
                page(1)
            ))
            .is_err(),
            "a rail refusal naming another provider must not conform"
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

            fn check(&self) -> peregrine_providers::ProviderFuture<'_, ()> {
                // Healthy here on purpose: this fixture exists to misname a *search* refusal, and a
                // probe that failed first would make the suite error for a different reason than the
                // one this test is named for. The probe's own misattribution has its own fixture.
                Box::pin(async { Ok(()) })
            }

            fn image(
                &self,
                _path: &str,
                _kind: peregrine_domain::catalogue::ImageKind,
            ) -> peregrine_providers::ProviderFuture<'_, Vec<u8>> {
                Box::pin(async {
                    Err(ProviderError::Unsupported {
                        provider: Provider::Tmdb,
                        capability: "images",
                    })
                })
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

            fn rail(
                &self,
                _rail: RailKind,
                _page: NonZeroU32,
            ) -> peregrine_providers::ProviderFuture<'_, Vec<SearchCandidate>> {
                // Healthy on purpose, for the reason the probe is: this fixture misnames a *search*
                // refusal, and a rail refined for a different reason would mask it.
                Box::pin(async { Ok(Vec::new()) })
            }
        }

        let wrong = Misnaming {
            descriptor: fake_descriptor(
                Provider::Tvdb,
                peregrine_providers::Capabilities::EMPTY.with(Capability::Trending),
            ),
        };

        assert!(
            catch_async(metadata_provider_satisfies_the_port(
                &wrong,
                &probe(),
                RailKind::PopularMovies,
                page(1)
            ))
            .is_err()
        );
    }

    #[test]
    fn a_probe_failure_that_names_the_wrong_provider_fails_the_suite() {
        // The probe's half of the same rule. A refused credential attributed to a provider nobody
        // asked would put the right error in front of the wrong settings row, which is worse than the
        // generic failure PG-E5-20 exists to remove.
        struct Misprobe {
            descriptor: peregrine_providers::ProviderDescriptor,
        }

        impl MetadataProvider for Misprobe {
            fn descriptor(&self) -> &peregrine_providers::ProviderDescriptor {
                &self.descriptor
            }

            fn check(&self) -> peregrine_providers::ProviderFuture<'_, ()> {
                Box::pin(async {
                    Err(ProviderError::Unauthorized {
                        provider: Provider::Tmdb,
                    })
                })
            }

            fn image(
                &self,
                _path: &str,
                _kind: peregrine_domain::catalogue::ImageKind,
            ) -> peregrine_providers::ProviderFuture<'_, Vec<u8>> {
                Box::pin(async { Ok(Vec::new()) })
            }

            fn title(
                &self,
                _id: &peregrine_domain::id::ExternalId,
                _kind: peregrine_domain::catalogue::TitleKind,
            ) -> peregrine_providers::ProviderFuture<'_, peregrine_domain::catalogue::TitleDetails>
            {
                Box::pin(async {
                    Err(ProviderError::Unsupported {
                        provider: Provider::Tvdb,
                        capability: "title",
                    })
                })
            }

            fn search(
                &self,
                _query: &SearchQuery,
            ) -> peregrine_providers::ProviderFuture<'_, Vec<SearchCandidate>> {
                Box::pin(async { Ok(Vec::new()) })
            }

            fn rail(
                &self,
                _rail: RailKind,
                _page: NonZeroU32,
            ) -> peregrine_providers::ProviderFuture<'_, Vec<SearchCandidate>> {
                // Declares `trending` in its descriptor below, so this must answer rather than
                // refuse: the fixture exists to misname a *probe* failure, and a rail refusal here
                // would panic for a different rule than the one this test is named for.
                Box::pin(async { Ok(Vec::new()) })
            }
        }

        let wrong = Misprobe {
            descriptor: fake_descriptor(
                Provider::Tvdb,
                peregrine_providers::Capabilities::EMPTY.with(Capability::Search),
            ),
        };

        assert!(
            catch_async(metadata_provider_satisfies_the_port(
                &wrong,
                &probe(),
                RailKind::PopularMovies,
                page(1)
            ))
            .is_err()
        );
    }
}
