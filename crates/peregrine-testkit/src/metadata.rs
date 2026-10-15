//! A scripted metadata provider.

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::time::Duration;

use peregrine_domain::catalogue::{
    EpisodeDetails, SeasonDetails, SeasonNumber, TitleDetails, TitleKind,
};
use peregrine_domain::id::ExternalId;
use peregrine_domain::id::Provider;
use peregrine_providers::{
    BrowseRequest, CachePolicy, Capabilities, Capability, GenreRef, MetadataProvider,
    ProviderDescriptor, ProviderError, ProviderFuture, ProviderPage, RailKind, RatePolicy,
    SearchCandidate, SearchQuery,
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
///
/// **A rail is one answer for every rail.** A fake that answered per [`RailKind`] would be
/// modelling a provider with seven lists it does not have, and a test that needs two rails to
/// differ builds two fakes: one is the provider's answer and the other is the second list.
#[derive(Debug)]
pub struct FakeMetadataProvider {
    descriptor: ProviderDescriptor,
    answer: Result<Vec<SearchCandidate>, ProviderError>,
    title_answer: Result<TitleDetails, ProviderError>,
    image_answer: Result<Vec<u8>, ProviderError>,
    /// What every rail answers. Defaults to `Ok(Vec::new())` in every constructor below, which is
    /// the honest default for a fixture whose caller is testing a search: a fake built for one
    /// operation is not also claiming a provider's trending list is empty.
    rail_answer: Result<Vec<SearchCandidate>, ProviderError>,
    /// What every browse page answers. Defaults to an empty page in every constructor below, for
    /// the reason `rail_answer` does: a fake built for a search is not also claiming a catalogue
    /// with nothing in it.
    browse_answer: Result<ProviderPage, ProviderError>,
    /// What the genre list answers. Empty by default, for the same reason.
    genres_answer: Result<Vec<GenreRef>, ProviderError>,
    /// What a read of the titles like one title answers. Empty by default, for the same reason.
    similar_answer: Result<Vec<SearchCandidate>, ProviderError>,
    /// What the season list answers. Empty by default, for the same reason.
    seasons_answer: Result<Vec<SeasonDetails>, ProviderError>,
    /// What each season's episode list answers, keyed by the number asked for. A season nobody
    /// scripted answers an empty list, and one scripted as a failure answers that failure — which is
    /// how "one season of several 404s" is written down, since the fake holds answers rather than a
    /// sequence.
    episodes_answer: BTreeMap<u32, Result<Vec<EpisodeDetails>, ProviderError>>,
    /// What a probe answers. Defaults to `Ok(())` in every constructor below: a fake built to test a
    /// search is not also claiming its credential is bad, and a test that wants the refusal asks for it
    /// by name.
    check_answer: Result<(), ProviderError>,
}

impl FakeMetadataProvider {
    /// A provider that answers `answer` to every search, whatever is asked.
    #[must_use]
    pub const fn new(
        descriptor: ProviderDescriptor,
        answer: Result<Vec<SearchCandidate>, ProviderError>,
    ) -> Self {
        let provider = descriptor.provider();
        Self {
            descriptor,
            answer,
            // A fake built by the constructors above declares only `search`, so reading a title is a
            // refusal rather than a panic: an adapter that answered a capability it never declared is
            // exactly what the conformance suite exists to catch, and a fake must model that.
            title_answer: Err(ProviderError::Unsupported {
                provider,
                capability: "title",
            }),
            image_answer: Err(ProviderError::Unsupported {
                provider,
                capability: "images",
            }),
            rail_answer: Ok(Vec::new()),
            browse_answer: Ok(ProviderPage::empty()),
            genres_answer: Ok(Vec::new()),
            similar_answer: Ok(Vec::new()),
            // Empty by default, for the reason `rail_answer` is: a fake built for one operation is
            // not also claiming a provider with no seasons.
            seasons_answer: Ok(Vec::new()),
            episodes_answer: BTreeMap::new(),
            check_answer: Ok(()),
        }
    }

    /// A provider that declares `search` and `title`, answering `details` to every title read.
    #[must_use]
    pub fn describing(provider: Provider, details: TitleDetails) -> Self {
        Self {
            descriptor: fake_descriptor(
                provider,
                Capabilities::EMPTY
                    .with(Capability::Search)
                    .with(Capability::Title),
            ),
            answer: Ok(Vec::new()),
            title_answer: Ok(details),
            image_answer: Err(ProviderError::Unsupported {
                provider,
                capability: "images",
            }),
            rail_answer: Ok(Vec::new()),
            browse_answer: Ok(ProviderPage::empty()),
            genres_answer: Ok(Vec::new()),
            similar_answer: Ok(Vec::new()),
            // Empty by default, for the reason `rail_answer` is: a fake built for one operation is
            // not also claiming a provider with no seasons.
            seasons_answer: Ok(Vec::new()),
            episodes_answer: BTreeMap::new(),
            check_answer: Ok(()),
        }
    }

    /// A provider that declares `images` and answers `bytes` to every image read.
    #[must_use]
    pub fn serving_images(provider: Provider, details: TitleDetails, bytes: Vec<u8>) -> Self {
        Self {
            descriptor: fake_descriptor(
                provider,
                Capabilities::EMPTY
                    .with(Capability::Search)
                    .with(Capability::Title)
                    .with(Capability::Images),
            ),
            answer: Ok(Vec::new()),
            title_answer: Ok(details),
            image_answer: Ok(bytes),
            rail_answer: Ok(Vec::new()),
            browse_answer: Ok(ProviderPage::empty()),
            genres_answer: Ok(Vec::new()),
            similar_answer: Ok(Vec::new()),
            // Empty by default, for the reason `rail_answer` is: a fake built for one operation is
            // not also claiming a provider with no seasons.
            seasons_answer: Ok(Vec::new()),
            episodes_answer: BTreeMap::new(),
            check_answer: Ok(()),
        }
    }

    /// A provider whose stored credential is refused, for the key-validation flow (PG-E5-20).
    #[must_use]
    pub fn refusing_credentials(provider: Provider) -> Self {
        Self {
            descriptor: fake_descriptor(provider, Capabilities::EMPTY.with(Capability::Search)),
            answer: Ok(Vec::new()),
            title_answer: Err(ProviderError::Unsupported {
                provider,
                capability: "title",
            }),
            image_answer: Err(ProviderError::Unsupported {
                provider,
                capability: "images",
            }),
            rail_answer: Ok(Vec::new()),
            browse_answer: Ok(ProviderPage::empty()),
            genres_answer: Ok(Vec::new()),
            similar_answer: Ok(Vec::new()),
            seasons_answer: Ok(Vec::new()),
            episodes_answer: BTreeMap::new(),
            check_answer: Err(ProviderError::Unauthorized { provider }),
        }
    }

    /// A provider that declares `trending` and answers `candidates` to every rail.
    ///
    /// It declares `search` and `title` as well, because the port's conformance suite walks a
    /// provider through every operation it has, and a fake that could only be asked for a rail
    /// would be a fake that cannot stand in for the provider a screen actually talks to.
    #[must_use]
    pub fn publishing_rails(provider: Provider, candidates: Vec<SearchCandidate>) -> Self {
        let mut fake = Self::new(
            fake_descriptor(
                provider,
                Capabilities::EMPTY
                    .with(Capability::Search)
                    .with(Capability::Trending),
            ),
            Ok(Vec::new()),
        );
        fake.rail_answer = Ok(candidates);
        fake
    }

    /// The same fake, answering `candidates` to every rail.
    ///
    /// A builder rather than another constructor, and the descriptor is *not* rewritten: a fake
    /// built by [`FakeMetadataProvider::serving_images`] declares `search`, `title`, and `images`,
    /// and this method answers a rail the descriptor does not declare. That is deliberate — the
    /// conformance suite is the thing that decides whether a provider may do that, and a builder
    /// that silently widened the capability set would make the fake agree with itself instead of
    /// with the port. Use [`FakeMetadataProvider::publishing_rails`] for a provider that *declares*
    /// trending.
    #[must_use]
    pub fn with_rail_answer(mut self, candidates: Vec<SearchCandidate>) -> Self {
        self.rail_answer = Ok(candidates);
        self
    }

    /// The same fake, answering `page` to every browse request.
    ///
    /// A builder rather than another constructor, for the reason
    /// [`FakeMetadataProvider::with_rail_answer`] gives: the descriptor is *not* rewritten, so a
    /// fake whose descriptor does not declare `trending` answers a browse the port says it may not
    /// serve. That is deliberate — the conformance suite decides whether a provider may do that,
    /// and a builder that silently widened the capability set would make the fake agree with itself.
    #[must_use]
    pub fn with_browse_answer(mut self, page: ProviderPage) -> Self {
        self.browse_answer = Ok(page);
        self
    }

    /// The same fake, answering `genres` to every genre read.
    #[must_use]
    pub fn with_genres_answer(mut self, genres: Vec<GenreRef>) -> Self {
        self.genres_answer = Ok(genres);
        self
    }

    /// The same fake, answering `candidates` to every read of what is like one title.
    #[must_use]
    pub fn with_similar_answer(mut self, candidates: Vec<SearchCandidate>) -> Self {
        self.similar_answer = Ok(candidates);
        self
    }

    /// A provider that declares `search`, `title`, and `season`, answering `details` to every title
    /// read and `seasons` to every season list.
    ///
    /// The season capability is *declared* here rather than left to a builder, because the port's rule
    /// is that a provider either answers a capability or refuses it, and the refusal is a different
    /// test: [`FakeMetadataProvider::describing`] is the provider that cannot be asked about a season.
    /// The episodes of each season are scripted separately through
    /// [`FakeMetadataProvider::with_episodes`], so a fake built here and left alone is a series whose
    /// seasons are published and whose episodes nobody has read.
    #[must_use]
    pub fn describing_episodes(
        provider: Provider,
        details: TitleDetails,
        seasons: Vec<SeasonDetails>,
    ) -> Self {
        Self {
            descriptor: fake_descriptor(
                provider,
                Capabilities::EMPTY
                    .with(Capability::Search)
                    .with(Capability::Title)
                    .with(Capability::Season),
            ),
            answer: Ok(Vec::new()),
            title_answer: Ok(details),
            image_answer: Err(ProviderError::Unsupported {
                provider,
                capability: "images",
            }),
            rail_answer: Ok(Vec::new()),
            browse_answer: Ok(ProviderPage::empty()),
            genres_answer: Ok(Vec::new()),
            similar_answer: Ok(Vec::new()),
            seasons_answer: Ok(seasons),
            episodes_answer: BTreeMap::new(),
            check_answer: Ok(()),
        }
    }

    /// The same fake, answering `episodes` for the season numbered `season_number`.
    ///
    /// One season at a time, because that is the shape of the question: a title's seasons are separate
    /// requests, and a test that needs one of them to fail while the others are written needs to say
    /// *which* one.
    #[must_use]
    pub fn with_episodes(mut self, season_number: u32, episodes: Vec<EpisodeDetails>) -> Self {
        self.episodes_answer.insert(season_number, Ok(episodes));
        self
    }

    /// The same fake, failing the season numbered `season_number` with `error`.
    ///
    /// The provider-having-a-bad-day case at one season, which the title page's refresh must not turn
    /// into a failed title: a series whose answer is missing one season still has the others.
    #[must_use]
    pub fn with_season_failure(mut self, season_number: u32, error: ProviderError) -> Self {
        self.episodes_answer.insert(season_number, Err(error));
        self
    }

    /// A provider that declares `search`, `trending`, `title`, and `images`, and answers each with
    /// what the caller scripted.
    ///
    /// One constructor for the four answers together because they are one scenario: the Discover
    /// rails job lists titles *and* fills their artwork, so a fixture for it needs a provider that
    /// can do both, and composing four separate builders would be four chances to build a provider
    /// the job cannot actually be driven with.
    #[must_use]
    pub fn publishing_rails_and_serving_images(
        provider: Provider,
        details: TitleDetails,
        image: Vec<u8>,
        rails: Vec<SearchCandidate>,
    ) -> Self {
        Self {
            descriptor: fake_descriptor(
                provider,
                Capabilities::EMPTY
                    .with(Capability::Search)
                    .with(Capability::Title)
                    .with(Capability::Images)
                    .with(Capability::Trending),
            ),
            answer: Ok(Vec::new()),
            title_answer: Ok(details),
            image_answer: Ok(image),
            rail_answer: Ok(rails),
            browse_answer: Ok(ProviderPage::empty()),
            genres_answer: Ok(Vec::new()),
            similar_answer: Ok(Vec::new()),
            // Empty by default, for the reason `rail_answer` is: a fake built for one operation is
            // not also claiming a provider with no seasons.
            seasons_answer: Ok(Vec::new()),
            episodes_answer: BTreeMap::new(),
            check_answer: Ok(()),
        }
    }

    /// The same fake, failing every rail with `error`.
    ///
    /// Distinct from [`FakeMetadataProvider::failing`], which refuses `search`: this one declares
    /// `trending` in its descriptor — so the port says it must answer — and then fails the call,
    /// which is the provider-having-a-bad-day case the rails job reports.
    #[must_use]
    pub fn failing_rails(mut self, error: ProviderError) -> Self {
        self.rail_answer = Err(error);
        self
    }

    /// Why this fake refuses a season read, or `None` when it answers one.
    ///
    /// Two refusals and both are the port's: a film has no seasons, and a provider that has not
    /// declared `season` has said it cannot serve one. A fake that answered anyway would be a fake no
    /// adapter can be, which is exactly what the conformance suite exists to catch.
    fn season_refusal(&self, kind: TitleKind) -> Option<ProviderError> {
        if kind == TitleKind::Movie || !self.descriptor.can(Capability::Season) {
            return Some(ProviderError::Unsupported {
                provider: self.descriptor.provider(),
                capability: Capability::Season.as_str(),
            });
        }
        None
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

    fn check(&self) -> ProviderFuture<'_, ()> {
        let answer = self.check_answer.clone();
        Box::pin(async move { answer })
    }

    fn image(
        &self,
        _path: &str,
        _kind: peregrine_domain::catalogue::ImageKind,
    ) -> ProviderFuture<'_, Vec<u8>> {
        let answer = self.image_answer.clone();
        Box::pin(async move { answer })
    }

    fn title(&self, _id: &ExternalId, _kind: TitleKind) -> ProviderFuture<'_, TitleDetails> {
        let answer = self.title_answer.clone();
        Box::pin(async move { answer })
    }

    fn seasons(&self, _id: &ExternalId, kind: TitleKind) -> ProviderFuture<'_, Vec<SeasonDetails>> {
        if let Some(refusal) = self.season_refusal(kind) {
            return Box::pin(async move { Err(refusal) });
        }
        let answer = self.seasons_answer.clone();
        Box::pin(async move { answer })
    }

    fn season_episodes(
        &self,
        _id: &ExternalId,
        kind: TitleKind,
        season_number: SeasonNumber,
    ) -> ProviderFuture<'_, Vec<EpisodeDetails>> {
        if let Some(refusal) = self.season_refusal(kind) {
            return Box::pin(async move { Err(refusal) });
        }
        // A season nobody scripted answers an empty list rather than a refusal: the list itself was
        // served, and a season the provider published with no episodes read is a real state.
        let answer = self
            .episodes_answer
            .get(&season_number.get())
            .cloned()
            .unwrap_or(Ok(Vec::new()));
        Box::pin(async move { answer })
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

    fn rail(&self, _rail: RailKind, _page: NonZeroU32) -> ProviderFuture<'_, Vec<SearchCandidate>> {
        // The same refusal `search` makes, for the same reason: a capability the descriptor does
        // not declare is one the provider has said it cannot serve, and answering anyway is what
        // the conformance suite exists to catch.
        if !self.descriptor.can(Capability::Trending) {
            let refusal = ProviderError::Unsupported {
                provider: self.descriptor.provider(),
                capability: Capability::Trending.as_str(),
            };
            return Box::pin(async move { Err(refusal) });
        }
        let answer = self.rail_answer.clone();
        Box::pin(async move { answer })
    }

    fn browse(&self, _request: &BrowseRequest) -> ProviderFuture<'_, ProviderPage> {
        // The same refusal `rail` makes, for the same reason: a capability the descriptor does not
        // declare is one the provider has said it cannot serve. `browse` is gated on `trending`,
        // exactly as the port documents.
        if !self.descriptor.can(Capability::Trending) {
            let refusal = ProviderError::Unsupported {
                provider: self.descriptor.provider(),
                capability: Capability::Trending.as_str(),
            };
            return Box::pin(async move { Err(refusal) });
        }
        let answer = self.browse_answer.clone();
        Box::pin(async move { answer })
    }

    fn genres(&self, _kind: TitleKind) -> ProviderFuture<'_, Vec<GenreRef>> {
        if !self.descriptor.can(Capability::Trending) {
            let refusal = ProviderError::Unsupported {
                provider: self.descriptor.provider(),
                capability: Capability::Trending.as_str(),
            };
            return Box::pin(async move { Err(refusal) });
        }
        let answer = self.genres_answer.clone();
        Box::pin(async move { answer })
    }

    fn similar(
        &self,
        _id: &ExternalId,
        _kind: TitleKind,
    ) -> ProviderFuture<'_, Vec<SearchCandidate>> {
        // Gated on `trending` beside `browse` and `genres`, exactly as the port documents: a
        // provider with no queried list has none to answer a "like this" question with.
        if !self.descriptor.can(Capability::Trending) {
            let refusal = ProviderError::Unsupported {
                provider: self.descriptor.provider(),
                capability: Capability::Trending.as_str(),
            };
            return Box::pin(async move { Err(refusal) });
        }
        let answer = self.similar_answer.clone();
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
        .with_artwork_path(Some("/poster.jpg".to_owned()))
        .with_backdrop_path(Some("/backdrop.jpg".to_owned()))
        .with_rating(Some(9.5))
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
        assert_eq!(
            answered[0].artwork_path(),
            Some("/poster.jpg"),
            "a fake that was handed artwork answers with it"
        );
        assert_eq!(
            answered[0].backdrop_path(),
            Some("/backdrop.jpg"),
            "and the wide image the same answer carried"
        );
        assert_eq!(answered[0].rating(), Some(9.5));
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

    #[test]
    fn a_fake_that_was_not_told_about_a_rail_answers_an_empty_one() {
        // The default every constructor uses except `publishing_rails`: a fake built to test a
        // search is not also claiming a provider's trending list is empty, so the rail answer is
        // empty and the descriptor is what decides whether it is served at all.
        let scripted = FakeMetadataProvider::searching(Provider::Tmdb, Vec::new());

        assert_eq!(
            block_on(scripted.rail(RailKind::PopularMovies, page())),
            Err(ProviderError::Unsupported {
                provider: Provider::Tmdb,
                capability: "trending",
            }),
            "a fake that declares only search must refuse a rail"
        );

        let publishing = FakeMetadataProvider::publishing_rails(Provider::Tmdb, Vec::new());

        assert_eq!(
            block_on(publishing.rail(RailKind::PopularMovies, page())),
            Ok(Vec::new())
        );
    }

    /// One page for the rail reading above.
    fn page() -> NonZeroU32 {
        NonZeroU32::new(1).expect("one is not zero")
    }
}
