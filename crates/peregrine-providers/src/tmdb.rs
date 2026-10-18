//! The TMDB adapter: what TMDB says, translated into this application's own vocabulary.
//!
//! **Nothing TMDB-shaped leaves this module** (ADR 0007 rule 2). The JSON types below are private,
//! they deserialize a *subset* of what the public API documents, and every field that could be absent
//! is optional here rather than assumed — an answer is untrusted input like any other (`PG-E5-05`).
//!
//! The operations this adapter serves are `search`, `title`, `seasons`, `season_episodes`, `rail`,
//! `browse`, `genres`, `similar`, `image`, and `check`. The rest of `PG-E5-06` — configuration and the
//! full discover filter set — and `PG-E5-07`/`PG-E5-08`'s credits, videos, episode groups, and content
//! ratings are what the port will grow to ask for.
//!
//! **The fixtures these tests are written against are synthetic.** The response shape follows TMDB's
//! public documentation; no request has been made to the live service from this repository, because
//! `AGENTS R14` forbids it and a test that needed a key would be a test nobody could run. When the
//! first live call happens (`PG-E5-20`'s "test connection"), the shapes here are what to check.

use std::time::Duration;

use jiff::civil::Date;
use peregrine_domain::catalogue::{
    EpisodeDetails, EpisodeNumber, SeasonDetails, SeasonNumber, TitleDetails, TitleKind,
};
use peregrine_domain::{ExternalId, NonEmptyText, Provider};
use reqwest::Url;
use serde::Deserialize;

use crate::capability::{Capabilities, Capability};
use crate::descriptor::{Attribution, CachePolicy, ProviderDescriptor, RatePolicy};
use crate::error::ProviderError;
use crate::http::{HttpClient, HttpPolicy, Request};
use crate::hygiene::{Body, BodyCap};
use crate::provider::{
    BrowseRequest, BrowseSort, CastMember, GenreRef, MetadataProvider, ProviderFuture,
    ProviderPage, RailKind, SearchCandidate, SearchQuery, TitleCredits,
};

/// The largest search answer this adapter will read.
///
/// TMDB's search pages are small — twenty results of ids and names — so a megabyte is generous for a
/// legitimate answer and small enough that a wrong or hostile one is refused before it is parsed.
pub const SEARCH_BODY_CAP: BodyCap = BodyCap::new(std::num::NonZeroU64::new(1_048_576).unwrap());

/// How many votes a title must have to appear when the caller sorts by score.
///
/// **The adapter's floor, not the caller's, and it is here because TMDB's score order is unusable
/// without one.** `/discover` orders by the raw average, so a page sorted by `vote_average.desc`
/// with no floor is a wall of `10.0` from items a handful of people voted on; a floor is what makes
/// "best reviewed" mean the best of what an audience actually reviewed. It is applied to that one
/// order and no other, because every other order is meaningful without it. The number is a
/// deliberate choice rather than a documented requirement — TMDB's own guidance is to pair the
/// order with *some* `vote_count.gte` — and it is a constant so the choice is visible and testable
/// rather than buried in a URL.
pub const RATING_VOTE_FLOOR: u32 = 200;

/// The last page TMDB will serve, for any paged endpoint.
///
/// **A documented ceiling and not a guess.** TMDB's own staff state that a query returning more than
/// 20,000 items is served only to page 500 — "Pages start at 1 and max at 500" is the error body for
/// 501 — while `total_pages` keeps reporting the true total in the tens of thousands. This adapter
/// therefore stops claiming another page at 500 rather than asking for one it knows will be refused.
///
/// The consequence for a reader is real and worth stating where the number is: an unfiltered
/// discover list is reachable 20,000 titles deep, and reaching further needs the query narrowed —
/// by genre, by year, by score — which is what the browse screen's filters are for.
pub const LAST_PAGE: u32 = 500;

/// The attribution TMDB's terms require (`PG-E5-19` renders it).
///
/// Public because the notice has to be readable by a layer that has no adapter: a settings screen
/// lists what this build is obliged to say about the providers it *offers*, whether or not one is
/// configured. The descriptor is built from this same constant, so there is one string and not two.
pub const ATTRIBUTION_NOTICE: &str =
    "This product uses the TMDB API but is not endorsed or certified by TMDB.";

/// The TMDB adapter.
///
/// Holds the descriptor, the client, the base URL, and the credential. It is built by the registry
/// from a descriptor and a key the keyring provided (`PG-E2`), which is why the credential is a
/// parameter rather than something this type goes looking for: an adapter that fetched its own secret
/// would be an adapter that could not be tested without one.
pub struct Tmdb {
    descriptor: ProviderDescriptor,
    client: HttpClient,
    base: String,
    image_base: String,
    credential: String,
}

impl Tmdb {
    /// The production base URL.
    pub const BASE_URL: &'static str = "https://api.themoviedb.org/3";

    /// Builds an adapter against `base` — the live service, or a test server.
    ///
    /// `base` is a parameter rather than a constant so the adapter can be pointed at a `wiremock`
    /// server, which is the only way its behaviour can be tested at all (`AGENTS R14`).
    ///
    /// # Errors
    ///
    /// [`ProviderError`] when the descriptor cannot be built or the HTTP client cannot be constructed,
    /// which is a failure of the policy rather than of anything a provider said.
    pub fn new(base: &str, image_base: &str, credential: String) -> Result<Self, ProviderError> {
        Self::with_capabilities(
            base,
            image_base,
            credential,
            Capabilities::EMPTY
                .with(Capability::Search)
                .with(Capability::Title)
                .with(Capability::Season)
                .with(Capability::Images)
                .with(Capability::Trending),
        )
    }

    /// Builds an adapter declaring exactly `capabilities`.
    ///
    /// The capability set is a parameter rather than a literal inside [`Tmdb::new`] for one reason:
    /// the refusal path — a provider that has said it cannot serve a capability must refuse rather
    /// than answer — is else unreachable in this adapter, and a rule with no test is a rule nobody
    /// has run. Nothing in the shipping path calls this with a set other than the full one.
    fn with_capabilities(
        base: &str,
        image_base: &str,
        credential: String,
        capabilities: Capabilities,
    ) -> Result<Self, ProviderError> {
        let descriptor = ProviderDescriptor::new(
            Provider::Tmdb,
            "TMDB",
            capabilities,
            // TMDB's documented limit is fifty requests a second; this is far below it, because the
            // cost of being wrong in this direction is a slower sync and in the other is a ban.
            RatePolicy::new(nonzero(20), Duration::from_secs(1), nonzero(10), true).map_err(
                |_| ProviderError::Unsupported {
                    provider: Provider::Tmdb,
                    capability: "a rate policy",
                },
            )?,
            CachePolicy::new(Duration::from_hours(24), None, true).map_err(|_| {
                ProviderError::Unsupported {
                    provider: Provider::Tmdb,
                    capability: "a cache policy",
                }
            })?,
            Some(Attribution::new(ATTRIBUTION_NOTICE, true).map_err(|_| {
                ProviderError::Unsupported {
                    provider: Provider::Tmdb,
                    capability: "an attribution",
                }
            })?),
        )
        .map_err(|_| ProviderError::Unsupported {
            provider: Provider::Tmdb,
            capability: "a descriptor",
        })?;

        let policy = HttpPolicy {
            timeout: Duration::from_secs(15),
            connect_timeout: Duration::from_secs(5),
            user_agent: "Peregrine/0.1 (+https://github.com/peregrine-media/peregrine)".to_owned(),
            backoff: crate::http::Backoff::default(),
        };

        Ok(Self {
            descriptor,
            client: HttpClient::new(Provider::Tmdb, &policy)?,
            base: base.trim_end_matches('/').to_owned(),
            // A parameter for the same reason the API base is: an adapter that could only talk to the
            // live CDN could not be tested at all (`AGENTS R14`).
            image_base: image_base.trim_end_matches('/').to_owned(),
            credential,
        })
    }

    /// The URL of one documented path on this adapter's base.
    ///
    /// One helper rather than four copies of the same `map_err`, because every endpoint this
    /// adapter reads is the base plus a path and a failed `Url::parse` means the same thing each
    /// time: the base is not a URL, which is a failure of the policy rather than of anything a
    /// provider said.
    fn url(&self, path: &str) -> Result<Url, ProviderError> {
        Url::parse(&format!("{}/{path}", self.base)).map_err(|_| ProviderError::Transport {
            provider: Provider::Tmdb,
            kind: crate::error::TransportFailure::Connect,
        })
    }

    /// The URL one search goes to.
    ///
    /// A `Url` rather than a formatted string, because the query is the user's own text and a title
    /// with an `&` or a space in it must be encoded rather than glued into a path. `include_adult` is
    /// false: a provider-side filter is a courtesy to a household's search box, and the *scope* is what
    /// decides what an account may see (`AGENTS R34`) — the two are different questions and neither
    /// substitutes for the other.
    fn search_url(&self, query: &SearchQuery) -> Result<Url, ProviderError> {
        let endpoint = match query.kind() {
            Some(TitleKind::Movie) => "search/movie",
            Some(TitleKind::Show) => "search/tv",
            None => "search/multi",
        };
        let mut url = self.url(endpoint)?;
        url.query_pairs_mut()
            .append_pair("query", query.text().as_str())
            .append_pair("include_adult", "false")
            .append_pair("page", "1");
        Ok(url)
    }

    /// The URL one rail page goes to.
    ///
    /// The mapping from [`RailKind`] to a path is the whole of what this adapter adds to the port's
    /// vocabulary, and it is deliberately total: a rail this build knows and this endpoint does not
    /// serve would be a silent empty list, so the `match` has no catch-all and a new rail fails the
    /// build here rather than at a user's screen.
    ///
    /// `page` is the caller's, and it is sent as-is: a provider's paging is one-based, which is why
    /// the port takes a [`NonZeroU32`](std::num::NonZeroU32) rather than an offset.
    ///
    /// `/trending/all/week` is the endpoint for [`RailKind::TrendingWeek`] rather than
    /// `/trending/movie/week` or `/trending/tv/week`, because a rail that named one kind would be a
    /// rail that lies about the other half of what it shows — the answer carries each item's own
    /// `media_type`, and the candidate mapping already reads it.
    fn rail_url(&self, rail: RailKind, page: std::num::NonZeroU32) -> Result<Url, ProviderError> {
        let endpoint = match rail {
            RailKind::TrendingWeek => "trending/all/week",
            RailKind::PopularMovies => "movie/popular",
            RailKind::TopRatedMovies => "movie/top_rated",
            RailKind::UpcomingMovies => "movie/upcoming",
            RailKind::PopularShows => "tv/popular",
            RailKind::OnTheAirShows => "tv/on_the_air",
            RailKind::TopRatedShows => "tv/top_rated",
        };
        let mut url = self.url(endpoint)?;
        url.query_pairs_mut()
            .append_pair("page", &page.get().to_string());
        Ok(url)
    }

    /// The URL one browse page goes to.
    ///
    /// `/discover/movie` and `/discover/tv` rather than one of the named lists [`Tmdb::rail_url`]
    /// reads, because here the filters *are* the endpoint's parameters: the kind is the path and
    /// everything else is a query pair. The order mapping is total in both the order and the kind
    /// because TMDB names the date field differently for the two — `primary_release_date` for a
    /// film and `first_air_date` for a series — so a `match` with no catch-all makes a new order
    /// fail the build here rather than send a parameter TMDB ignores and answer the wrong list.
    ///
    /// **Sorting by score carries a vote floor** ([`RATING_VOTE_FLOOR`]), and that is the adapter's
    /// decision rather than a caller's: see that constant for why. Zero is never sent as a floor,
    /// because it is the value TMDB gives an item nobody has voted on rather than a floor at all,
    /// and `vote_average.gte=0` would state a filter the caller did not ask for.
    fn browse_url(&self, request: &BrowseRequest) -> Result<Url, ProviderError> {
        let kind = request.kind();
        let endpoint = match kind {
            TitleKind::Movie => "discover/movie",
            TitleKind::Show => "discover/tv",
        };
        let mut url = self.url(endpoint)?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs
                .append_pair("page", &request.page().get().to_string())
                .append_pair("sort_by", sort_parameter(request.sort(), kind))
                .append_pair("include_adult", "false");
            if let Some(genre) = request.genre() {
                pairs.append_pair("with_genres", &genre.to_string());
            }
            if let Some(cast) = request.cast() {
                pairs.append_pair("with_cast", cast);
            }
            if let Some(floor) = request.min_rating().filter(|floor| *floor > 0) {
                pairs.append_pair("vote_average.gte", &floor.to_string());
            }
            if request.sort() == BrowseSort::Rating {
                pairs.append_pair("vote_count.gte", &RATING_VOTE_FLOOR.to_string());
            }
        }
        Ok(url)
    }

    /// The URL the genre list for `kind` goes to.
    ///
    /// Two endpoints rather than one because TMDB keeps two vocabularies, and the ids do not agree
    /// between them — which is why [`MetadataProvider::genres`] takes a kind at all.
    fn genres_url(&self, kind: TitleKind) -> Result<Url, ProviderError> {
        let endpoint = match kind {
            TitleKind::Movie => "genre/movie/list",
            TitleKind::Show => "genre/tv/list",
        };
        self.url(endpoint)
    }

    /// The URL one "similar titles" read goes to.
    ///
    /// `/movie/{id}/similar` and `/tv/{id}/similar`: one path per kind for the reason
    /// [`MetadataProvider::title`] gives, because TMDB's ids are one namespace of integers and a
    /// film's id sent to the series path would answer another title's recommendations or nothing.
    ///
    /// No `page` is sent, so the provider's own default — its first page — is what arrives. The rail
    /// that draws this holds one strip, and `PG-E7-15` owns the version that pages.
    /// The URL one list of "titles rated alongside this one" goes to.
    ///
    /// **`/recommendations` and not `/similar`, and that is TMDB's own advice rather than a taste.**
    /// Their staff describe `/similar` as "just an aggregation around genres and keywords" over
    /// user-contributed data, say it "does not tend to yield very good results", and urge
    /// `/recommendations` instead — "based off of user ratings which is better in most situations
    /// (although still not perfect, especially for new releases)". The parenthetical is why
    /// [`Tmdb::similar`] still keeps `/similar` as a fallback.
    fn recommendations_url(&self, id: &ExternalId, kind: TitleKind) -> Result<Url, ProviderError> {
        let endpoint = match kind {
            TitleKind::Movie => "movie",
            TitleKind::Show => "tv",
        };
        self.url(&format!("{endpoint}/{}/recommendations", id.value()))
    }

    /// The URL `/similar` lives at, which is this adapter's *fallback* and no longer its answer.
    fn similar_url(&self, id: &ExternalId, kind: TitleKind) -> Result<Url, ProviderError> {
        let endpoint = match kind {
            TitleKind::Movie => "movie",
            TitleKind::Show => "tv",
        };
        self.url(&format!("{endpoint}/{}/similar", id.value()))
    }

    /// The URL one film's own record goes to, read for its collection and nothing else.
    ///
    /// `/movie/{id}` rather than a cheaper endpoint because TMDB exposes a film's franchise only on
    /// its details, and `belongs_to_collection` is the one signal that says "same series" exactly.
    /// The URL a series' season list is read from.
    ///
    /// `/tv/{id}`, and deliberately not a seasons path of its own: TMDB publishes a series' season
    /// list inside the series object, which is the same answer `title` reads. The two mapping different
    /// subsets of one payload is why filling a series' seasons costs one extra request rather than two,
    /// and why the list here carries only the fields a season summary has.
    fn seasons_url(&self, id: &ExternalId) -> Result<Url, ProviderError> {
        self.url(&format!("tv/{}", id.value()))
    }

    /// The URL one season's episodes are read from.
    ///
    /// The season number is a path segment rather than a query parameter, which is what makes a 404
    /// mean "no such season" rather than "no such series" — the distinction the sync reads as that one
    /// season's problem rather than the title's.
    fn season_url(
        &self,
        id: &ExternalId,
        season_number: SeasonNumber,
    ) -> Result<Url, ProviderError> {
        self.url(&format!("tv/{}/season/{}", id.value(), season_number.get()))
    }

    /// Why this adapter refuses a season read, or `None` when it serves one.
    ///
    /// Two refusals, and the film one is the reason this is a function rather than a capability check:
    /// a film has no seasons at all, so an empty list would be the adapter claiming a fact about a
    /// catalogue instead of refusing a question it cannot be asked.
    fn season_refusal(&self, kind: TitleKind) -> Option<ProviderError> {
        if kind == TitleKind::Movie || !self.descriptor.capabilities().contains(Capability::Season)
        {
            return Some(ProviderError::Unsupported {
                provider: Provider::Tmdb,
                capability: Capability::Season.as_str(),
            });
        }
        None
    }

    fn movie_url(&self, id: &ExternalId) -> Result<Url, ProviderError> {
        self.url(&format!("movie/{}", id.value()))
    }

    /// The URL one collection's parts go to.
    fn collection_url(&self, collection: u32) -> Result<Url, ProviderError> {
        self.url(&format!("collection/{collection}"))
    }

    /// One paged list answer, mapped to candidates of `kind`.
    ///
    /// A discover page, a recommendation list, and a `/similar` list are the same object with the same
    /// mapping — one kind per endpoint and no `media_type` in the answer, so the kind the path promised
    /// is the kind of every result — which is why they share one reader rather than three.
    async fn list(
        &self,
        url: &str,
        kind: TitleKind,
    ) -> Result<Vec<SearchCandidate>, ProviderError> {
        let fetched = self
            .client
            .fetch(
                Provider::Tmdb,
                Request {
                    url,
                    credential: Some(&self.credential),
                },
            )
            .await?;

        let answer: BrowseAnswer = Body::new(Provider::Tmdb, &fetched.bytes, fetched.media_type())
            .decode(SEARCH_BODY_CAP)?;

        Ok(answer
            .results
            .into_iter()
            .filter_map(|result| result.into_candidate_for(Some(kind)))
            .collect())
    }

    /// A film's collection parts, or nothing when it is in no collection.
    ///
    /// **Best-effort, and deliberately so.** This is an enhancement to a rail whose subject — the
    /// title — has already been read successfully: a details or collection request that fails leaves
    /// the rail as the provider's own list, where failing the whole read would replace a page the
    /// reader asked for with an error about a franchise they did not.
    async fn collection_parts(&self, movie_url: &str) -> Vec<SearchCandidate> {
        let Some(collection) = self.collection_of(movie_url).await else {
            return Vec::new();
        };
        let Ok(url) = self.collection_url(collection) else {
            return Vec::new();
        };
        let Ok(fetched) = self
            .client
            .fetch(
                Provider::Tmdb,
                Request {
                    url: url.as_str(),
                    credential: Some(&self.credential),
                },
            )
            .await
        else {
            return Vec::new();
        };
        let Ok(answer) = Body::new(Provider::Tmdb, &fetched.bytes, fetched.media_type())
            .decode::<CollectionPartsAnswer>(SEARCH_BODY_CAP)
        else {
            return Vec::new();
        };
        answer
            .parts
            .into_iter()
            .filter_map(|result| result.into_candidate_for(Some(TitleKind::Movie)))
            .collect()
    }

    /// Which collection a film belongs to, if any.
    async fn collection_of(&self, movie_url: &str) -> Option<u32> {
        let fetched = self
            .client
            .fetch(
                Provider::Tmdb,
                Request {
                    url: movie_url,
                    credential: Some(&self.credential),
                },
            )
            .await
            .ok()?;
        let answer: DetailsAnswer = Body::new(Provider::Tmdb, &fetched.bytes, fetched.media_type())
            .decode(SEARCH_BODY_CAP)
            .ok()?;
        answer.belongs_to_collection?.id
    }
}

/// The `sort_by` value for one order and kind.
///
/// Total in both, so a new [`BrowseSort`] is a build failure here rather than a parameter TMDB
/// silently ignores.
fn sort_parameter(sort: BrowseSort, kind: TitleKind) -> &'static str {
    match (sort, kind) {
        (BrowseSort::Popularity, _) => "popularity.desc",
        (BrowseSort::Rating, _) => "vote_average.desc",
        (BrowseSort::Newest, TitleKind::Movie) => "primary_release_date.desc",
        (BrowseSort::Newest, TitleKind::Show) => "first_air_date.desc",
        (BrowseSort::Oldest, TitleKind::Movie) => "primary_release_date.asc",
        (BrowseSort::Oldest, TitleKind::Show) => "first_air_date.asc",
    }
}

impl MetadataProvider for Tmdb {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn check(&self) -> ProviderFuture<'_, ()> {
        // `/configuration` rather than a search: it is the smallest answer TMDB has, it is the same
        // endpoint whatever the credential's scopes are, and it costs almost nothing against the rate
        // limit. The body is deliberately never decoded — see the port's own doc — so a configuration
        // shape this build has never read cannot fail a working key.
        let url = format!("{}/configuration", self.base);

        Box::pin(async move {
            self.client
                .fetch(
                    Provider::Tmdb,
                    Request {
                        url: &url,
                        credential: Some(&self.credential),
                    },
                )
                .await?;
            Ok(())
        })
    }

    fn search(&self, query: &SearchQuery) -> ProviderFuture<'_, Vec<SearchCandidate>> {
        // Everything borrowed is read *before* the async block and moved in as owned data, so the
        // returned future borrows only `self` — which is the lifetime the port's elided signature
        // gives it. A block that captured `query` would demand both live as long, and an impl may not
        // be narrower than its trait.
        if !self.descriptor.capabilities().contains(Capability::Search) {
            // The port's own rule: a provider either answers or refuses, and it never spends a request
            // finding out which (`crate::provider`).
            let unsupported = ProviderError::Unsupported {
                provider: Provider::Tmdb,
                capability: "search",
            };
            return Box::pin(async move { Err(unsupported) });
        }
        let limit = query.limit().get() as usize;
        let url = match self.search_url(query) {
            Ok(url) => url.to_string(),
            Err(error) => return Box::pin(async move { Err(error) }),
        };

        Box::pin(async move {
            let fetched = self
                .client
                .fetch(
                    Provider::Tmdb,
                    Request {
                        url: &url,
                        credential: Some(&self.credential),
                    },
                )
                .await?;

            let answer: SearchAnswer =
                Body::new(Provider::Tmdb, &fetched.bytes, fetched.media_type())
                    .decode(SEARCH_BODY_CAP)?;

            let mut candidates = Vec::new();
            for result in answer.results.into_iter().take(limit) {
                if let Some(candidate) = result.into_candidate() {
                    candidates.push(candidate);
                }
            }
            Ok(candidates)
        })
    }

    fn rail(
        &self,
        rail: RailKind,
        page: std::num::NonZeroU32,
    ) -> ProviderFuture<'_, Vec<SearchCandidate>> {
        // The refusal comes first, before a URL is built or a token spent: a provider either
        // answers or refuses, and it never spends a request finding out which (`crate::provider`).
        if !self
            .descriptor
            .capabilities()
            .contains(Capability::Trending)
        {
            let unsupported = ProviderError::Unsupported {
                provider: Provider::Tmdb,
                capability: "trending",
            };
            return Box::pin(async move { Err(unsupported) });
        }
        // This adapter serves every rail this build names, and the mapping is total, so a rail it
        // cannot serve cannot be constructed. The page, however, is a backend's own ceiling, and this
        // adapter asks for page one only: TMDB serves 500 pages and refuses 501 — a moderator states
        // the cap and the refusal is `status_code: 22` — while `total_pages` keeps reporting the true
        // total, which is why [`LAST_PAGE`] exists and why nothing here pages a rail at all.
        let url = match self.rail_url(rail, page) {
            Ok(url) => url.to_string(),
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let fixed = rail.kind();

        Box::pin(async move {
            let fetched = self
                .client
                .fetch(
                    Provider::Tmdb,
                    Request {
                        url: &url,
                        credential: Some(&self.credential),
                    },
                )
                .await?;

            let answer: SearchAnswer =
                Body::new(Provider::Tmdb, &fetched.bytes, fetched.media_type())
                    .decode(SEARCH_BODY_CAP)?;

            // The answer's bound is the port's page: `RAIL_PAGE_LIMIT` and not a `.take(limit)`,
            // because a rail has no caller-supplied limit to respect.
            Ok(answer
                .results
                .into_iter()
                .filter_map(|result| result.into_candidate_for(fixed))
                .take(crate::provider::RAIL_PAGE_LIMIT)
                .collect())
        })
    }

    fn browse(&self, request: &BrowseRequest) -> ProviderFuture<'_, ProviderPage> {
        // The refusal comes first, before a URL is built or a token spent: a provider either
        // answers or refuses, and it never spends a request finding out which (`crate::provider`).
        if !self
            .descriptor
            .capabilities()
            .contains(Capability::Trending)
        {
            let unsupported = ProviderError::Unsupported {
                provider: Provider::Tmdb,
                capability: "trending",
            };
            return Box::pin(async move { Err(unsupported) });
        }
        let url = match self.browse_url(request) {
            Ok(url) => url.to_string(),
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let fixed = request.kind();

        Box::pin(async move {
            let fetched = self
                .client
                .fetch(
                    Provider::Tmdb,
                    Request {
                        url: &url,
                        credential: Some(&self.credential),
                    },
                )
                .await?;

            let answer: BrowseAnswer =
                Body::new(Provider::Tmdb, &fetched.bytes, fetched.media_type())
                    .decode(SEARCH_BODY_CAP)?;

            // A discover endpoint answers one kind and sends no `media_type`, so the kind the path
            // promised is the kind every result is — the rule `rail` states for its own lists.
            let has_more = answer.has_more();
            let items = answer
                .results
                .into_iter()
                .filter_map(|result| result.into_candidate_for(Some(fixed)))
                .take(crate::provider::RAIL_PAGE_LIMIT)
                .collect();
            Ok(ProviderPage::new(items, has_more))
        })
    }

    fn genres(&self, kind: TitleKind) -> ProviderFuture<'_, Vec<GenreRef>> {
        if !self
            .descriptor
            .capabilities()
            .contains(Capability::Trending)
        {
            let unsupported = ProviderError::Unsupported {
                provider: Provider::Tmdb,
                capability: "trending",
            };
            return Box::pin(async move { Err(unsupported) });
        }
        let url = match self.genres_url(kind) {
            Ok(url) => url.to_string(),
            Err(error) => return Box::pin(async move { Err(error) }),
        };

        Box::pin(async move {
            let fetched = self
                .client
                .fetch(
                    Provider::Tmdb,
                    Request {
                        url: &url,
                        credential: Some(&self.credential),
                    },
                )
                .await?;

            let answer: GenresAnswer =
                Body::new(Provider::Tmdb, &fetched.bytes, fetched.media_type())
                    .decode(SEARCH_BODY_CAP)?;

            Ok(answer
                .genres
                .into_iter()
                .filter_map(GenreEntry::into_genre)
                .collect())
        })
    }

    fn similar(
        &self,
        id: &ExternalId,
        kind: TitleKind,
    ) -> ProviderFuture<'_, Vec<SearchCandidate>> {
        if !self
            .descriptor
            .capabilities()
            .contains(Capability::Trending)
        {
            let unsupported = ProviderError::Unsupported {
                provider: Provider::Tmdb,
                capability: "trending",
            };
            return Box::pin(async move { Err(unsupported) });
        }
        // Read the URLs before the future is built, for the reason `search` gives: the block must
        // borrow only `self`, and every failure that can be reported without a request is reported
        // without one.
        let recommendations = match self.recommendations_url(id, kind) {
            Ok(url) => url.to_string(),
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let fallback = match self.similar_url(id, kind) {
            Ok(url) => url.to_string(),
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        // **Only a film can be in a collection**, so a series pays no request for one: TMDB models
        // franchises as film collections and has no series equivalent.
        let movie = match kind {
            TitleKind::Movie => match self.movie_url(id) {
                Ok(url) => Some(url.to_string()),
                Err(error) => return Box::pin(async move { Err(error) }),
            },
            TitleKind::Show => None,
        };
        let opened = id.value().to_owned();

        Box::pin(async move {
            // **The franchise leads, and deterministically.** A reader who opens *Avatar* expects the
            // other *Avatar* films before anything a provider considers merely similar, and no
            // similarity score can be relied on to produce that: the sequels share a genre set with
            // thousands of other science-fiction films, and *Fire and Ash* is a new release with
            // almost no ratings for the rating-based endpoint to have learned from. The collection is
            // the one signal that says "same series" exactly, so it is read first and its parts are
            // the head of the answer.
            let mut items = match &movie {
                Some(url) => self.collection_parts(url).await,
                None => Vec::new(),
            };
            // Chronological, because a franchise is watched in order and TMDB's part order is not
            // documented to be one. The sort is stable, so undated parts keep the provider's order.
            items.sort_by_key(SearchCandidate::released);

            let found = self.list(&recommendations, kind).await?;
            // The case TMDB's own staff name — "still not perfect, especially for new releases": a
            // title too new for anyone to have rated alongside anything answers this empty, and an
            // empty rail is worse than a mediocre one, so `/similar` is asked instead.
            let found = if found.is_empty() {
                self.list(&fallback, kind).await?
            } else {
                found
            };
            items.extend(found);

            // The title itself and each part once: a collection's own list contains the film that was
            // opened, and a recommendation can repeat a part.
            let mut seen = std::collections::BTreeSet::new();
            seen.insert(opened);
            Ok(items
                .into_iter()
                .filter(|candidate| seen.insert(candidate.external_id().value().to_owned()))
                .take(crate::provider::RAIL_PAGE_LIMIT)
                .collect())
        })
    }

    fn credits(&self, id: &ExternalId, kind: TitleKind) -> ProviderFuture<'_, TitleCredits> {
        let endpoint = match kind {
            TitleKind::Movie => "movie",
            TitleKind::Show => "tv",
        };
        let url = format!("{}/{endpoint}/{}/credits", self.base, id.value());
        let credential = self.credential.clone();

        Box::pin(async move {
            let fetched = self
                .client
                .fetch(
                    Provider::Tmdb,
                    Request {
                        url: &url,
                        credential: Some(&credential),
                    },
                )
                .await?;
            let answer: CreditsAnswer =
                Body::new(Provider::Tmdb, &fetched.bytes, fetched.media_type())
                    .decode(SEARCH_BODY_CAP)?;

            let mut cast = Vec::new();
            for member in answer.cast.into_iter().take(40) {
                let name = member.name.trim().to_owned();
                if name.is_empty() {
                    continue;
                }
                let character = member.character.and_then(|value| {
                    let trimmed = value.trim();
                    (!trimmed.is_empty()).then(|| trimmed.to_owned())
                });
                cast.push(CastMember {
                    provider_id: member.id.to_string(),
                    name,
                    character,
                    order: member.order,
                    profile_path: member.profile_path.filter(|path| !path.trim().is_empty()),
                });
            }
            Ok(TitleCredits { cast })
        })
    }

    fn image(
        &self,
        path: &str,
        kind: peregrine_domain::catalogue::ImageKind,
    ) -> ProviderFuture<'_, Vec<u8>> {
        if !self.descriptor.capabilities().contains(Capability::Images) {
            let unsupported = ProviderError::Unsupported {
                provider: Provider::Tmdb,
                capability: "images",
            };
            return Box::pin(async move { Err(unsupported) });
        }
        // The size is this adapter's choice for the kind: a poster is shown in a grid, a backdrop behind
        // a title, and an episode's still on the card that lists it. `w300` is TMDB's own name for the
        // still size it documents, and it is deliberately not `w780`: a card is a fraction of a
        // backdrop's width, so fetching a hero-sized image for one spends bandwidth on pixels the screen
        // throws away.
        let size = match kind {
            peregrine_domain::catalogue::ImageKind::Poster => "w342",
            peregrine_domain::catalogue::ImageKind::Backdrop => "w780",
            peregrine_domain::catalogue::ImageKind::Still => "w300",
        };
        // A path from a provider is untrusted input on its way into a URL. Cleaning it a segment at a
        // time is what makes it safe *and* correct: `..` would climb out of the size directory, `.` and
        // an empty segment would produce a `//` that some servers serve and others refuse, and a path
        // beginning with a scheme never gets the chance to replace the host.
        let cleaned = path
            .split('/')
            .filter(|segment| !segment.is_empty() && *segment != ".." && *segment != ".")
            .collect::<Vec<_>>()
            .join("/");
        let url = format!("{}/{size}/{cleaned}", self.image_base);

        Box::pin(async move {
            let fetched = self
                .client
                .fetch(
                    Provider::Tmdb,
                    Request {
                        url: &url,
                        // The image CDN is public: sending the credential to a second host would hand
                        // it to whoever runs that host.
                        credential: None,
                    },
                )
                .await?;
            Ok(fetched.bytes)
        })
    }

    fn title(&self, id: &ExternalId, kind: TitleKind) -> ProviderFuture<'_, TitleDetails> {
        if !self.descriptor.capabilities().contains(Capability::Title) {
            let unsupported = ProviderError::Unsupported {
                provider: Provider::Tmdb,
                capability: "title",
            };
            return Box::pin(async move { Err(unsupported) });
        }
        let endpoint = match kind {
            TitleKind::Movie => "movie",
            TitleKind::Show => "tv",
        };
        let url = format!("{}/{endpoint}/{}", self.base, id.value());
        let credential = self.credential.clone();

        Box::pin(async move {
            let fetched = self
                .client
                .fetch(
                    Provider::Tmdb,
                    Request {
                        url: &url,
                        credential: Some(&credential),
                    },
                )
                .await?;
            let answer: DetailsAnswer =
                Body::new(Provider::Tmdb, &fetched.bytes, fetched.media_type())
                    .decode(SEARCH_BODY_CAP)?;

            let name = answer
                .title
                .or(answer.name)
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty())
                .and_then(|name| NonEmptyText::new(&name).ok())
                .ok_or(ProviderError::SchemaDrift {
                    provider: Provider::Tmdb,
                })?;
            let released = answer
                .release_date
                .or(answer.first_air_date)
                .filter(|date| !date.is_empty())
                .and_then(|date| date.parse().ok());
            let own_name = match kind {
                TitleKind::Movie => answer.original_title,
                TitleKind::Show => answer.original_name,
            };

            Ok(TitleDetails {
                kind,
                name,
                original_name: own_name.filter(|name| !name.is_empty()),
                overview: answer.overview.filter(|text| !text.is_empty()),
                tagline: answer.tagline.filter(|text| !text.is_empty()),
                released,
                runtime_minutes: answer.runtime.or_else(|| {
                    answer
                        .episode_run_time
                        .and_then(|times| times.first().copied())
                }),
                original_language: answer.original_language.filter(|code| !code.is_empty()),
                origin_country: answer.origin_country.filter(|codes| !codes.is_empty()),
                rating: answer.vote_average,
                votes: answer.vote_count,
                popularity: answer.popularity,
                adult: answer.adult,
                in_production: answer.in_production,
                season_count: answer.number_of_seasons,
                episode_count: answer.number_of_episodes,
                homepage: answer.homepage.filter(|url| !url.is_empty()),
                poster_path: answer.poster_path.filter(|path| !path.is_empty()),
                backdrop_path: answer.backdrop_path.filter(|path| !path.is_empty()),
            })
        })
    }

    fn seasons(&self, id: &ExternalId, kind: TitleKind) -> ProviderFuture<'_, Vec<SeasonDetails>> {
        if let Some(refusal) = self.season_refusal(kind) {
            return Box::pin(async move { Err(refusal) });
        }
        let url = match self.seasons_url(id) {
            Ok(url) => url.to_string(),
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let credential = self.credential.clone();

        Box::pin(async move {
            let fetched = self
                .client
                .fetch(
                    Provider::Tmdb,
                    Request {
                        url: &url,
                        credential: Some(&credential),
                    },
                )
                .await?;
            let answer: SeasonsAnswer =
                Body::new(Provider::Tmdb, &fetched.bytes, fetched.media_type())
                    .decode(SEARCH_BODY_CAP)?;

            Ok(answer
                .seasons
                .into_iter()
                .filter_map(SeasonEntry::into_details)
                .collect())
        })
    }

    fn season_episodes(
        &self,
        id: &ExternalId,
        kind: TitleKind,
        season_number: SeasonNumber,
    ) -> ProviderFuture<'_, Vec<EpisodeDetails>> {
        if let Some(refusal) = self.season_refusal(kind) {
            return Box::pin(async move { Err(refusal) });
        }
        let url = match self.season_url(id, season_number) {
            Ok(url) => url.to_string(),
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let credential = self.credential.clone();

        Box::pin(async move {
            let fetched = self
                .client
                .fetch(
                    Provider::Tmdb,
                    Request {
                        url: &url,
                        credential: Some(&credential),
                    },
                )
                .await?;
            let answer: EpisodesAnswer =
                Body::new(Provider::Tmdb, &fetched.bytes, fetched.media_type())
                    .decode(SEARCH_BODY_CAP)?;

            Ok(answer
                .episodes
                .into_iter()
                .filter_map(|episode| episode.into_details(season_number))
                .collect())
        })
    }
}

/// One title's details, as much of the documented object as this adapter reads.
///
/// One struct for films and series, because the two objects differ only in which of `title`/`name` and
/// `release_date`/`first_air_date` are filled. Everything is optional: a provider omits freely, and an
/// absent field must never be read as "clear this".
#[derive(Debug, Deserialize)]
struct DetailsAnswer {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    name: Option<String>,
    /// The film's franchise, when it belongs to one.
    ///
    /// **Films only, and the empty case is the normal one**: TMDB models franchises as film
    /// collections, a series has no equivalent, and most films belong to none. See
    /// [`Tmdb::similar`] for what this is read for.
    #[serde(default)]
    belongs_to_collection: Option<CollectionRef>,
    #[serde(default)]
    original_title: Option<String>,
    #[serde(default)]
    original_name: Option<String>,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    tagline: Option<String>,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    first_air_date: Option<String>,
    #[serde(default)]
    runtime: Option<u32>,
    #[serde(default)]
    episode_run_time: Option<Vec<u32>>,
    #[serde(default)]
    original_language: Option<String>,
    /// Documented for series. Left absent for films rather than guessed at a shape this build has not
    /// seen, because a provider's answer is not the place to find out what its documentation says.
    #[serde(default)]
    origin_country: Option<Vec<String>>,
    #[serde(default)]
    vote_average: Option<f64>,
    #[serde(default)]
    vote_count: Option<u64>,
    #[serde(default)]
    popularity: Option<f64>,
    #[serde(default)]
    adult: Option<bool>,
    #[serde(default)]
    in_production: Option<bool>,
    #[serde(default)]
    number_of_seasons: Option<u32>,
    #[serde(default)]
    number_of_episodes: Option<u32>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    poster_path: Option<String>,
    #[serde(default)]
    backdrop_path: Option<String>,
}

/// What `/tv/{id}` answers about a series' seasons, as much of it as this adapter reads.
///
/// **`seasons` is required rather than defaulted, and that is a decision about what an absence means.**
/// Every series TMDB serves carries the array, an empty one is the real statement "no season is
/// published yet", and defaulting a *missing* key would turn a drifted shape into that same statement —
/// a screen showing a show with no seasons because the provider renamed a field is worse than the
/// `SchemaDrift` this build reports instead.
#[derive(Debug, Deserialize)]
struct SeasonsAnswer {
    seasons: Vec<SeasonEntry>,
}

/// One season, as the series object lists it.
///
/// Everything optional for [`SearchResult`]'s reason — a provider omits freely — and the number is
/// optional too because a season with no usable number cannot be stored: `season.season_number` is
/// half of the unique key, so a row without one is a row the write path has no identity for.
#[derive(Debug, Deserialize)]
struct SeasonEntry {
    #[serde(default)]
    season_number: Option<i64>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    air_date: Option<String>,
    #[serde(default)]
    poster_path: Option<String>,
    #[serde(default)]
    episode_count: Option<i64>,
}

impl SeasonEntry {
    /// This entry as a season, or `None` when it names no number this build can store.
    fn into_details(self) -> Option<SeasonDetails> {
        let number = u32::try_from(self.season_number?).ok()?;
        Some(SeasonDetails {
            season_number: SeasonNumber::new(number),
            name: self.name.filter(|name| !name.trim().is_empty()),
            overview: self.overview.filter(|text| !text.trim().is_empty()),
            air_date: read_date(self.air_date),
            poster_path: self.poster_path.filter(|path| !path.trim().is_empty()),
            episode_count: self
                .episode_count
                .and_then(|count| u32::try_from(count).ok()),
        })
    }
}

/// What `/tv/{id}/season/{n}` answers, as much of it as this adapter reads.
///
/// `episodes` is required for the reason [`SeasonsAnswer`]'s `seasons` is: an empty array is a season
/// with no episodes, which is a state the screen draws, and a missing one is drift that must not look
/// like it.
#[derive(Debug, Deserialize)]
struct EpisodesAnswer {
    episodes: Vec<EpisodeEntry>,
}

/// One episode, as a season's answer lists it.
#[derive(Debug, Deserialize)]
struct EpisodeEntry {
    #[serde(default)]
    season_number: Option<i64>,
    #[serde(default)]
    episode_number: Option<i64>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    air_date: Option<String>,
    /// How long the episode runs. TMDB documents this as nullable, and an absent runtime is an episode
    /// without one rather than a runtime of zero.
    #[serde(default)]
    runtime: Option<i64>,
    #[serde(default)]
    still_path: Option<String>,
    /// The episode's score, read together with its count for [`scored_rating`]'s reason.
    #[serde(default)]
    vote_average: Option<f64>,
    #[serde(default)]
    vote_count: Option<u64>,
    /// TMDB's own spelling of where the episode sits in its season, which is the vocabulary
    /// `episode.episode_type` stores — the same five values, so a name this build does not know is a
    /// `None` rather than a row that fails the `CHECK`.
    #[serde(default)]
    episode_type: Option<String>,
}

impl EpisodeEntry {
    /// This entry as an episode, or `None` when it names no number this build can store.
    ///
    /// `requested` is the season the read asked for, and it is what the episode's season falls back to:
    /// the season is a segment of the path that was just fetched, so a per-episode `season_number`
    /// disagreeing with it would be the provider contradicting its own answer.
    fn into_details(self, requested: SeasonNumber) -> Option<EpisodeDetails> {
        let number = u32::try_from(self.episode_number?).ok()?;
        let season = self
            .season_number
            .and_then(|number| u32::try_from(number).ok())
            .map_or(requested, SeasonNumber::new);
        Some(EpisodeDetails {
            season_number: season,
            episode_number: EpisodeNumber::new(number),
            name: self.name.filter(|name| !name.trim().is_empty()),
            overview: self.overview.filter(|text| !text.trim().is_empty()),
            air_date: read_date(self.air_date),
            runtime_minutes: self.runtime.and_then(|minutes| u32::try_from(minutes).ok()),
            still_path: self.still_path.filter(|path| !path.trim().is_empty()),
            rating: scored_rating(self.vote_average, self.vote_count),
            votes: self.vote_count.filter(|count| *count > 0),
            episode_type: self
                .episode_type
                .and_then(|kind| peregrine_domain::catalogue::EpisodeType::parse(&kind)),
        })
    }
}

/// A date as TMDB writes one, or `None` when it wrote nothing this build can read.
///
/// One reader for the season, episode, and title mappings: TMDB spells an absent or unknown date as an
/// empty string rather than `null`, so an answer with no date is the same answer in both encodings.
/// A date that does not parse is an absent date rather than a failed read, which is the rule
/// [`DetailsAnswer`]'s `release_date` already follows.
fn read_date(value: Option<String>) -> Option<Date> {
    value
        .filter(|date| !date.is_empty())
        .and_then(|date| date.parse().ok())
}

#[derive(Deserialize)]
struct CreditsAnswer {
    #[serde(default)]
    cast: Vec<CastAnswer>,
}

#[derive(Deserialize)]
struct CastAnswer {
    id: u64,
    name: String,
    character: Option<String>,
    order: Option<u32>,
    profile_path: Option<String>,
}

/// Where TMDB serves images, and the widths this build asks for.
///
/// A second host, which is why it is a constant here rather than something a caller supplies: the image
/// CDN is public, needs no credential, and — per `AGENTS R24`'s documented exception — cannot
/// revalidate, so an image request carries no conditional header.
pub const IMAGE_BASE: &str = "https://image.tmdb.org/t/p";

/// A TMDB read token compiled into a release build, when CI supplied one.
///
/// The repository never holds a token. Release CI passes `PEREGRINE_TMDB_TOKEN` at compile time so
/// a shipped binary works on first launch while the source stays clean, and a key the user stores
/// still takes precedence. `None` means the setup screen must ask.
#[must_use]
pub fn builtin_token() -> Option<&'static str> {
    option_env!("PEREGRINE_TMDB_TOKEN")
}

/// What TMDB answers a search with, as much of it as this adapter reads.
///
/// No `deny_unknown_fields`: a provider that adds a field is not drifting, and refusing an answer
/// because it carries more than this build knows would break on the provider's next deploy.
#[derive(Debug, Deserialize)]
struct SearchAnswer {
    #[serde(default)]
    results: Vec<SearchResult>,
}

/// A film's collection, as `/movie/{id}` names it.
///
/// Only the identifier is read: the name is copy the provider owns and this build has no surface for,
/// and the parts are what the rail draws.
#[derive(Debug, Deserialize)]
struct CollectionRef {
    #[serde(default)]
    id: Option<u32>,
}

/// What `/collection/{id}` answers, as much of it as this adapter reads.
///
/// `parts` are the same objects a list endpoint carries — an id, a title, a date, a poster, a score —
/// so they become candidates through the mapping that already exists and a collection is no new kind
/// of title to this build.
#[derive(Debug, Deserialize)]
struct CollectionPartsAnswer {
    #[serde(default)]
    parts: Vec<SearchResult>,
}

/// What TMDB answers a discover page with, as much of it as this adapter reads.
///
/// `page` and `total_pages` are what make paging decidable: a caller that only had `results` would
/// have to guess whether the next request finds anything, and both guesses are wrong — see
/// [`ProviderPage`]. A provider that sends neither is taken at its word that this is the last page,
/// because asking again would be asking a question the answer declined to answer.
#[derive(Debug, Deserialize)]
struct BrowseAnswer {
    #[serde(default)]
    results: Vec<SearchResult>,
    #[serde(default)]
    page: Option<u32>,
    #[serde(default)]
    total_pages: Option<u32>,
}

impl BrowseAnswer {
    /// Whether a page after this one exists.
    ///
    /// Both numbers or no answer: a missing `page` makes "this is page one" an assumption, and an
    /// assumption is the whole thing this type exists to avoid.
    ///
    /// **`total_pages` is bounded by [`LAST_PAGE`], and believing it unbounded is a real defect.**
    /// TMDB answers a wide discover query with a `total_pages` in the tens of thousands while serving
    /// only the first five hundred: a request for page 501 is answered
    /// `status_code: 22, "Invalid page: Pages start at 1 and max at 500."`. So a reader who scrolled
    /// far enough was sent from a list to an error page by the adapter's own arithmetic — the cap
    /// belongs here, next to the endpoint whose limit it is, and not in a screen.
    fn has_more(&self) -> bool {
        match (self.page, self.total_pages) {
            (Some(page), Some(total)) => page < total.min(LAST_PAGE),
            _ => false,
        }
    }
}

/// What TMDB answers a genre list with, as much of it as this adapter reads.
#[derive(Debug, Deserialize)]
struct GenresAnswer {
    #[serde(default)]
    genres: Vec<GenreEntry>,
}

/// One genre in that answer.
///
/// Everything optional for the reason [`SearchResult`] states: a provider omits freely, and a genre
/// with no id or a blank name is one this build skips rather than one that fails the whole list.
#[derive(Debug, Deserialize)]
struct GenreEntry {
    #[serde(default)]
    id: Option<u32>,
    #[serde(default)]
    name: Option<String>,
}

impl GenreEntry {
    /// This entry as the port's genre, or `None` when it names nothing.
    ///
    /// A blank name is not a name — a control that drew one would offer a filter with no label —
    /// and the trim is what makes that true of `"  "` as well as of `""`.
    fn into_genre(self) -> Option<GenreRef> {
        let name = self.name?.trim().to_owned();
        if name.is_empty() {
            return None;
        }
        let name = NonEmptyText::new(&name).ok()?;
        Some(GenreRef::new(self.id?, name))
    }
}

/// One search result.
///
/// Everything that could be absent is: a person result carries no title, an unreleased film carries no
/// date, and a name that has not been translated yet can be empty. Each of those is a field this
/// adapter skips rather than a reason to fail the whole answer.
#[derive(Debug, Deserialize)]
struct SearchResult {
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    media_type: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    first_air_date: Option<String>,
    /// The poster's path on the image CDN, which every list endpoint sends. Documented as
    /// nullable, and an empty string is the same answer as `null`.
    #[serde(default)]
    poster_path: Option<String>,
    /// The backdrop's path on the image CDN, which the film and series list endpoints send beside
    /// the poster. Nullable, and read the same way: an empty or whitespace-only path is an absent
    /// one, which is what keeps a caller from asking the image host for its root.
    #[serde(default)]
    backdrop_path: Option<String>,
    /// The audience score, out of ten. **`0.0` is what TMDB sends an item nobody has voted on**, so
    /// it is read together with `vote_count` rather than on its own.
    #[serde(default)]
    vote_average: Option<f64>,
    #[serde(default)]
    vote_count: Option<u64>,
    /// The short synopsis every list endpoint sends for an item that has one. Documented as
    /// nullable, and an empty string is the same answer as `null`.
    #[serde(default)]
    overview: Option<String>,
    /// The genres this item was listed under, as TMDB's own identifiers.
    ///
    /// **A list answer carries these and a details answer does not** — `/movie/{id}` spells genres as
    /// objects with names — which is why they travel on the candidate rather than being looked up:
    /// they are what lets a caller ask for more of what it was just shown.
    #[serde(default)]
    genre_ids: Option<Vec<u32>>,
}

/// TMDB's score, or `None` when the item is unrated.
///
/// A `vote_average` of zero with nothing counting it is not a rating of zero — it is the value TMDB
/// puts on an item nobody has voted on — and a `0.0` badge on a poster is a claim the provider never
/// made. A zero *with* votes behind it is a real, if unflattering, score and is kept.
fn scored_rating(average: Option<f64>, votes: Option<u64>) -> Option<f64> {
    let average = average?;
    if average == 0.0 && votes.unwrap_or(0) == 0 {
        None
    } else {
        Some(average)
    }
}

impl SearchResult {
    /// This result as a candidate, or `None` when it is not a title this build can use.
    ///
    /// A person result, an id TMDB did not send, a name that is empty after trimming, or a kind this
    /// build does not have are all skipped: a search that answered four titles and a person is a
    /// search that found four titles.
    fn into_candidate(self) -> Option<SearchCandidate> {
        self.into_candidate_for(None)
    }

    /// The same, when the endpoint that answered already fixed the kind.
    ///
    /// `/search/multi` sends a `media_type` per result and it is the only thing that can say
    /// whether a result is a film or a series, so a search passes `None` and reads it. Every other
    /// endpoint this adapter reads is already one kind — `/movie/popular` cannot answer a series —
    /// and those answers carry no `media_type` at all, so passing the kind the endpoint promises is
    /// what keeps a rail from answering an empty list because a field TMDB never sends was missing.
    ///
    /// The promised kind is a fact about the *endpoint*, and it is stated here rather than in the
    /// rail mapping because this is where a result becomes a candidate: a rail that asked
    /// `/movie/upcoming` and labelled the answer `Show` would be a mistake nothing else would catch.
    ///
    /// The artwork, the backdrop, the score, the synopsis, and the vote count come along because a
    /// list answer is where they are free (`PG-E7-08`, `PG-E7-05`): reading them here is what makes
    /// filling a rail of posters cost one request per title rather than two. A path or a synopsis
    /// that is empty or whitespace is an absent one rather than an empty one, for the reason
    /// [`SearchCandidate::with_artwork_path`] states, and a vote count of zero is TMDB saying nobody
    /// has voted — the same absence [`scored_rating`] reads.
    fn into_candidate_for(self, fixed: Option<TitleKind>) -> Option<SearchCandidate> {
        let kind = match fixed {
            Some(kind) => kind,
            None => match self.media_type.as_deref() {
                Some("movie") => TitleKind::Movie,
                Some("tv") => TitleKind::Show,
                _ => return None,
            },
        };
        let name = self
            .title
            .or(self.name)
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())?;
        let value = self.id?.to_string();
        let external_id = ExternalId::new(Provider::Tmdb, &value).ok()?;
        let name = NonEmptyText::new(&name).ok()?;
        let released = self
            .release_date
            .or(self.first_air_date)
            .filter(|date| !date.is_empty())
            .and_then(|date| date.parse().ok());
        let artwork = self.poster_path.filter(|path| !path.trim().is_empty());
        let backdrop = self.backdrop_path.filter(|path| !path.trim().is_empty());
        let rating = scored_rating(self.vote_average, self.vote_count);
        let overview = self.overview.filter(|text| !text.trim().is_empty());
        let votes = self.vote_count.filter(|count| *count > 0);
        let genre_ids = self.genre_ids.unwrap_or_default();
        Some(
            SearchCandidate::new(external_id, name, kind, released)
                .with_artwork_path(artwork)
                .with_backdrop_path(backdrop)
                .with_rating(rating)
                .with_overview(overview)
                .with_votes(votes)
                .with_genre_ids(genre_ids),
        )
    }
}

/// A `NonZeroU32` for the policies above, which are built rather than parsed.
fn nonzero(value: u32) -> std::num::NonZeroU32 {
    std::num::NonZeroU32::new(value).unwrap_or(std::num::NonZeroU32::MIN)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use peregrine_domain::catalogue::TitleKind;
    use peregrine_domain::{NonEmptyText, Provider};
    use wiremock::matchers::{header, method, path, path_regex, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{RATING_VOTE_FLOOR, Tmdb};
    use crate::capability::{Capabilities, Capability};
    use crate::error::ProviderError;
    use crate::provider::{BrowseRequest, BrowseSort, MetadataProvider, RailKind, SearchQuery};

    /// A multi-search answer with the three shapes that matter: a film, a series, and a person.
    ///
    /// The film carries artwork and a score and the series carries neither, because a list answer
    /// may do either and both are candidates.
    const MULTI: &str = r#"{
      "page": 1,
      "results": [
        {"media_type": "movie", "id": 603, "title": "The Matrix", "release_date": "1999-03-31",
         "poster_path": "/matrix.jpg", "vote_average": 8.2, "vote_count": 24000},
        {"media_type": "tv", "id": 1396, "name": "Breaking Bad", "first_air_date": "2008-01-20"},
        {"media_type": "person", "id": 6384, "name": "Keanu Reeves"}
      ],
      "total_results": 3
    }"#;

    fn query(text: &str, limit: u32) -> SearchQuery {
        SearchQuery::new(
            NonEmptyText::new(text).expect("a non-empty query"),
            NonZeroU32::new(limit).expect("a non-zero limit"),
            None,
        )
    }

    /// A JSON answer, with the content type `hygiene` requires.
    ///
    /// Written as a helper because three fixtures first forgot it and every one of them failed with
    /// `Decode` — which is the adapter being right: `PG-E5-05` refuses an answer whose type is not the
    /// one the endpoint documents, and a test that omitted the header was testing that refusal by
    /// accident.
    fn json(body: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_raw(body, "application/json")
    }

    fn adapter(server: &MockServer) -> Tmdb {
        Tmdb::new(&server.uri(), &server.uri(), "test-token".to_owned())
            .expect("the adapter builds")
    }

    #[tokio::test]
    async fn a_search_maps_the_providers_answer_into_domain_candidates() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/multi"))
            .and(query_param("query", "matrix"))
            .and(query_param("include_adult", "false"))
            .respond_with(json(MULTI))
            .mount(&server)
            .await;

        let found = adapter(&server)
            .search(&query("matrix", 20))
            .await
            .expect("a search answers");

        assert_eq!(found.len(), 2, "the person result is not a title");
        assert_eq!(found[0].name().as_str(), "The Matrix");
        assert_eq!(found[0].external_id().provider(), Provider::Tmdb);
        assert_eq!(found[0].external_id().value(), "603");
        assert_eq!(
            found[0].artwork_path(),
            Some("/matrix.jpg"),
            "a list answer's poster is carried rather than fetched again"
        );
        assert_eq!(found[0].rating(), Some(8.2));
        assert_eq!(found[1].name().as_str(), "Breaking Bad");
        assert!(found[1].released().is_some(), "the first air date is read");
        assert_eq!(found[1].artwork_path(), None, "an item with no poster");
        assert_eq!(found[1].rating(), None, "an item with no score");
    }

    #[tokio::test]
    async fn a_list_item_maps_the_four_fields_a_list_answer_carries_and_an_unrated_one_maps_none() {
        // Four items and the answers a provider can give for each field: everything, nothing, a
        // score of zero with nothing counting it, and strings that are only whitespace. **The zero
        // is the one worth pinning**: TMDB sends `vote_average: 0` and `vote_count: 0` for an item
        // nobody has voted on, and a `0.0` badge or a "0 votes" line on a poster is a claim the
        // provider never made.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/movie/popular"))
            .respond_with(json(
                r#"{"results": [
                    {"id": 1, "title": "Rated", "poster_path": "/one.jpg", "vote_average": 8.2, "vote_count": 24000,
                     "overview": "A hacker learns the truth."},
                    {"id": 2, "title": "Bare"},
                    {"id": 3, "title": "Unrated", "poster_path": "   ", "vote_average": 0, "vote_count": 0,
                     "overview": ""},
                    {"id": 4, "title": "Zero but voted", "vote_average": 0, "vote_count": 12,
                     "overview": "   "}
                ]}"#,
            ))
            .mount(&server)
            .await;

        let found = adapter(&server)
            .rail(RailKind::PopularMovies, page(1))
            .await
            .expect("the rail answers");

        assert_eq!(found.len(), 4);
        assert_eq!(found[0].artwork_path(), Some("/one.jpg"));
        assert_eq!(found[0].rating(), Some(8.2));
        assert_eq!(found[0].overview(), Some("A hacker learns the truth."));
        assert_eq!(found[0].votes(), Some(24_000));
        assert_eq!(found[1].artwork_path(), None);
        assert_eq!(found[1].rating(), None);
        assert_eq!(found[1].overview(), None);
        assert_eq!(found[1].votes(), None);
        assert_eq!(
            found[2].artwork_path(),
            None,
            "a whitespace path is an absent path, not an empty one"
        );
        assert_eq!(
            found[2].rating(),
            None,
            "a zero an item's own answer gives with no votes is unrated"
        );
        assert_eq!(
            found[2].overview(),
            None,
            "an empty synopsis is no synopsis, not a blank line"
        );
        assert_eq!(
            found[2].votes(),
            None,
            "nothing counted a zero, so there is no count to draw"
        );
        assert_eq!(
            found[3].rating(),
            Some(0.0),
            "a zero the provider counted is a score"
        );
        assert_eq!(found[3].votes(), Some(12));
        assert_eq!(
            found[3].overview(),
            None,
            "a whitespace-only synopsis says nothing"
        );
    }

    #[tokio::test]
    async fn the_request_carries_the_credential_as_a_bearer_header() {
        // Not as a query parameter: a key in a URL reaches every log and every error path, which is
        // why this crate's own error carries no URL either.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(json(MULTI))
            .expect(1)
            .mount(&server)
            .await;

        adapter(&server)
            .search(&query("matrix", 5))
            .await
            .expect("the credential is sent");
    }

    #[tokio::test]
    async fn a_kind_restricts_the_endpoint_rather_than_filtering_the_answer() {
        // Asking the wrong endpoint and dropping half the results would spend a request to learn
        // something the query already said.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/movie"))
            .respond_with(json(r#"{"results":[]}"#))
            .expect(1)
            .mount(&server)
            .await;

        let mut restricted = query("matrix", 5);
        restricted = SearchQuery::new(
            restricted.text().clone(),
            restricted.limit(),
            Some(peregrine_domain::catalogue::TitleKind::Movie),
        );

        adapter(&server)
            .search(&restricted)
            .await
            .expect("the restricted endpoint answers");
    }

    #[tokio::test]
    async fn the_limit_is_respected_and_a_result_with_nothing_to_name_it_is_skipped() {
        // The port's conformance suite checks the limit; this checks what happens to a result that
        // cannot become a candidate at all, which is the malformed case §4 asks about.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/multi"))
            .respond_with(json(
                r#"{"results": [
                    {"media_type": "movie", "id": 1, "title": "   "},
                    {"media_type": "movie", "id": 2, "title": "Dune", "release_date": ""},
                    {"media_type": "unknown", "id": 3, "title": "Nothing"},
                    {"media_type": "movie", "title": "No id"},
                    {"media_type": "movie", "id": 5, "title": "Arrival", "release_date": "2016-11-11"}
                ]}"#,
            ))
            .mount(&server)
            .await;

        let found = adapter(&server)
            .search(&query("anything", 20))
            .await
            .expect("a search answers");

        assert_eq!(found.len(), 2, "Dune and Arrival: {found:?}");
        assert_eq!(found[0].name().as_str(), "Dune");
        assert!(
            found[0].released().is_none(),
            "an empty date is an absent date"
        );
        assert_eq!(found[1].name().as_str(), "Arrival");
    }

    #[tokio::test]
    async fn the_limit_truncates_the_answer() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/multi"))
            .and(query_param("query", "matrix"))
            .respond_with(json(MULTI))
            .mount(&server)
            .await;

        let found = adapter(&server)
            .search(&query("matrix", 1))
            .await
            .expect("a search answers");

        assert_eq!(found.len(), 1);
    }

    #[tokio::test]
    async fn the_statuses_that_are_final_are_reported_as_themselves() {
        for (status, expected) in [
            (404, "not found"),
            (401, "unauthorized"),
            (403, "unauthorized"),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(status))
                .mount(&server)
                .await;

            let error = adapter(&server)
                .search(&query("matrix", 5))
                .await
                .expect_err("the status is final");

            let named = match error {
                ProviderError::NotFound { .. } => "not found",
                ProviderError::Unauthorized { .. } => "unauthorized",
                other => panic!("{status} became {other:?}"),
            };
            assert_eq!(named, expected, "status {status}");
        }
    }

    #[tokio::test]
    async fn an_answer_that_is_not_what_the_provider_documents_is_drift() {
        // A stale row keeps rendering from cache and is marked for a later sync (ADR 0007 rule 7), so
        // the shape has to be a reported state rather than a panic or an empty list.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(json("{\"results\": \"not a list\"}"))
            .mount(&server)
            .await;

        let error = adapter(&server)
            .search(&query("matrix", 5))
            .await
            .expect_err("the shape drifted");

        assert!(
            matches!(error, ProviderError::SchemaDrift { .. }),
            "got {error:?}"
        );
    }

    #[tokio::test]
    async fn a_probe_answers_ok_when_the_credential_is_accepted() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/configuration"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(json(
                "{\"images\":{\"secure_base_url\":\"https://image.tmdb.org/t/p/\"}}",
            ))
            .mount(&server)
            .await;

        adapter(&server)
            .check()
            .await
            .expect("a working credential answers");
    }

    #[tokio::test]
    async fn a_refused_credential_is_unauthorized_rather_than_a_transport_failure() {
        // PG-E5-20's acceptance, at the layer that decides it. The settings screen says "that key was
        // refused" or "TMDB could not be reached" and those are different instructions to a person, so
        // a 401 and a 500 must not arrive as the same typed failure. Nothing about the key itself is
        // in the error: this crate's vocabulary carries no free text (SECURITY.md T13).
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/configuration"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let refused = adapter(&server)
            .check()
            .await
            .expect_err("the credential was refused");

        assert_eq!(
            refused,
            ProviderError::Unauthorized {
                provider: Provider::Tmdb
            }
        );
    }

    #[tokio::test]
    async fn a_broken_provider_is_not_reported_as_a_broken_credential() {
        // The other half of that distinction: a provider having a bad day must not send someone to
        // re-paste a key that was never the problem.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/configuration"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let failed = adapter(&server)
            .check()
            .await
            .expect_err("the provider failed");

        assert!(
            matches!(failed, ProviderError::Transport { .. }),
            "got {failed:?}"
        );
        assert_ne!(
            failed,
            ProviderError::Unauthorized {
                provider: Provider::Tmdb
            }
        );
    }

    #[tokio::test]
    async fn a_probe_reads_no_model_of_the_answer() {
        // The rule the port states: a probe must not decode what it does not need. A configuration
        // shape this build has never seen — TMDB adding a section, or a proxy answering with something
        // else entirely — is still a working credential, and a build that failed here would send
        // people to fix a key that works.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/configuration"))
            .respond_with(json(
                "{\"something\":\"this build does not read\",\"nested\":[1,2,3]}",
            ))
            .mount(&server)
            .await;

        adapter(&server)
            .check()
            .await
            .expect("the credential works whatever the body says");
    }

    #[tokio::test]
    async fn a_probe_sends_the_credential_in_the_header_and_never_in_the_url() {
        // The rule the port's error type exists for: a key in a URL reaches every log, every error,
        // and any redirect that echoes it. Asserted on the request the server actually received.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/configuration"))
            .and(header("authorization", "Bearer test-token"))
            .and(query_param("api_key", "test-token"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/configuration"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(json("{}"))
            .mount(&server)
            .await;

        adapter(&server)
            .check()
            .await
            .expect("the credential works");
    }

    #[tokio::test]
    async fn a_film_s_details_are_mapped_into_domain_fields() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/movie/603"))
            .respond_with(json(
                r#"{
                    "title": "The Matrix",
                    "original_title": "The Matrix",
                    "overview": "A hacker learns the truth.",
                    "tagline": "Welcome to the real world.",
                    "release_date": "1999-03-31",
                    "runtime": 136,
                    "original_language": "en",
                    "vote_average": 8.2,
                    "vote_count": 24000,
                    "popularity": 42.5,
                    "adult": false,
                    "homepage": "https://thematrix.com",
                    "poster_path": "/poster.jpg",
                    "backdrop_path": "/backdrop.jpg"
                }"#,
            ))
            .mount(&server)
            .await;

        let details = adapter(&server)
            .title(
                &peregrine_domain::ExternalId::new(Provider::Tmdb, "603").expect("an id"),
                peregrine_domain::catalogue::TitleKind::Movie,
            )
            .await
            .expect("the details answer");

        assert_eq!(details.name.as_str(), "The Matrix");
        assert_eq!(
            details.overview.as_deref(),
            Some("A hacker learns the truth.")
        );
        assert_eq!(details.runtime_minutes, Some(136));
        assert_eq!(details.rating, Some(8.2));
        assert_eq!(details.votes, Some(24_000));
        assert_eq!(
            details.released,
            Some("1999-03-31".parse().expect("a date"))
        );
        assert_eq!(details.poster_path.as_deref(), Some("/poster.jpg"));
    }

    #[tokio::test]
    async fn an_image_is_fetched_without_the_credential_and_cannot_climb_out_of_its_directory() {
        // Two things at once, because they are the same request: the public CDN gets **no** credential —
        // sending it to a second host would hand it to whoever runs that host — and a path from a
        // provider cannot escape the size directory it is appended to.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/w342/poster.jpg"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"jpeg".to_vec()))
            .mount(&server)
            .await;

        let adapter = Tmdb::new(
            &format!("{}/3", server.uri()),
            &server.uri(),
            "test-token".to_owned(),
        )
        .expect("the adapter builds");
        let bytes = adapter
            .image(
                // A path a compromised or mistaken provider could send: cleaned, it addresses a file
                // in this size's directory rather than one above it.
                "/../poster.jpg",
                peregrine_domain::catalogue::ImageKind::Poster,
            )
            .await
            .expect("the image answers");
        assert_eq!(bytes, b"jpeg");
    }

    #[tokio::test]
    async fn the_descriptor_declares_what_the_adapter_can_actually_do() {
        // The port's rule: a provider either answers or refuses, and it never spends a request
        // finding out which. This is the declaration the conformance suite reads.
        let server = MockServer::start().await;
        let adapter = adapter(&server);
        let descriptor = adapter.descriptor();

        assert_eq!(descriptor.provider(), Provider::Tmdb);
        assert_eq!(descriptor.display_name(), "TMDB");
        assert!(descriptor.can(crate::capability::Capability::Search));
        assert!(
            descriptor.can(crate::capability::Capability::Trending),
            "the rail operation is gated on this capability"
        );
    }

    /// One rail and the documented path that answers it.
    ///
    /// A table because the mapping *is* the test: every row is one endpoint the task names, and a
    /// rail whose path changed would fail its row rather than a shared assertion.
    const RAIL_PATHS: [(RailKind, &str); 7] = [
        (RailKind::TrendingWeek, "/trending/all/week"),
        (RailKind::PopularMovies, "/movie/popular"),
        (RailKind::TopRatedMovies, "/movie/top_rated"),
        (RailKind::UpcomingMovies, "/movie/upcoming"),
        (RailKind::PopularShows, "/tv/popular"),
        (RailKind::OnTheAirShows, "/tv/on_the_air"),
        (RailKind::TopRatedShows, "/tv/top_rated"),
    ];

    /// One page of a rail, in the shape `/movie/popular` documents: no `media_type`, because the
    /// endpoint has already said what kind it answers. One item carries artwork, a backdrop and a
    /// score and the other carries none of them, which is what a real page looks like — and one
    /// carries a whitespace backdrop, which is TMDB's other spelling of "no wide image".
    const MOVIE_RAIL: &str = r#"{
      "page": 1,
      "results": [
        {"id": 603, "title": "The Matrix", "release_date": "1999-03-31",
         "poster_path": "/matrix.jpg", "backdrop_path": "/matrix-wide.jpg",
         "vote_average": 7.0, "vote_count": 100},
        {"id": 27205, "title": "Inception", "release_date": "2010-07-16"},
        {"id": 157336, "title": "Interstellar", "release_date": "2014-11-05",
         "backdrop_path": "   "}
      ],
      "total_pages": 500
    }"#;

    /// One page of `/trending/all/week`, which does carry a `media_type` per result — and a person,
    /// which is not a title.
    const TRENDING_RAIL: &str = r#"{
      "page": 1,
      "results": [
        {"media_type": "movie", "id": 603, "title": "The Matrix", "release_date": "1999-03-31"},
        {"media_type": "tv", "id": 1396, "name": "Breaking Bad", "first_air_date": "2008-01-20"},
        {"media_type": "person", "id": 6384, "name": "Keanu Reeves"}
      ]
    }"#;

    fn page(number: u32) -> std::num::NonZeroU32 {
        std::num::NonZeroU32::new(number).expect("the test passes a non-zero page")
    }

    #[tokio::test]
    async fn every_rail_reads_the_endpoint_it_names() {
        for (rail, endpoint) in RAIL_PATHS {
            let server = MockServer::start().await;
            let body = if rail == RailKind::TrendingWeek {
                TRENDING_RAIL
            } else {
                MOVIE_RAIL
            };
            Mock::given(method("GET"))
                .and(path(endpoint))
                .and(query_param("page", "1"))
                .and(header("authorization", "Bearer test-token"))
                .respond_with(json(body))
                .expect(1)
                .mount(&server)
                .await;

            let found = adapter(&server)
                .rail(rail, page(1))
                .await
                .unwrap_or_else(|error| panic!("{} answered {error:?}", rail.as_str()));

            assert!(!found.is_empty(), "{} answered nothing", rail.as_str());
            for candidate in &found {
                assert_eq!(candidate.external_id().provider(), Provider::Tmdb);
                // The endpoint fixes the kind for every rail but the mixed one, and a candidate
                // labelled against the endpoint that answered it is the mistake this pins.
                if let Some(kind) = rail.kind() {
                    assert_eq!(candidate.kind(), kind, "{}", rail.as_str());
                }
            }
        }
    }

    #[tokio::test]
    async fn a_trending_rail_carries_both_kinds_and_drops_what_is_not_a_title() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/trending/all/week"))
            .respond_with(json(TRENDING_RAIL))
            .mount(&server)
            .await;

        let found = adapter(&server)
            .rail(RailKind::TrendingWeek, page(1))
            .await
            .expect("the rail answers");

        assert_eq!(found.len(), 2, "a person is not a title: {found:?}");
        assert_eq!(
            found[0].kind(),
            peregrine_domain::catalogue::TitleKind::Movie
        );
        assert_eq!(
            found[1].kind(),
            peregrine_domain::catalogue::TitleKind::Show
        );
    }

    #[tokio::test]
    async fn a_rail_page_is_the_callers_and_not_always_the_first() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/movie/top_rated"))
            .and(query_param("page", "3"))
            .respond_with(json(MOVIE_RAIL))
            .expect(1)
            .mount(&server)
            .await;

        adapter(&server)
            .rail(RailKind::TopRatedMovies, page(3))
            .await
            .expect("the third page answers");
    }

    #[tokio::test]
    async fn a_rail_that_needs_details_does_not_need_a_media_type() {
        // The bug this pins: `/movie/popular` sends no `media_type`, so a mapping that read one
        // would answer an empty list for every single-kind rail — a screen with rails that are
        // permanently empty and no error anywhere to explain it.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/movie/popular"))
            .respond_with(json(MOVIE_RAIL))
            .mount(&server)
            .await;

        let found = adapter(&server)
            .rail(RailKind::PopularMovies, page(1))
            .await
            .expect("the rail answers");

        assert_eq!(found.len(), 3);
        assert_eq!(found[0].name().as_str(), "The Matrix");
        assert_eq!(found[0].artwork_path(), Some("/matrix.jpg"));
        assert_eq!(found[0].rating(), Some(7.0));
        assert_eq!(
            found[0].backdrop_path(),
            Some("/matrix-wide.jpg"),
            "the wide image the list answer already sends is what the hero draws, and it is free"
        );
        assert_eq!(
            found[1].artwork_path(),
            None,
            "an item the list gave no poster still comes back"
        );
        assert_eq!(
            found[1].backdrop_path(),
            None,
            "an item with no wide image says none rather than borrowing the poster"
        );
        assert_eq!(
            found[2].backdrop_path(),
            None,
            "a whitespace backdrop is TMDB's other spelling of no backdrop, and a caller that kept \
             it would ask the image host for its root"
        );
    }

    #[tokio::test]
    async fn a_refused_credential_on_a_rail_is_unauthorized_and_a_broken_provider_is_not() {
        for (status, unauthorized) in [(401, true), (500, false)] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/movie/popular"))
                .respond_with(ResponseTemplate::new(status))
                .mount(&server)
                .await;

            let error = adapter(&server)
                .rail(RailKind::PopularMovies, page(1))
                .await
                .expect_err("the status is final");

            if unauthorized {
                assert_eq!(
                    error,
                    ProviderError::Unauthorized {
                        provider: Provider::Tmdb
                    },
                    "status {status}"
                );
            } else {
                assert!(
                    matches!(error, ProviderError::Transport { .. }),
                    "status {status} became {error:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn a_rail_answer_that_is_not_what_the_provider_documents_is_drift() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/tv/on_the_air"))
            .respond_with(json("{\"results\": \"not a list\"}"))
            .mount(&server)
            .await;

        let error = adapter(&server)
            .rail(RailKind::OnTheAirShows, page(1))
            .await
            .expect_err("the shape drifted");

        assert!(
            matches!(error, ProviderError::SchemaDrift { .. }),
            "got {error:?}"
        );
    }

    #[tokio::test]
    async fn a_rail_the_descriptor_does_not_declare_is_refused_without_a_request() {
        // The port's rule, at the layer that decides it: an adapter that has said it cannot serve
        // trending must refuse, name itself and the capability, and spend no token finding out. The
        // server below would answer 200 to everything, so an implementation that asked first would
        // pass a weaker test — `.expect(0)` is what makes this one about the refusal.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(json(MOVIE_RAIL))
            .expect(0)
            .mount(&server)
            .await;

        let refusing = Tmdb::with_capabilities(
            &server.uri(),
            &server.uri(),
            "test-token".to_owned(),
            crate::capability::Capabilities::EMPTY.with(Capability::Search),
        )
        .expect("the adapter builds");
        let error = refusing
            .rail(RailKind::PopularMovies, page(1))
            .await
            .expect_err("the capability is not declared");

        assert_eq!(
            error,
            ProviderError::Unsupported {
                provider: Provider::Tmdb,
                capability: "trending",
            }
        );
    }

    /// A browse request, built the way the port asks for one.
    fn request(
        kind: TitleKind,
        number: u32,
        sort: BrowseSort,
        genre: Option<u32>,
        min_rating: Option<u8>,
    ) -> BrowseRequest {
        BrowseRequest::new(kind, page(number), sort, genre, min_rating)
    }

    /// One discover page, in the shape `/discover/movie` documents.
    const DISCOVER_PAGE: &str = r#"{
      "page": 2,
      "total_pages": 4,
      "results": [
        {"id": 603, "title": "The Matrix", "release_date": "1999-03-31",
         "poster_path": "/matrix.jpg", "vote_average": 8.2, "vote_count": 24000,
         "overview": "A hacker learns the truth."}
      ]
    }"#;

    #[tokio::test]
    async fn a_browse_page_sends_the_filters_it_was_given_and_maps_the_answer() {
        // Every filter in one request, because that is the shape the screen composes: a page, an
        // order, a genre, and a floor. A parameter that stopped being sent would fail here rather
        // than silently widen the list a person asked to narrow.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/discover/movie"))
            .and(query_param("page", "2"))
            .and(query_param("sort_by", "vote_average.desc"))
            .and(query_param("with_genres", "28"))
            .and(query_param("vote_average.gte", "7"))
            .and(query_param("vote_count.gte", RATING_VOTE_FLOOR.to_string()))
            .and(query_param("include_adult", "false"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(json(DISCOVER_PAGE))
            .expect(1)
            .mount(&server)
            .await;

        let answer = adapter(&server)
            .browse(&request(
                TitleKind::Movie,
                2,
                BrowseSort::Rating,
                Some(28),
                Some(7),
            ))
            .await
            .expect("the page answers");

        assert_eq!(answer.items().len(), 1);
        assert_eq!(answer.items()[0].name().as_str(), "The Matrix");
        assert_eq!(answer.items()[0].rating(), Some(8.2));
        assert_eq!(
            answer.items()[0].overview(),
            Some("A hacker learns the truth."),
            "a list answer's prose rides the candidate, as it does for a rail"
        );
        assert!(
            answer.has_more(),
            "page two of four has pages after it, which is what the screen's load-more hangs off"
        );
    }

    #[tokio::test]
    async fn a_browse_page_that_sends_nothing_but_its_items_says_there_is_no_more() {
        // A provider that does not say how many pages it has is taken at its word that this is the
        // last one. Asking again would be asking a question the answer declined to answer, and a
        // screen that guessed from the item count would scroll for ever.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/discover/movie"))
            .respond_with(json(r#"{"results": [{"id": 1, "title": "Bare"}]}"#))
            .mount(&server)
            .await;
        let bare = adapter(&server)
            .browse(&request(
                TitleKind::Movie,
                1,
                BrowseSort::Popularity,
                None,
                None,
            ))
            .await
            .expect("the page answers");
        assert!(
            !bare.has_more(),
            "an answer with no page count is the last page"
        );

        // And the honest other half: a page that is not the last one says so.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/discover/movie"))
            .respond_with(json(r#"{"page": 4, "total_pages": 4, "results": []}"#))
            .mount(&server)
            .await;
        let last = adapter(&server)
            .browse(&request(
                TitleKind::Movie,
                4,
                BrowseSort::Popularity,
                None,
                None,
            ))
            .await
            .expect("the page answers");
        assert!(!last.has_more(), "the last page has nothing after it");
    }

    /// One order, one kind, and the endpoint and parameter that answer them.
    ///
    /// A table because the mapping *is* the test: the two kinds do not share a date field, so a row
    /// whose parameter changed would fail its own case rather than a shared assertion.
    const BROWSE_ORDERS: [(TitleKind, BrowseSort, &str, &str); 8] = [
        (
            TitleKind::Movie,
            BrowseSort::Popularity,
            "/discover/movie",
            "popularity.desc",
        ),
        (
            TitleKind::Movie,
            BrowseSort::Rating,
            "/discover/movie",
            "vote_average.desc",
        ),
        (
            TitleKind::Movie,
            BrowseSort::Newest,
            "/discover/movie",
            "primary_release_date.desc",
        ),
        (
            TitleKind::Movie,
            BrowseSort::Oldest,
            "/discover/movie",
            "primary_release_date.asc",
        ),
        (
            TitleKind::Show,
            BrowseSort::Popularity,
            "/discover/tv",
            "popularity.desc",
        ),
        (
            TitleKind::Show,
            BrowseSort::Rating,
            "/discover/tv",
            "vote_average.desc",
        ),
        (
            TitleKind::Show,
            BrowseSort::Newest,
            "/discover/tv",
            "first_air_date.desc",
        ),
        (
            TitleKind::Show,
            BrowseSort::Oldest,
            "/discover/tv",
            "first_air_date.asc",
        ),
    ];

    #[tokio::test]
    async fn every_order_and_kind_reads_the_endpoint_and_sort_field_it_names() {
        for (kind, sort, endpoint, sort_by) in BROWSE_ORDERS {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path(endpoint))
                .and(query_param("sort_by", sort_by))
                .respond_with(json(r#"{"page": 1, "total_pages": 1, "results": []}"#))
                .expect(1)
                .mount(&server)
                .await;

            adapter(&server)
                .browse(&request(kind, 1, sort, None, None))
                .await
                .unwrap_or_else(|error| {
                    panic!("{kind:?} {sort:?} on {endpoint} answered {error:?}")
                });
        }
    }

    #[tokio::test]
    async fn a_floor_of_zero_is_not_sent_as_one_and_the_vote_floor_is_only_for_the_score_order() {
        // Zero is what TMDB gives an item nobody has voted on rather than a floor, so a caller that
        // means "no floor" must send nothing; and the vote floor belongs to the score order alone,
        // because every other order is meaningful without it. Both are asserted on the URL, which
        // is where "was it sent" is decidable — a server that ignored the parameter could not tell
        // the difference.
        let adapter = Tmdb::new(
            "https://api.themoviedb.org/3",
            "https://image.tmdb.org/t/p",
            "test-token".to_owned(),
        )
        .expect("the adapter builds");

        let unfiltered = adapter
            .browse_url(&request(
                TitleKind::Movie,
                1,
                BrowseSort::Popularity,
                None,
                Some(0),
            ))
            .expect("a url")
            .to_string();
        assert!(unfiltered.contains("sort_by=popularity.desc"));
        assert!(
            !unfiltered.contains("vote_average.gte"),
            "a floor of zero is no floor: {unfiltered}"
        );
        assert!(
            !unfiltered.contains("with_genres"),
            "no genre was chosen: {unfiltered}"
        );
        assert!(
            !unfiltered.contains("vote_count.gte"),
            "the vote floor belongs to the score order and not to this one: {unfiltered}"
        );

        let by_score = adapter
            .browse_url(&request(
                TitleKind::Movie,
                1,
                BrowseSort::Rating,
                None,
                None,
            ))
            .expect("a url")
            .to_string();
        assert!(
            by_score.contains(&format!("vote_count.gte={RATING_VOTE_FLOOR}")),
            "ordering by score needs a vote floor or the page is a wall of 10.0 from one vote: {by_score}"
        );
    }

    #[tokio::test]
    async fn a_queried_list_the_descriptor_does_not_declare_is_refused_without_a_request() {
        // The port's rule, at the layer that decides it: an adapter that has said it cannot serve
        // trending must refuse both, name itself and the capability, and spend no token finding
        // out. The server below would answer 200 to everything, so `.expect(0)` is what makes this
        // test one about the refusal rather than about the answer.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(json(DISCOVER_PAGE))
            .expect(0)
            .mount(&server)
            .await;

        let refusing = Tmdb::with_capabilities(
            &server.uri(),
            &server.uri(),
            "test-token".to_owned(),
            Capabilities::EMPTY.with(Capability::Search),
        )
        .expect("the adapter builds");

        let refused = refusing
            .browse(&request(
                TitleKind::Movie,
                1,
                BrowseSort::Popularity,
                None,
                None,
            ))
            .await
            .expect_err("the capability is not declared");
        assert_eq!(
            refused,
            ProviderError::Unsupported {
                provider: Provider::Tmdb,
                capability: "trending",
            }
        );

        let refused = refusing
            .genres(TitleKind::Movie)
            .await
            .expect_err("the capability is not declared");
        assert_eq!(
            refused,
            ProviderError::Unsupported {
                provider: Provider::Tmdb,
                capability: "trending",
            }
        );

        let refused = refusing
            .similar(
                &peregrine_domain::ExternalId::new(Provider::Tmdb, "603").expect("a fixture id"),
                TitleKind::Movie,
            )
            .await
            .expect_err("the capability is not declared");
        assert_eq!(
            refused,
            ProviderError::Unsupported {
                provider: Provider::Tmdb,
                capability: "trending",
            }
        );
    }

    #[tokio::test]
    async fn a_film_s_franchise_leads_the_recommendations_in_release_order() {
        // The behaviour a reader judges this rail by, and the one no similarity score can be trusted
        // to produce. The title is *Avatar: Fire and Ash* — a new release, which is exactly the case
        // TMDB's staff warn the rating-based endpoint is weakest at — and the rail must lead with the
        // other *Avatar* films, oldest first, never repeating the film that was opened, with whatever
        // the provider merely considers similar after them.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/movie/83533"))
            .respond_with(json(
                r#"{"id": 83533, "belongs_to_collection": {"id": 87096}}"#,
            ))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/collection/87096"))
            .respond_with(json(
                // **`name`, not `title`.** A collection's parts are the same objects a list endpoint
                // carries, but TMDB spells a film's name `title` on `/discover` and `name` here, and
                // a fixture that only ever wrote `title` would pass while every real collection
                // answered nothing. One part keeps the other spelling so both stay pinned.
                r#"{"id": 87096, "parts": [
                    {"id": 76600, "name": "Avatar: The Way of Water", "release_date": "2022-12-14"},
                    {"id": 19995, "name": "Avatar", "release_date": "2009-12-15"},
                    {"id": 83533, "title": "Avatar: Fire and Ash", "release_date": "2025-12-17"}
                ]}"#,
            ))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/movie/83533/recommendations"))
            .respond_with(json(
                r#"{"page": 1, "total_pages": 1, "results": [
                    {"id": 19995, "title": "Avatar", "release_date": "2009-12-15"},
                    {"id": 157336, "title": "Interstellar", "release_date": "2014-11-05",
                     "genre_ids": [12, 878]}
                ]}"#,
            ))
            .expect(1)
            .mount(&server)
            .await;
        // Not reached: recommendations answered, so the fallback stays unasked.
        Mock::given(method("GET"))
            .and(path("/movie/83533/similar"))
            .respond_with(json(r#"{"results": []}"#))
            .expect(0)
            .mount(&server)
            .await;

        let found = adapter(&server)
            .similar(
                &peregrine_domain::ExternalId::new(Provider::Tmdb, "83533").expect("a fixture id"),
                TitleKind::Movie,
            )
            .await
            .expect("the rail answers");

        let names: Vec<&str> = found.iter().map(|one| one.name().as_str()).collect();
        assert_eq!(
            names,
            vec!["Avatar", "Avatar: The Way of Water", "Interstellar"],
            "the franchise leads oldest-first, the opened film is not repeated, and the provider's \
             own list follows: {names:?}"
        );
        assert_eq!(
            found[2].genre_ids(),
            [12, 878],
            "a list answer's genres ride the candidate, because they are what a caller asks for more of"
        );
    }

    #[tokio::test]
    async fn a_film_in_no_collection_is_answered_with_the_provider_s_list_alone() {
        // The ordinary case: most films are in no franchise, and a film that is in none costs the
        // collection request and gets nothing back from it rather than failing.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/movie/603"))
            .respond_with(json(r#"{"id": 603}"#))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/movie/603/recommendations"))
            .respond_with(json(
                r#"{"page": 1, "total_pages": 1, "results": [
                    {"id": 604, "title": "The Matrix Reloaded", "release_date": "2003-05-15",
                     "poster_path": "/reloaded.jpg", "vote_average": 7.1}
                ]}"#,
            ))
            .expect(1)
            .mount(&server)
            .await;

        let found = adapter(&server)
            .similar(
                &peregrine_domain::ExternalId::new(Provider::Tmdb, "603").expect("a fixture id"),
                TitleKind::Movie,
            )
            .await
            .expect("the rail answers");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name().as_str(), "The Matrix Reloaded");
        assert_eq!(
            found[0].artwork_path(),
            Some("/reloaded.jpg"),
            "a list answer's poster rides the candidate, as it does for a rail"
        );
    }

    #[tokio::test]
    async fn an_empty_recommendation_list_falls_back_to_the_similar_endpoint() {
        // TMDB's own staff name this case: `/recommendations` "is still not perfect, especially for new
        // releases", and a release nobody has rated alongside anything answers it empty. An empty rail
        // is worse than a mediocre one, so the older endpoint is asked instead.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/movie/83533"))
            .respond_with(json(r#"{"id": 83533}"#))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/movie/83533/recommendations"))
            .respond_with(json(r#"{"page": 1, "total_pages": 0, "results": []}"#))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/movie/83533/similar"))
            .respond_with(json(
                r#"{"results": [{"id": 19995, "title": "Avatar", "release_date": "2009-12-15"}]}"#,
            ))
            .expect(1)
            .mount(&server)
            .await;

        let found = adapter(&server)
            .similar(
                &peregrine_domain::ExternalId::new(Provider::Tmdb, "83533").expect("a fixture id"),
                TitleKind::Movie,
            )
            .await
            .expect("the rail answers");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name().as_str(), "Avatar");
    }

    #[tokio::test]
    async fn a_series_asks_no_collection_for_itself_and_reads_the_series_paths() {
        // One path per kind for the reason `title` gives, and no franchise lookup at all: TMDB models
        // collections for films only, so a series that asked for one would spend a request to be told
        // what the type system already says.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/tv/1396/recommendations"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(json(
                r#"{"page": 1, "total_pages": 1, "results": [
                    {"id": 60059, "name": "Better Call Saul", "first_air_date": "2015-02-08"}
                ]}"#,
            ))
            .expect(1)
            .mount(&server)
            .await;
        // Any `/movie/...` at all, which is what a series must not ask for.
        Mock::given(method("GET"))
            .and(path_regex("^/movie/.*"))
            .respond_with(json(r#"{"id": 1}"#))
            .expect(0)
            .mount(&server)
            .await;

        let found = adapter(&server)
            .similar(
                &peregrine_domain::ExternalId::new(Provider::Tmdb, "1396").expect("a fixture id"),
                TitleKind::Show,
            )
            .await
            .expect("the rail answers");

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name().as_str(), "Better Call Saul");
        assert_eq!(found[0].kind(), TitleKind::Show);
    }

    #[tokio::test]
    async fn a_browse_page_at_the_provider_s_last_page_says_there_is_no_more() {
        // TMDB serves 500 pages and refuses 501 — `status_code: 22, "Invalid page: Pages start at 1
        // and max at 500"` — while still reporting the true `total_pages` in the tens of thousands.
        // Believing that number is what turned a deep scroll into an error page.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/discover/movie"))
            .and(query_param("page", "500"))
            .respond_with(json(
                r#"{"page": 500, "total_pages": 43702, "results": []}"#,
            ))
            .mount(&server)
            .await;

        let page = adapter(&server)
            .browse(&request(
                TitleKind::Movie,
                500,
                BrowseSort::Popularity,
                None,
                None,
            ))
            .await
            .expect("the page answers");

        assert!(
            !page.has_more(),
            "the provider's last servable page is the end, whatever its total says"
        );

        // And one page earlier, more is still promised.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/discover/movie"))
            .and(query_param("page", "499"))
            .respond_with(json(
                r#"{"page": 499, "total_pages": 43702, "results": []}"#,
            ))
            .mount(&server)
            .await;
        let page = adapter(&server)
            .browse(&request(
                TitleKind::Movie,
                499,
                BrowseSort::Popularity,
                None,
                None,
            ))
            .await
            .expect("the page answers");
        assert!(page.has_more());
    }

    /// A series' `/tv/{id}` answer, with the season shapes a switcher draws.
    ///
    /// The entry with no number is the one worth naming: `season.season_number` is half of the unique
    /// key a season is stored under, so an entry without one has no identity to write and is skipped
    /// rather than stored under a number this build invented.
    const SEASONS: &str = r#"{
      "id": 1396,
      "name": "Breaking Bad",
      "seasons": [
        {"season_number": 1, "name": "Season 1", "overview": "Chemistry.", "air_date": "2008-01-20",
         "poster_path": "/s1.jpg", "episode_count": 7},
        {"season_number": 0, "name": "Specials", "poster_path": "   ", "episode_count": 0},
        {"season_number": 2, "name": "Season 2", "air_date": "", "poster_path": "/s2.jpg"},
        {"name": "No number"}
      ]
    }"#;

    /// One season's `/tv/{id}/season/{n}` answer, with every field a card reads and the ways TMDB
    /// spells "nothing".
    const EPISODES: &str = r#"{
      "season_number": 1,
      "episodes": [
        {"season_number": 1, "episode_number": 1, "name": "Pilot", "overview": "A diagnosis.",
         "air_date": "2008-01-20", "runtime": 58, "still_path": "/s1e1.jpg",
         "vote_average": 8.2, "vote_count": 3000, "episode_type": "premiere"},
        {"season_number": 1, "episode_number": 2, "name": "Cat's in the Bag", "air_date": "",
         "still_path": "   ", "runtime": 48, "vote_average": 0, "vote_count": 0},
        {"season_number": 1, "name": "No number"},
        {"season_number": 1, "episode_number": 7, "name": "Finale", "episode_type": "finale",
         "vote_average": 0, "vote_count": 12}
      ]
    }"#;

    fn show() -> peregrine_domain::ExternalId {
        peregrine_domain::ExternalId::new(Provider::Tmdb, "1396").expect("a fixture id")
    }

    fn season(number: u32) -> peregrine_domain::catalogue::SeasonNumber {
        peregrine_domain::catalogue::SeasonNumber::new(number)
    }

    #[tokio::test]
    async fn a_series_season_list_maps_what_a_switcher_draws_and_skips_one_with_no_number() {
        // The four fields a season summary has, and the two absences TMDB writes differently: a path of
        // whitespace and an empty air date are both "nothing", exactly as a list answer's are.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/tv/1396"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(json(SEASONS))
            .expect(1)
            .mount(&server)
            .await;

        let seasons = adapter(&server)
            .seasons(&show(), TitleKind::Show)
            .await
            .expect("the season list answers");

        assert_eq!(seasons.len(), 3, "the entry with no number is not a season");
        assert_eq!(seasons[0].season_number, season(1));
        assert_eq!(seasons[0].name.as_deref(), Some("Season 1"));
        assert_eq!(seasons[0].overview.as_deref(), Some("Chemistry."));
        assert_eq!(
            seasons[0].air_date,
            Some(jiff::civil::Date::constant(2008, 1, 20))
        );
        assert_eq!(seasons[0].poster_path.as_deref(), Some("/s1.jpg"));
        assert_eq!(seasons[0].episode_count, Some(7));
        assert_eq!(
            seasons[1].season_number,
            season(0),
            "season zero is a season like any other, and the specials flag is the store's"
        );
        assert_eq!(
            seasons[1].poster_path, None,
            "a whitespace path is an absent path"
        );
        assert_eq!(seasons[1].episode_count, Some(0));
        assert_eq!(seasons[2].air_date, None, "an empty date is an absent date");
        assert_eq!(
            seasons[2].episode_count, None,
            "a season the answer did not count is not a season with none"
        );
    }

    #[tokio::test]
    async fn a_seasons_episodes_map_every_field_a_card_draws_and_an_unrated_one_maps_none() {
        // The still, the air date, the runtime, the synopsis, the score, and the episode type: what the
        // title page's episode cards read (`PG-E7-13`), and the four ways TMDB says "nothing" about
        // them. **The zero-with-no-votes is the one worth pinning** — a `0.0` badge on an episode
        // nobody has rated is a claim the provider never made, and the same zero with votes behind it
        // is a real score.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/tv/1396/season/1"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(json(EPISODES))
            .expect(1)
            .mount(&server)
            .await;

        let episodes = adapter(&server)
            .season_episodes(&show(), TitleKind::Show, season(1))
            .await
            .expect("the season's episodes answer");

        assert_eq!(
            episodes.len(),
            3,
            "the entry with no number is not an episode"
        );
        assert_eq!(episodes[0].episode_number.get(), 1);
        assert_eq!(episodes[0].season_number, season(1));
        assert_eq!(episodes[0].name.as_deref(), Some("Pilot"));
        assert_eq!(episodes[0].overview.as_deref(), Some("A diagnosis."));
        assert_eq!(
            episodes[0].air_date,
            Some(jiff::civil::Date::constant(2008, 1, 20))
        );
        assert_eq!(episodes[0].runtime_minutes, Some(58));
        assert_eq!(episodes[0].still_path.as_deref(), Some("/s1e1.jpg"));
        assert_eq!(episodes[0].rating, Some(8.2));
        assert_eq!(episodes[0].votes, Some(3000));
        assert_eq!(
            episodes[0].episode_type,
            Some(peregrine_domain::catalogue::EpisodeType::Premiere)
        );
        assert_eq!(episodes[1].name.as_deref(), Some("Cat's in the Bag"));
        assert_eq!(episodes[1].air_date, None);
        assert_eq!(episodes[1].still_path, None, "a whitespace still is absent");
        assert_eq!(
            episodes[1].runtime_minutes,
            Some(48),
            "an episode with no date and no still is still an episode"
        );
        assert_eq!(episodes[1].rating, None, "a zero with no votes is unrated");
        assert_eq!(
            episodes[1].votes, None,
            "nothing counted it, so there is no count"
        );
        assert_eq!(episodes[1].episode_type, None);
        assert_eq!(episodes[2].episode_number.get(), 7);
        assert_eq!(
            episodes[2].rating,
            Some(0.0),
            "a zero the provider counted is a score"
        );
        assert_eq!(episodes[2].votes, Some(12));
        assert_eq!(
            episodes[2].episode_type,
            Some(peregrine_domain::catalogue::EpisodeType::Finale)
        );
    }

    #[tokio::test]
    async fn a_season_with_no_episodes_answers_an_empty_list_rather_than_a_refusal() {
        // A season announced with nothing in it yet is a real statement about a catalogue, and the two
        // halves of the title page draw it differently from "the provider would not answer".
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/tv/1396/season/3"))
            .respond_with(json(r#"{"season_number": 3, "episodes": []}"#))
            .expect(1)
            .mount(&server)
            .await;

        let episodes = adapter(&server)
            .season_episodes(&show(), TitleKind::Show, season(3))
            .await
            .expect("an empty season is an answer");

        assert_eq!(episodes, Vec::new());
    }

    #[tokio::test]
    async fn a_film_is_refused_a_season_read_without_spending_a_request() {
        // A film has no seasons, so an empty list would be this adapter claiming a fact about a
        // catalogue rather than refusing a question it cannot be asked. `expect(0)` on a catch-all is
        // what proves nothing was asked: `TMDB` numbers films and series in one id space, so asking
        // the wrong endpoint is a request that could plausibly succeed and return nonsense.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(json(r#"{"seasons": []}"#))
            .expect(0)
            .mount(&server)
            .await;
        let film = peregrine_domain::ExternalId::new(Provider::Tmdb, "603").expect("a fixture id");

        for refused in [
            adapter(&server)
                .seasons(&film, TitleKind::Movie)
                .await
                .err(),
            adapter(&server)
                .season_episodes(&film, TitleKind::Movie, season(1))
                .await
                .err(),
        ] {
            assert_eq!(
                refused,
                Some(ProviderError::Unsupported {
                    provider: Provider::Tmdb,
                    capability: "get_season",
                })
            );
        }
    }

    #[tokio::test]
    async fn a_descriptor_that_does_not_declare_seasons_refuses_without_a_request() {
        // The port's own rule, at the adapter: a provider that has said it cannot serve a capability
        // refuses rather than answering, and it never spends a request finding out. The capability set
        // is only ever narrower than [`Tmdb::new`]'s in a test, which is the one way this path is
        // reachable at all.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(json(SEASONS))
            .expect(0)
            .mount(&server)
            .await;
        let silent = Tmdb::with_capabilities(
            &server.uri(),
            &server.uri(),
            "test-token".to_owned(),
            Capabilities::EMPTY
                .with(Capability::Search)
                .with(Capability::Title),
        )
        .expect("the adapter builds");

        assert_eq!(
            silent.seasons(&show(), TitleKind::Show).await,
            Err(ProviderError::Unsupported {
                provider: Provider::Tmdb,
                capability: "get_season",
            })
        );
    }

    #[tokio::test]
    async fn a_season_the_provider_does_not_hold_is_not_found_rather_than_an_empty_season() {
        // The distinction the sync runs on: a 404 is one season's problem and the others are still
        // written, while an empty episode list is a season nobody has filled in.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/tv/1396/season/9"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let error = adapter(&server)
            .season_episodes(&show(), TitleKind::Show, season(9))
            .await
            .expect_err("the season does not exist");

        assert_eq!(
            error,
            ProviderError::NotFound {
                provider: Provider::Tmdb
            }
        );
    }

    #[tokio::test]
    async fn a_season_answer_that_omits_its_list_is_drift() {
        // `seasons` and `episodes` are required rather than defaulted, and this is the reason: a
        // defaulted missing key would render as a series with no seasons — a lie about a catalogue —
        // where the drift this build reports keeps the row stale and rendering from cache.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/tv/1396"))
            .respond_with(json(r#"{"id": 1396, "name": "Breaking Bad"}"#))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/tv/1396/season/1"))
            .respond_with(json(r#"{"episodes": "not a list"}"#))
            .mount(&server)
            .await;

        assert!(matches!(
            adapter(&server).seasons(&show(), TitleKind::Show).await,
            Err(ProviderError::SchemaDrift { .. })
        ));
        assert!(matches!(
            adapter(&server)
                .season_episodes(&show(), TitleKind::Show, season(1))
                .await,
            Err(ProviderError::SchemaDrift { .. })
        ));
    }

    #[tokio::test]
    async fn a_rate_limited_season_read_is_reported_as_rate_limited_rather_than_retried_for_ever() {
        // `AGENTS R24`'s behaviour at the two new endpoints, and it is inherited rather than
        // reimplemented: both go through the same `HttpClient` as every other read, so a `429` parks
        // the token bucket for what the provider asked and the caller is told the provider refused
        // rather than being handed a transport failure it would retry into a wall.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/tv/1396/season/1"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
            .mount(&server)
            .await;

        let error = adapter(&server)
            .season_episodes(&show(), TitleKind::Show, season(1))
            .await
            .expect_err("the provider is refusing");

        assert!(
            matches!(error, ProviderError::RateLimited { .. }),
            "got {error:?}"
        );
    }

    #[tokio::test]
    async fn the_genre_list_is_read_per_kind_and_an_entry_that_names_nothing_is_skipped() {
        // The two vocabularies are separate endpoints because their ids do not agree, and an entry
        // a control could not draw — no id, or a name that is only whitespace — is skipped rather
        // than offered as a filter with no label.
        for (kind, endpoint) in [
            (TitleKind::Movie, "/genre/movie/list"),
            (TitleKind::Show, "/genre/tv/list"),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path(endpoint))
                .and(header("authorization", "Bearer test-token"))
                .respond_with(json(
                    r#"{"genres": [
                        {"id": 28, "name": "Action"},
                        {"id": 35, "name": "  Comedy  "},
                        {"id": 99, "name": "   "},
                        {"name": "No id"}
                    ]}"#,
                ))
                .expect(1)
                .mount(&server)
                .await;

            let genres = adapter(&server)
                .genres(kind)
                .await
                .unwrap_or_else(|error| panic!("{kind:?} answered {error:?}"));

            assert_eq!(
                genres.len(),
                2,
                "{kind:?}: a blank name and a missing id are not genres to draw"
            );
            assert_eq!(genres[0].id(), 28);
            assert_eq!(genres[0].name().as_str(), "Action");
            assert_eq!(
                genres[1].name().as_str(),
                "Comedy",
                "a name is trimmed before it is offered"
            );
        }
    }

    #[tokio::test]
    async fn credits_are_mapped_with_their_characters_and_blank_names_are_skipped() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/movie/603/credits"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(json(
                r#"{"cast": [
                    {"id": 6384, "name": "  Keanu Reeves ", "character": " Neo ", "order": 0,
                     "profile_path": "/keanu.jpg"},
                    {"id": 9, "name": "   ", "character": "Nobody", "order": 1},
                    {"id": 2, "name": "Carrie-Anne Moss", "character": "   ", "order": 2,
                     "profile_path": ""}
                ]}"#,
            ))
            .expect(1)
            .mount(&server)
            .await;

        let id = peregrine_domain::ExternalId::new(Provider::Tmdb, "603").expect("a provider id");
        let credits = adapter(&server)
            .credits(&id, TitleKind::Movie)
            .await
            .expect("the credits answer");

        assert_eq!(credits.cast.len(), 2, "a person with no name is not cast");
        assert_eq!(credits.cast[0].name, "Keanu Reeves", "a name is trimmed");
        assert_eq!(credits.cast[0].character.as_deref(), Some("Neo"));
        assert_eq!(credits.cast[0].order, Some(0));
        assert_eq!(credits.cast[0].profile_path.as_deref(), Some("/keanu.jpg"));
        assert_eq!(
            credits.cast[1].character, None,
            "a whitespace character is no character"
        );
        assert_eq!(
            credits.cast[1].profile_path, None,
            "an empty path is no portrait"
        );
    }

    #[tokio::test]
    async fn a_cast_filter_asks_the_provider_for_that_person() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/discover/movie"))
            .and(query_param("with_cast", "6384"))
            .respond_with(json(r#"{"page":1,"results":[],"total_pages":1}"#))
            .expect(1)
            .mount(&server)
            .await;

        let request = BrowseRequest::new(
            TitleKind::Movie,
            NonZeroU32::new(1).expect("a page"),
            BrowseSort::Popularity,
            None,
            None,
        )
        .with_cast(Some("6384".to_owned()));

        adapter(&server)
            .browse(&request)
            .await
            .expect("the filmography page answers");
    }
}
