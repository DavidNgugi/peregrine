//! A scripted metadata provider.

use std::num::NonZeroU32;
use std::time::Duration;

use peregrine_domain::id::Provider;
use peregrine_providers::{
    CachePolicy, Capabilities, Capability, MetadataProvider, ProviderDescriptor, ProviderError,
    ProviderFuture, RatePolicy, SearchCandidate, SearchQuery,
};

/// A descriptor that is well formed for the capabilities it is given.
///
/// Every test that needs a provider needs one of these first, and the numbers in it are
/// deliberately unremarkable: they are a fixture's, not a claim about what any real provider
/// allows. The researched figures live beside the adapter that answers to them, with the ones
/// that are unverified marked as such in `docs/research/data-sources.md`.
///
/// # Panics
///
/// Panics when the capabilities are empty, because a provider that can be asked nothing is
/// refused by the port itself — a test that wants that case wants
/// [`ProviderDescriptor::new`] and the error it returns, not this.
#[must_use]
pub fn fake_descriptor(provider: Provider, capabilities: Capabilities) -> ProviderDescriptor {
    let rate = RatePolicy::new(
        NonZeroU32::new(5).expect("five is not zero"),
        Duration::from_secs(1),
        NonZeroU32::new(1).expect("one is not zero"),
        false,
    )
    .expect("a one-second window is not empty");
    let cache = CachePolicy::new(Duration::from_secs(60), None, false)
        .expect("a policy with no ceiling cannot contradict one");

    ProviderDescriptor::new(provider, "Fake provider", capabilities, rate, cache, None)
        .expect("the caller passes at least one capability")
}

/// A metadata provider that answers exactly what a test told it to.
///
/// It holds an answer rather than a script: the operations the port has are pure lookups as
/// far as a test is concerned, and a fake that replayed a *sequence* would be modelling a
/// state machine no provider has. When a test needs one call to differ from the next it is
/// describing a scenario — a 429 and then a success — and the place for that is a fixture in
/// the adapter's own crate (PG-E5-17), not here.
#[derive(Debug)]
pub struct FakeMetadataProvider {
    descriptor: ProviderDescriptor,
    answer: Result<Vec<SearchCandidate>, ProviderError>,
}

impl FakeMetadataProvider {
    /// A provider that answers `answer` to every search, whatever is asked.
    #[must_use]
    pub const fn new(
        descriptor: ProviderDescriptor,
        answer: Result<Vec<SearchCandidate>, ProviderError>,
    ) -> Self {
        Self { descriptor, answer }
    }

    /// A provider that declares `search` and returns `candidates`.
    #[must_use]
    pub fn searching(provider: Provider, candidates: Vec<SearchCandidate>) -> Self {
        Self::new(
            fake_descriptor(provider, Capabilities::EMPTY.with(Capability::Search)),
            Ok(candidates),
        )
    }

    /// A provider that declares `search` and fails with `error`.
    #[must_use]
    pub fn failing(provider: Provider, error: ProviderError) -> Self {
        Self::new(
            fake_descriptor(provider, Capabilities::EMPTY.with(Capability::Search)),
            Err(error),
        )
    }
}

impl MetadataProvider for FakeMetadataProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn search(&self, _query: &SearchQuery) -> ProviderFuture<'_, Vec<SearchCandidate>> {
        // The refusal comes first, and the fake makes it for the same reason an adapter must:
        // a capability the descriptor does not declare is one the provider has said it cannot
        // serve, and answering anyway is what the conformance suite exists to catch. `search`
        // is the only operation the port has, so a fake declaring anything else can only
        // demonstrate this path.
        if !self.descriptor.can(Capability::Search) {
            let refusal = ProviderError::Unsupported {
                provider: self.descriptor.provider(),
                capability: Capability::Search.as_str(),
            };
            return Box::pin(async move { Err(refusal) });
        }

        // The script is cloned per call rather than borrowed, because a real adapter's future
        // owns its response and a fake whose future borrowed from itself would let a test
        // pass that the real thing cannot reproduce.
        let answer = self.answer.clone();
        Box::pin(async move { answer })
    }
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use jiff::civil::Date;
    use peregrine_domain::TitleKind;
    use peregrine_domain::id::ExternalId;
    use peregrine_domain::text::NonEmptyText;

    use super::*;

    pub(crate) fn candidate(provider: Provider, id: &str, name: &str) -> SearchCandidate {
        SearchCandidate::new(
            ExternalId::new(provider, id).expect("the test passes a valid id"),
            NonEmptyText::new(name).expect("the test passes real text"),
            TitleKind::Show,
            Some(Date::constant(2008, 1, 20)),
        )
    }

    fn query() -> SearchQuery {
        SearchQuery::new(
            NonEmptyText::new("breaking bad").expect("real text"),
            NonZeroU32::new(10).expect("ten is not zero"),
            None,
        )
    }

    #[test]
    fn the_fake_answers_with_what_it_was_given() {
        let fake = FakeMetadataProvider::searching(
            Provider::Tmdb,
            vec![candidate(Provider::Tmdb, "1396", "Breaking Bad")],
        );
        let answered = block_on(fake.search(&query())).expect("the fake was told to succeed");

        assert_eq!(answered.len(), 1);
        assert_eq!(answered[0].name().as_str(), "Breaking Bad");
    }

    #[test]
    fn the_fake_answers_the_same_way_every_time() {
        // A fake that answered once and then stopped would make a test's second call a
        // different test.
        let fake = FakeMetadataProvider::searching(
            Provider::Tmdb,
            vec![candidate(Provider::Tmdb, "1396", "Breaking Bad")],
        );

        let first = block_on(fake.search(&query())).expect("succeeds");
        let second = block_on(fake.search(&query())).expect("succeeds");

        assert_eq!(first, second);
    }

    #[test]
    fn a_failing_fake_fails_with_the_error_it_was_given() {
        let fake = FakeMetadataProvider::failing(
            Provider::Tmdb,
            ProviderError::Unauthorized {
                provider: Provider::Tmdb,
            },
        );

        assert_eq!(
            block_on(fake.search(&query())),
            Err(ProviderError::Unauthorized {
                provider: Provider::Tmdb
            })
        );
    }

    #[test]
    fn the_fake_describes_itself_as_a_provider_that_can_search() {
        let fake = FakeMetadataProvider::searching(Provider::Tvdb, Vec::new());

        assert!(fake.descriptor().can(Capability::Search));
        assert!(!fake.descriptor().can(Capability::Videos));
        assert_eq!(fake.descriptor().provider(), Provider::Tvdb);
    }

    #[test]
    fn the_fake_refuses_a_capability_it_does_not_declare() {
        // The behaviour the conformance suite requires of every adapter, demonstrated by the
        // fake: a provider that has said it cannot search must refuse rather than answer, and
        // must name itself and the capability when it does.
        let silent = FakeMetadataProvider::new(
            fake_descriptor(
                Provider::Tvdb,
                Capabilities::EMPTY.with(Capability::Trending),
            ),
            Ok(vec![candidate(Provider::Tvdb, "81189", "Breaking Bad")]),
        );

        assert_eq!(
            block_on(silent.search(&query())),
            Err(ProviderError::Unsupported {
                provider: Provider::Tvdb,
                capability: "search",
            })
        );
    }
}
