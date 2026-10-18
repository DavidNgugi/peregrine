//! The `MetadataProvider` port.
//!
//! One trait, implemented by every metadata adapter, and read by everything that needs to
//! know what a provider is before it asks for anything. The registry holds them
//! (`Vec<Arc<dyn MetadataProvider>>`, PG-E5-12), the token bucket is built from the rate the
//! descriptor declares (PG-E5-03), and the diagnostics screen lists what each one says about
//! itself (PG-E5-22).
//!
//! # Why the signature is a boxed future
//!
//! `search` reaches the network, so it is asynchronous, and the port is `dyn`-compatible
//! because a registry of providers is heterogeneous — TMDB, TVDB, and `OMDb` behind one list,
//! composed from settings rather than from a `match`. On this toolchain an `async fn` in a
//! trait rules `dyn` out (`error[E0038]: the trait is not dyn compatible`), and a concrete
//! return type in its place would make every provider's future a different type. So the
//! method returns a boxed future through [`ProviderFuture`], which costs one allocation per
//! network request and buys object safety without a proc-macro dependency.
//!
//! # Why the port's operations are a subset of the eight capabilities
//!
//! `ARCHITECTURE` §13 lists eight capabilities. `search` was the first, and it is the one whose
//! request and response the flows fixed early — FLOWS F3 has the sync service call `search` and get
//! candidates back to upsert as stub titles. `title` (PG-E5-07) and `image` (PG-E5-15) followed as
//! their flows arrived, `rail` followed with the Discover screen's rails (PG-E7-08): it answers the
//! same [`SearchCandidate`] a search does, which is the whole reason it needed no new type, and
//! `seasons`/`season_episodes` followed with the title page's season switcher and episode cards
//! (PG-E5-06, PG-E7-13). The rest return *drafts* of the catalogue entities, which carry no row id and
//! therefore cannot be the domain's stored types; those drafts are the normal form, and the normal form
//! is designed against real provider payloads in PG-E5-06 to PG-E5-16 rather than guessed at here from
//! the providers' documentation.
//!
//! [`MetadataProvider::check`] is the one operation that is **not** one of those eight, and it is here
//! because it belongs to no single capability: it asks whether the provider can be used at all. A
//! credential has to be known to work *before* it is stored (PG-E5-20), and an adapter is the only
//! thing that knows which of its endpoints answers that question most cheaply.

use std::future::Future;
use std::num::NonZeroU32;
use std::pin::Pin;

use jiff::civil::Date;

use peregrine_domain::catalogue::{EpisodeDetails, SeasonDetails, SeasonNumber, TitleKind};
use peregrine_domain::id::ExternalId;
use peregrine_domain::text::NonEmptyText;

use crate::descriptor::ProviderDescriptor;
use crate::error::ProviderError;

/// The most candidates one rail page may answer with.
///
/// A rail takes no limit argument — a home screen draws a page and nothing else — so "no more than
/// a page" needs a number somewhere or it is not a claim anything can check. This is it: the
/// conformance suite refuses a rail answer larger than this, and an adapter whose provider pages
/// larger than this truncates. Twenty is the bound the first adapter's lists arrive in
/// (`PG-E5-06`), which is what makes it a *page* rather than a guess, and it is deliberately a
/// constant rather than a parameter: a caller that wants fewer items draws fewer.
pub const RAIL_PAGE_LIMIT: usize = 20;

/// One named list a provider publishes, as a *home screen* names it.
///
/// # Why the vocabulary stops at "what a home screen can show"
///
/// The obvious alternative is to mirror the provider's own catalogue of paths — TMDB alone has
/// `/trending/movie/week`, `/movie/popular`, `/movie/top_rated`, `/movie/upcoming`,
/// `/tv/popular`, `/tv/on_the_air`, `/tv/top_rated`, and a `discover` endpoint with a filter set
/// large enough to be its own task (`PG-E5-06`). A port that copied those names would make every
/// other provider answer in TMDB's vocabulary: TVDB has no `/movie/upcoming`, and a second
/// metadata source would spend its adapter mapping its own lists onto names it did not choose.
///
/// What a rail *means* — the week's most watched, the most popular, the best reviewed, the soonest
/// to arrive, the ones on air now — is a question a person asks of any provider, and each adapter
/// answers it with whatever endpoint (or composition of endpoints) it has. An endpoint this
/// vocabulary cannot express is an endpoint no home screen has a place for, and growing it is a
/// deliberate act with a place to put the list.
///
/// # What a rail is not
///
/// A rail is not a search and not a filter: it takes no text and no arguments beyond the page, it
/// is ordered by the provider rather than by the caller, and the answer is the same stub a search
/// answers with (see [`SearchCandidate`]) because the sync layer already knows how to upsert one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum RailKind {
    /// What the provider's audience watched most this week, films and series together.
    TrendingWeek,
    /// The most popular films.
    PopularMovies,
    /// The best reviewed films.
    TopRatedMovies,
    /// Films not yet released.
    UpcomingMovies,
    /// The most popular series.
    PopularShows,
    /// Series with an episode airing around now.
    OnTheAirShows,
    /// The best reviewed series.
    TopRatedShows,
}

impl RailKind {
    /// Every rail, in the order a home screen shows them.
    pub const ALL: [Self; 7] = [
        Self::TrendingWeek,
        Self::PopularMovies,
        Self::TopRatedMovies,
        Self::UpcomingMovies,
        Self::PopularShows,
        Self::OnTheAirShows,
        Self::TopRatedShows,
    ];

    /// The stable identifier a rail carries across IPC, logs, and a journal payload.
    ///
    /// It is deliberately not the heading: a heading is copy and belongs in the catalogue of
    /// translations (`packages/i18n`), so renaming "On the air" is a copy change rather than a
    /// change to what a caller asks for.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TrendingWeek => "trending_week",
            Self::PopularMovies => "popular_movies",
            Self::TopRatedMovies => "top_rated_movies",
            Self::UpcomingMovies => "upcoming_movies",
            Self::PopularShows => "popular_shows",
            Self::OnTheAirShows => "on_the_air_shows",
            Self::TopRatedShows => "top_rated_shows",
        }
    }

    /// The kind every item of this rail is, or `None` for a rail that mixes the two.
    ///
    /// It is what lets an adapter tell a provider which endpoint to read — a `movie/popular` list
    /// cannot answer a series — and it is `None` for exactly the rails whose answer carries its own
    /// media type, which is why the adapter and not the caller decides what an item is.
    #[must_use]
    pub const fn kind(self) -> Option<TitleKind> {
        match self {
            Self::TrendingWeek => None,
            Self::PopularMovies | Self::TopRatedMovies | Self::UpcomingMovies => {
                Some(TitleKind::Movie)
            }
            Self::PopularShows | Self::OnTheAirShows | Self::TopRatedShows => Some(TitleKind::Show),
        }
    }

    /// Parses the name [`RailKind::as_str`] writes, or `None` for one this build does not know.
    ///
    /// The other half of a stored vocabulary: a rail's name is written down — in a manifest, a
    /// journal payload, an IPC call — and reading it back has to be the same decision as writing
    /// it, rather than a second list of names somewhere else that can drift from this one.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        // A `match` on the literal rather than a scan of `ALL`: the order of `ALL` is the order a
        // screen shows the rails in, and a reader here should not have to check that changing it
        // cannot change what a stored name means.
        match value {
            "trending_week" => Some(Self::TrendingWeek),
            "popular_movies" => Some(Self::PopularMovies),
            "top_rated_movies" => Some(Self::TopRatedMovies),
            "upcoming_movies" => Some(Self::UpcomingMovies),
            "popular_shows" => Some(Self::PopularShows),
            "on_the_air_shows" => Some(Self::OnTheAirShows),
            "top_rated_shows" => Some(Self::TopRatedShows),
            _ => None,
        }
    }
}

/// How a browse page is ordered.
///
/// The vocabulary is a person's — the most popular, the best reviewed, the newest, the oldest —
/// rather than a provider's sort parameter, for the reason [`RailKind`] states about named lists:
/// TMDB spells these `popularity.desc`, `vote_average.desc`, `primary_release_date.desc`, and
/// `primary_release_date.asc`, and a port that copied those names would make every other provider
/// answer in TMDB's vocabulary.
///
/// **Popularity is the default because it is the only order every provider means the same thing
/// by.** A score is a provider's own scale (see [`SearchCandidate::rating`]), and a date is missing
/// for anything unreleased or undated, so both are orders a caller chooses deliberately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
#[repr(u8)]
pub enum BrowseSort {
    /// What the provider's audience is watching, most first.
    #[default]
    Popularity,
    /// The best reviewed first.
    Rating,
    /// The most recently released first.
    Newest,
    /// The oldest first.
    Oldest,
}

impl BrowseSort {
    /// Every order, in the order a filter control offers them.
    pub const ALL: [Self; 4] = [Self::Popularity, Self::Rating, Self::Newest, Self::Oldest];

    /// The stable identifier an order carries across IPC, a log line, and a saved filter.
    ///
    /// It is deliberately not the control's label: a label is copy and belongs in the catalogue of
    /// translations (`packages/i18n`), so renaming "Newest" is a copy change rather than a change
    /// to what a caller asks for.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Popularity => "popularity",
            Self::Rating => "rating",
            Self::Newest => "newest",
            Self::Oldest => "oldest",
        }
    }

    /// Parses the name [`BrowseSort::as_str`] writes, or `None` for one this build does not know.
    ///
    /// The other half of a stored vocabulary: an order is written down — in a URL, a saved smart
    /// collection, an IPC call — and reading it back has to be the same decision as writing it.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "popularity" => Some(Self::Popularity),
            "rating" => Some(Self::Rating),
            "newest" => Some(Self::Newest),
            "oldest" => Some(Self::Oldest),
            _ => None,
        }
    }
}

/// What one browse page asks a provider for.
///
/// **A queried list, not a fixed one.** [`RailKind`] is the vocabulary of the *named* lists a home
/// screen draws; this is the vocabulary of a list a caller composes — which kind, how ordered, and
/// the two filters a person reaches for first, a genre and a floor under the score. It exists
/// because a rail takes no arguments, so "the action films from the eighties, best reviewed first"
/// is a question the port could not previously ask.
///
/// The page is a [`NonZeroU32`] for the reason [`MetadataProvider::rail`] gives: a provider's lists
/// are paged from one, so "the zeroth page" is unrepresentable rather than refused. The genre is a
/// *provider's own* identifier, which is what its filter takes; it comes back from
/// [`MetadataProvider::genres`] and never from a screen. The floor is a whole score out of ten,
/// which is the scale every filter control this application draws uses; an adapter whose provider
/// scores out of five maps it, and the port normalizes nothing (see [`SearchCandidate::rating`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowseRequest {
    kind: TitleKind,
    page: NonZeroU32,
    sort: BrowseSort,
    genre: Option<u32>,
    min_rating: Option<u8>,
    /// The provider's id for one person, when the page is a filmography rather than a catalogue.
    cast: Option<String>,
}

impl BrowseRequest {
    /// Builds a page request: `kind`'s titles, ordered by `sort`, optionally within one of the
    /// provider's genres and optionally floored at `min_rating` out of ten.
    #[must_use]
    pub const fn new(
        kind: TitleKind,
        page: NonZeroU32,
        sort: BrowseSort,
        genre: Option<u32>,
        min_rating: Option<u8>,
    ) -> Self {
        Self {
            kind,
            page,
            sort,
            genre,
            min_rating,
            cast: None,
        }
    }

    /// Narrows the page to one person's credits.
    ///
    /// The id is the provider's own identifier for the person, not a name: a name would be a search
    /// with a filter applied after it, and the point of the filter is that the provider does it.
    #[must_use]
    pub fn with_cast(mut self, cast: Option<String>) -> Self {
        self.cast = cast.filter(|value| !value.trim().is_empty());
        self
    }

    /// The person's provider id, when the page is narrowed to one.
    #[must_use]
    pub fn cast(&self) -> Option<&str> {
        self.cast.as_deref()
    }

    /// Film or series, which is a fact about the endpoint rather than a filter over the answer.
    #[must_use]
    pub const fn kind(&self) -> TitleKind {
        self.kind
    }

    /// Which page of the list to read, counting from one.
    #[must_use]
    pub const fn page(&self) -> NonZeroU32 {
        self.page
    }

    /// How the provider is asked to order the page.
    #[must_use]
    pub const fn sort(&self) -> BrowseSort {
        self.sort
    }

    /// The provider's own identifier for the genre to restrict to, when the caller picked one.
    #[must_use]
    pub const fn genre(&self) -> Option<u32> {
        self.genre
    }

    /// The lowest score, out of ten, a title may have to appear, when the caller set a floor.
    #[must_use]
    pub const fn min_rating(&self) -> Option<u8> {
        self.min_rating
    }
}

/// One genre a provider filters by, as the provider names it.
///
/// It is *not* [`peregrine_domain::catalogue::Genre`]: that type carries the `i64` row id
/// `peregrine-storage` assigns, and a provider's identifier is not a row. The id is what a
/// [`BrowseRequest`] hands back to the provider, and the name is what a filter control draws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenreRef {
    id: u32,
    name: NonEmptyText,
}

impl GenreRef {
    /// A genre the provider identified by `id` and named `name`.
    #[must_use]
    pub const fn new(id: u32, name: NonEmptyText) -> Self {
        Self { id, name }
    }

    /// The provider's own identifier, which is what its browse filter takes.
    #[must_use]
    pub const fn id(&self) -> u32 {
        self.id
    }

    /// The name a filter control draws.
    #[must_use]
    pub const fn name(&self) -> &NonEmptyText {
        &self.name
    }
}

/// One page of a provider's list, with whether another page follows it.
///
/// **A bare list cannot answer "is there more".** A page that carried only its items would leave a
/// screen to guess from the item count, and the two guesses are both wrong: a full-looking page
/// that happens to be the last one sends the reader to a page past the provider's end on every
/// scroll, and a short final page is indistinguishable from a truncation. TMDB answers
/// `total_pages` and `page`; this is where that answer travels, so the decision belongs to the
/// adapter that read it rather than to a screen counting tiles (`AGENTS R7`).
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderPage {
    items: Vec<SearchCandidate>,
    has_more: bool,
}

impl ProviderPage {
    /// A page holding `items`, with `has_more` saying whether a further page exists.
    #[must_use]
    pub const fn new(items: Vec<SearchCandidate>, has_more: bool) -> Self {
        Self { items, has_more }
    }

    /// A page with nothing on it and nothing after it — an empty answer, not a failure.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            items: Vec::new(),
            has_more: false,
        }
    }

    /// The candidates this page holds, in the provider's order.
    #[must_use]
    pub fn items(&self) -> &[SearchCandidate] {
        &self.items
    }

    /// The same, owned, for a caller that is done with the page.
    #[must_use]
    pub fn into_items(self) -> Vec<SearchCandidate> {
        self.items
    }

    /// Whether asking for the next page would find anything.
    #[must_use]
    pub const fn has_more(&self) -> bool {
        self.has_more
    }
}

/// What a provider operation answers with, as a future the caller can await.
///
/// Named rather than written out because every operation returns one, and a signature that
/// spelled `Pin<Box<dyn Future<Output = Result<…>> + Send + 'a>>` at each of them would be a
/// wall of punctuation around the part that matters.
pub type ProviderFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ProviderError>> + Send + 'a>>;

/// What a caller asks a provider to look for.
///
/// The text is a [`NonEmptyText`] and the limit a [`NonZeroU32`], so the two ways a search
/// request can be meaningless — an empty query and a request for nothing — are unrepresentable
/// rather than checked for. The kind is optional because a provider that can search both
/// films and series takes one endpoint per kind, and a caller that does not care asks both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    text: NonEmptyText,
    limit: NonZeroU32,
    kind: Option<TitleKind>,
}

impl SearchQuery {
    /// Builds a query for `text`, asking for at most `limit` candidates.
    #[must_use]
    pub const fn new(text: NonEmptyText, limit: NonZeroU32, kind: Option<TitleKind>) -> Self {
        Self { text, limit, kind }
    }

    /// What to look for.
    #[must_use]
    pub const fn text(&self) -> &NonEmptyText {
        &self.text
    }

    /// The most candidates to return.
    #[must_use]
    pub const fn limit(&self) -> NonZeroU32 {
        self.limit
    }

    /// The kind to restrict to, when the caller cares.
    #[must_use]
    pub const fn kind(&self) -> Option<TitleKind> {
        self.kind
    }
}

/// A title a search matched, as the provider identifies it.
///
/// A candidate is a *stub*: enough to list, to choose between two same-named titles, and to
/// re-ask for in full. It is not a [`peregrine_domain::catalogue::Title`] and cannot be — that
/// type carries the `i64` row id `peregrine-storage` assigns — so the sync service upserts
/// these as stub rows and a later `get_title` fills them in (PG-E5-06, PG-E6-01).
///
/// # What a *list* answer carries that a stub did not
///
/// A provider's list endpoints — a rail, a search page — already answer with a poster, a backdrop, a
/// score, a synopsis, and a vote count for every item, so a candidate carries all five
/// ([`SearchCandidate::artwork_path`], [`SearchCandidate::backdrop_path`],
/// [`SearchCandidate::rating`], [`SearchCandidate::overview`], and [`SearchCandidate::votes`]). That
/// is what keeps filling a rail of posters from costing a second request per title (`PG-E7-08`), and
/// it is why the fields belong here rather than on
/// [`peregrine_domain::catalogue::TitleDetails`], which is what a *details* read answers.
///
/// All five are optional, and a provider that sends none of them still produces a candidate: not
/// every list carries artwork or prose, and a provider's own field names never appear on this type.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchCandidate {
    external_id: ExternalId,
    name: NonEmptyText,
    kind: TitleKind,
    released: Option<Date>,
    artwork_path: Option<String>,
    backdrop_path: Option<String>,
    rating: Option<f64>,
    overview: Option<String>,
    votes: Option<u64>,
    /// The provider's own identifiers for this title's genres, as a *list* answer spells them.
    ///
    /// **Empty means "the answer did not say", not "this title has no genres".** A details read spells
    /// genres as objects with names rather than as a list of ids, so a candidate built from one carries
    /// none; only the list endpoints send `genre_ids`. A caller must therefore read an empty vector as
    /// absent evidence — see `peregrine-app`'s recommendation read, which is careful about exactly that.
    genre_ids: Vec<u32>,
}

impl SearchCandidate {
    /// Builds a candidate that carries only what every provider's list carries.
    ///
    /// The artwork, the rating, the synopsis, the vote count, and the genres are set with
    /// [`SearchCandidate::with_artwork_path`], [`SearchCandidate::with_backdrop_path`],
    /// [`SearchCandidate::with_rating`], [`SearchCandidate::with_overview`],
    /// [`SearchCandidate::with_votes`], and [`SearchCandidate::with_genre_ids`], because a provider
    /// that sends none of them is the rule rather than the exception and a ten-argument constructor
    /// would make every call site state five `None`s it does not mean.
    #[must_use]
    pub const fn new(
        external_id: ExternalId,
        name: NonEmptyText,
        kind: TitleKind,
        released: Option<Date>,
    ) -> Self {
        Self {
            external_id,
            name,
            kind,
            released,
            artwork_path: None,
            backdrop_path: None,
            rating: None,
            overview: None,
            votes: None,
            genre_ids: Vec::new(),
        }
    }

    /// The same candidate, carrying the provider-relative path of its poster.
    ///
    /// The path is the provider's own spelling and never a URL: only the adapter knows which host
    /// serves its images (`MetadataProvider::image`). A path that is present but blank is not a
    /// path — a caller that stored it would ask an image host for its root — and the conformance
    /// suite refuses one, so an adapter must map an empty field to `None` rather than to `""`.
    #[must_use]
    pub fn with_artwork_path(mut self, path: Option<String>) -> Self {
        self.artwork_path = path;
        self
    }

    /// The same candidate, carrying the provider-relative path of its backdrop.
    ///
    /// A backdrop is a wide image behind a title rather than a portrait one beside its name, and it
    /// is the second variant every list endpoint that sends a `poster_path` also sends. It travels
    /// on this type for the reason the artwork does — the list answer is where it is free — and it
    /// has an accessor of its own because the two are different pictures served at different sizes:
    /// [`MetadataProvider::image`] decides which variant a path is asked for
    /// ([`peregrine_domain::catalogue::ImageKind`]), and one path cannot stand in for the other.
    ///
    /// A blank or absent field is `None`, for the reason
    /// [`SearchCandidate::with_artwork_path`] states, and the conformance suite asks every adapter
    /// to map a whitespace path to one.
    #[must_use]
    pub fn with_backdrop_path(mut self, path: Option<String>) -> Self {
        self.backdrop_path = path;
        self
    }

    /// The same candidate, carrying the provider's own score for the title.
    ///
    /// The scale is the provider's: this port does not normalize a rating, because a caller that
    /// drew a provider's score against another provider's ceiling would be inventing a number
    /// neither of them sent.
    #[must_use]
    pub fn with_rating(mut self, rating: Option<f64>) -> Self {
        self.rating = rating;
        self
    }

    /// The same candidate, carrying the synopsis the list answer sent.
    ///
    /// A list endpoint that sends an `overview` is sending prose, and prose is what the hover
    /// preview draws inside the enlarged card (`PG-E7-05`). It is *not* a details read: a list
    /// answer's synopsis is the short one, and a later `refresh_title` may replace it.
    ///
    /// An empty or whitespace-only string is not prose, so an adapter maps one to `None` for the
    /// reason [`SearchCandidate::with_artwork_path`] states about a blank path.
    #[must_use]
    pub fn with_overview(mut self, overview: Option<String>) -> Self {
        self.overview = overview;
        self
    }

    /// The same candidate, carrying how many people voted on the title.
    ///
    /// It travels beside [`SearchCandidate::rating`] because a score without its denominator is a
    /// claim nobody can weigh, and because a provider that reports `0` for an item nobody has voted
    /// on is describing an absence rather than a count — an adapter maps that to `None`.
    #[must_use]
    pub const fn with_votes(mut self, votes: Option<u64>) -> Self {
        self.votes = votes;
        self
    }

    /// The same candidate, carrying the genres a *list* answer named it under.
    ///
    /// It is what lets a caller ask a provider for more of what it just recommended: a list answer's
    /// `genre_ids` are the provider's own identifiers, so they go straight back into
    /// [`MetadataProvider::browse`]'s `genre` without a lookup.
    ///
    /// **An empty vector is absent evidence rather than a title with no genres**, for the reason
    /// [`SearchCandidate`]'s field doc gives, and a caller that treated the two as the same would
    /// conclude that every title from a details read belongs to no genre at all.
    #[must_use]
    pub fn with_genre_ids(mut self, genre_ids: Vec<u32>) -> Self {
        self.genre_ids = genre_ids;
        self
    }

    /// The provider's own identifier for the title.
    ///
    /// It must name the provider that answered: a provider returning another provider's id is
    /// the coupling ADR 0007 rule 2 exists to prevent, and it is the one thing this port's
    /// conformance suite can check about a candidate without knowing the provider's fixtures.
    #[must_use]
    pub const fn external_id(&self) -> &ExternalId {
        &self.external_id
    }

    /// The title's name, as the provider writes it.
    #[must_use]
    pub const fn name(&self) -> &NonEmptyText {
        &self.name
    }

    /// Film or series.
    #[must_use]
    pub const fn kind(&self) -> TitleKind {
        self.kind
    }

    /// The release date for a film or the first air date for a series.
    ///
    /// It is what tells a remake from its original when the two share a name, which is the
    /// reconciliation PG-E5-13 has to do, so a candidate that drops it makes that task harder
    /// than the provider made it.
    #[must_use]
    pub const fn released(&self) -> Option<Date> {
        self.released
    }

    /// The provider-relative path of the title's poster, when the answer carried one.
    ///
    /// It is what the cache key is built from (`sha256(provider|path|variant)`), so a caller hands
    /// it back to [`MetadataProvider::image`] rather than building a URL out of it.
    #[must_use]
    pub fn artwork_path(&self) -> Option<&str> {
        self.artwork_path.as_deref()
    }

    /// The provider-relative path of the title's backdrop, when the answer carried one.
    ///
    /// `None` is "the provider sent no wide image", which is a state a screen draws — a poster
    /// cropped to the band, or a reserved box when there is no poster either — rather than a reason
    /// to stretch one, because the two are different pictures and the cache keys them separately.
    #[must_use]
    pub fn backdrop_path(&self) -> Option<&str> {
        self.backdrop_path.as_deref()
    }

    /// The provider's own score for the title, when the answer carried one.
    ///
    /// `None` is not zero: it is "the provider said nothing", and an adapter must map a value its
    /// provider uses for *unrated* to `None` rather than to a score nobody gave.
    #[must_use]
    pub const fn rating(&self) -> Option<f64> {
        self.rating
    }

    /// The synopsis the list answer carried, when it carried one.
    ///
    /// `None` is "the provider said nothing about what this is", which is a state the preview draws
    /// by leaving the line out rather than by inventing a summary (`AGENTS R30` is why this port
    /// does not guess one).
    #[must_use]
    pub fn overview(&self) -> Option<&str> {
        self.overview.as_deref()
    }

    /// How many people voted on the title, when the answer carried a count.
    ///
    /// Zero is never a count: it is what a provider sends for an item nobody has voted on, and an
    /// adapter must map it to `None` rather than to a number that reads as "nobody cares".
    #[must_use]
    pub const fn votes(&self) -> Option<u64> {
        self.votes
    }

    /// The provider's own identifiers for this title's genres, as a list answer named them.
    ///
    /// Empty is "the answer did not say", which is the normal state of a candidate from a details
    /// read and the reason a caller cannot read it as "no genres".
    #[must_use]
    pub fn genre_ids(&self) -> &[u32] {
        &self.genre_ids
    }
}

/// One person in a title's cast, as a provider names them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CastMember {
    /// The provider's own stable id for the person.
    pub provider_id: String,
    /// Their name.
    pub name: String,
    /// The character they play, when the provider says.
    pub character: Option<String>,
    /// Their billing position, lowest first.
    pub order: Option<u32>,
    /// The provider-relative path of their portrait, when it has one.
    pub profile_path: Option<String>,
}

/// A title's cast, in billing order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TitleCredits {
    /// The cast the provider named.
    pub cast: Vec<CastMember>,
}

/// A source of catalogue metadata.
///
/// Implementors are adapters: `peregrine-providers` keeps every provider's own types inside
/// its own module and returns only domain values (ADR 0007 rule 2), so nothing about TMDB's
/// JSON escapes the adapter that speaks it.
pub trait MetadataProvider: Send + Sync {
    /// What this provider is, what it can be asked for, and the terms it answers under.
    ///
    /// Borrowed rather than returned by value: the descriptor is fixed when the adapter is
    /// constructed and read on every call — the token bucket reads the rate before each
    /// request — so copying it would be a per-request allocation for no gain.
    fn descriptor(&self) -> &ProviderDescriptor;

    /// Asks whether this provider is reachable and its credential is accepted.
    ///
    /// It answers `Ok(())` and nothing else, deliberately: the question is "can this provider be
    /// used", and a caller that wanted what the answer *said* would be a caller asking a different
    /// one. The implementation picks the cheapest request it has — TMDB reads its own configuration,
    /// which is a few hundred bytes — and it must not decode the answer into a model, because a build
    /// that failed on a shape it does not read would report a working credential as broken the day
    /// the provider adds a field.
    ///
    /// **A refused credential must be [`ProviderError::Unauthorized`] and not a transport failure.**
    /// That distinction is the whole point of the operation: the settings screen tells a person
    /// either "that key was refused" or "TMDB could not be reached", and those are different
    /// instructions. It is also what the conformance suite holds every adapter to.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Unauthorized`] when the provider refused the credential,
    /// [`ProviderError::RateLimited`] or [`ProviderError::Transport`] when the provider could not be
    /// asked, and [`ProviderError::TooLarge`] when the answer exceeded the cap the caller set.
    fn check(&self) -> ProviderFuture<'_, ()>;

    /// Finds titles whose name matches `query`.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Unsupported`] when the descriptor does not declare
    /// [`crate::capability::Capability::Search`]. The check is the implementation's to make, because only it
    /// knows what it can serve, and the conformance suite is what holds it to that: a provider
    /// either answers or refuses, and it never spends a request finding out which.
    ///
    /// [`ProviderError::RateLimited`], [`ProviderError::SchemaDrift`],
    /// [`ProviderError::TooLarge`], and [`ProviderError::Transport`] are the provider's own
    /// answers, mapped by the adapter from its HTTP stack (PG-E5-02, PG-E5-05).
    fn search(&self, query: &SearchQuery) -> ProviderFuture<'_, Vec<SearchCandidate>>;

    /// Reads one title's details.
    ///
    /// `kind` is a parameter because a provider's identifier space is not always split by kind — TMDB
    /// numbers films and series in one namespace of integers — so an id alone does not say which
    /// endpoint answers it.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Unsupported`] when the descriptor does not declare
    /// [`crate::capability::Capability::Title`], and otherwise as [`MetadataProvider::search`].
    fn title(
        &self,
        id: &ExternalId,
        kind: TitleKind,
    ) -> ProviderFuture<'_, peregrine_domain::catalogue::TitleDetails>;

    /// Reads a series' seasons, with the name, synopsis, air date, poster, and episode count each one
    /// publishes.
    ///
    /// **A film has no seasons, and that is a refusal rather than an empty list.** An empty answer is a
    /// real statement about a series — a show announced with no season published yet — and a screen
    /// draws the two differently, so `kind` is a parameter and a provider asked about a film answers
    /// [`ProviderError::Unsupported`] rather than "nothing".
    ///
    /// The numbers in the answer are the canonical ones `DATA_MODEL` §2.3 fixes: they are what the
    /// write path keys on, which is why a provider that numbers a season differently is an ordering
    /// overlay upstream of this port rather than a renumbering here.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Unsupported`] when `kind` is a film or the descriptor does not declare
    /// [`crate::capability::Capability::Season`], and otherwise as [`MetadataProvider::search`].
    fn seasons(&self, id: &ExternalId, kind: TitleKind) -> ProviderFuture<'_, Vec<SeasonDetails>>;

    /// Reads one season's episodes, with everything a card draws: the name, the synopsis, the air date,
    /// the runtime, the still, and the score.
    ///
    /// Bounded by what a season is, and not paged: a season is a few dozen episodes, and the caller that
    /// writes them does so in one pass. `season_number` is the number `seasons` answered with, so the
    /// two reads cannot disagree about which season was asked for.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Unsupported`] as [`MetadataProvider::seasons`], and
    /// [`ProviderError::NotFound`] when the provider holds no such season — which the caller treats as
    /// that one season's problem rather than the whole title's, because a series whose answer is
    /// missing one season still has the others.
    fn season_episodes(
        &self,
        id: &ExternalId,
        kind: TitleKind,
        season_number: SeasonNumber,
    ) -> ProviderFuture<'_, Vec<EpisodeDetails>>;

    /// Reads one page of a named rail.
    ///
    /// `page` is a [`NonZeroU32`] because a provider's lists are paged from one, so "the zeroth
    /// page" is unrepresentable rather than refused — the mistake a home screen is most likely to
    /// make, since it draws the first page and nothing else.
    ///
    /// `Unsupported` is not the only refusal worth naming: a provider may declare
    /// [`crate::capability::Capability::Trending`] and still have no list for one rail. That is
    /// also [`ProviderError::Unsupported`], and it is why the conformance suite asks a provider to
    /// refuse a rail it does not serve rather than to answer an empty list — an empty answer is a
    /// real statement about a provider's catalogue, and a screen draws the two differently.
    ///
    /// An answer is at most [`RAIL_PAGE_LIMIT`] candidates, because that is what a page is.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Unsupported`] when the descriptor does not declare
    /// [`crate::capability::Capability::Trending`], and otherwise as [`MetadataProvider::search`].
    fn rail(&self, rail: RailKind, page: NonZeroU32) -> ProviderFuture<'_, Vec<SearchCandidate>>;

    /// Reads one page of a list composed from filters.
    ///
    /// This is the operation [`MetadataProvider::rail`] is not: a rail is a *named* list a screen
    /// draws because a provider publishes it, and this is a list a caller composes — a kind, an
    /// order, a genre, and a floor under the score. It answers a [`ProviderPage`] rather than a bare
    /// list because a caller that pages has to know where the provider's list ends, and only the
    /// adapter that read the answer can say.
    ///
    /// **It is gated on [`crate::capability::Capability::Trending`], and that is deliberate.** The
    /// capability set is `ARCHITECTURE` §13's eight, and "read what is popular now" is the one that
    /// describes a provider publishing queried lists at all; a ninth name would change a documented
    /// contract for an operation whose *shape* the existing capability already covers. A provider
    /// that declares `Trending` and has no filtered endpoint refuses with
    /// [`ProviderError::Unsupported`] for the reason [`MetadataProvider::rail`] gives — an empty
    /// answer is a real statement about a catalogue, and a screen draws the two differently.
    ///
    /// A page holds at most [`RAIL_PAGE_LIMIT`] candidates, because a page is a page whatever
    /// composed it.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Unsupported`] when the descriptor does not declare
    /// [`crate::capability::Capability::Trending`], and otherwise as [`MetadataProvider::search`].
    fn browse(&self, request: &BrowseRequest) -> ProviderFuture<'_, ProviderPage>;

    /// Reads the genres this provider filters by for `kind`.
    ///
    /// `kind` is a parameter because a provider's genre vocabulary is per kind rather than global:
    /// TMDB's film and series lists differ — "Action & Adventure" exists only for series, and the
    /// two number even the names they share differently — so a caller that cached one list for both
    /// would hand a series genre id to a film filter and get nothing back.
    ///
    /// It is gated on [`crate::capability::Capability::Trending`] beside
    /// [`MetadataProvider::browse`], because it is the request that gives the browse filter its
    /// vocabulary: a provider with no queried list has no genres to filter one by.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Unsupported`] when the descriptor does not declare
    /// [`crate::capability::Capability::Trending`], and otherwise as [`MetadataProvider::search`].
    fn genres(&self, kind: TitleKind) -> ProviderFuture<'_, Vec<GenreRef>>;

    /// Reads the titles a provider considers like `id`.
    ///
    /// **A recommendation is the provider's, not this build's.** `PG-E7-15` will one day rank a local
    /// history, but a title's own page asks a simpler question — what else is like this one — and
    /// that is a list only the provider can answer. `kind` is a parameter for the reason
    /// [`MetadataProvider::title`] gives: TMDB numbers films and series in one namespace of integers,
    /// so an id alone does not say which endpoint answers it.
    ///
    /// The answer is bounded like every other list ([`RAIL_PAGE_LIMIT`]), because that is what the
    /// endpoint returns and what the rail that draws it holds. It is deliberately *not* paged: a rail
    /// on a title's page is a strip a reader glances along, and `PG-E7-15` owns the version that
    /// ranks and pages.
    ///
    /// Gated on [`crate::capability::Capability::Trending`] beside [`MetadataProvider::browse`]: it is
    /// the same kind of request — a list a caller composes rather than one a provider names — and a
    /// provider that publishes no queried list has none to answer with.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Unsupported`] when the descriptor does not declare
    /// [`crate::capability::Capability::Trending`], and otherwise as [`MetadataProvider::search`].
    fn similar(&self, id: &ExternalId, kind: TitleKind)
    -> ProviderFuture<'_, Vec<SearchCandidate>>;

    /// Reads one title's cast.
    ///
    /// A provider that does not serve credits refuses with [`ProviderError::Unsupported`] rather
    /// than spending a request finding out, the same rule every other port method states.
    fn credits(&self, _id: &ExternalId, _kind: TitleKind) -> ProviderFuture<'_, TitleCredits> {
        let unsupported = ProviderError::Unsupported {
            provider: self.descriptor().provider(),
            capability: "credits",
        };
        Box::pin(async move { Err(unsupported) })
    }

    /// Reads one image's bytes.
    ///
    /// `path` is the provider-relative path a title's details carry, not a URL: only the adapter knows
    /// which host serves its images, and a caller that built the URL itself would be a caller that knows
    /// a provider's business.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Unsupported`] when the descriptor does not declare
    /// [`crate::capability::Capability::Images`], and otherwise as [`MetadataProvider::search`].
    fn image(
        &self,
        path: &str,
        kind: peregrine_domain::catalogue::ImageKind,
    ) -> ProviderFuture<'_, Vec<u8>>;
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::num::NonZeroU32;
    use std::pin::Pin;

    use jiff::civil::Date;

    use peregrine_domain::TitleKind;
    use peregrine_domain::id::{ExternalId, Provider};
    use peregrine_domain::text::NonEmptyText;

    use super::*;

    fn text(value: &str) -> NonEmptyText {
        NonEmptyText::new(value).expect("the test passes real text")
    }

    fn limit(value: u32) -> NonZeroU32 {
        NonZeroU32::new(value).expect("the test passes a non-zero limit")
    }

    /// A provider whose only job is to prove the trait can be used behind `dyn`.
    struct Stub {
        descriptor: ProviderDescriptor,
    }

    impl MetadataProvider for Stub {
        fn descriptor(&self) -> &ProviderDescriptor {
            &self.descriptor
        }

        fn check(&self) -> ProviderFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }

        fn image(
            &self,
            _path: &str,
            _kind: peregrine_domain::catalogue::ImageKind,
        ) -> ProviderFuture<'_, Vec<u8>> {
            Box::pin(async {
                Err(ProviderError::Unsupported {
                    provider: Provider::Tmdb,
                    capability: "images",
                })
            })
        }

        fn title(
            &self,
            _id: &ExternalId,
            _kind: TitleKind,
        ) -> ProviderFuture<'_, peregrine_domain::catalogue::TitleDetails> {
            Box::pin(async {
                Err(ProviderError::Unsupported {
                    provider: Provider::Tmdb,
                    capability: "title",
                })
            })
        }

        fn search(&self, _query: &SearchQuery) -> ProviderFuture<'_, Vec<SearchCandidate>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn seasons(
            &self,
            _id: &ExternalId,
            _kind: TitleKind,
        ) -> ProviderFuture<'_, Vec<SeasonDetails>> {
            Box::pin(async {
                Err(ProviderError::Unsupported {
                    provider: Provider::Tmdb,
                    capability: "get_season",
                })
            })
        }

        fn season_episodes(
            &self,
            _id: &ExternalId,
            _kind: TitleKind,
            _season_number: SeasonNumber,
        ) -> ProviderFuture<'_, Vec<EpisodeDetails>> {
            Box::pin(async {
                Err(ProviderError::Unsupported {
                    provider: Provider::Tmdb,
                    capability: "get_season",
                })
            })
        }

        fn rail(
            &self,
            _rail: RailKind,
            _page: NonZeroU32,
        ) -> ProviderFuture<'_, Vec<SearchCandidate>> {
            Box::pin(async {
                Err(ProviderError::Unsupported {
                    provider: Provider::Tmdb,
                    capability: "trending",
                })
            })
        }

        fn browse(&self, _request: &BrowseRequest) -> ProviderFuture<'_, ProviderPage> {
            Box::pin(async {
                Err(ProviderError::Unsupported {
                    provider: Provider::Tmdb,
                    capability: "trending",
                })
            })
        }

        fn genres(&self, _kind: TitleKind) -> ProviderFuture<'_, Vec<GenreRef>> {
            Box::pin(async {
                Err(ProviderError::Unsupported {
                    provider: Provider::Tmdb,
                    capability: "trending",
                })
            })
        }

        fn similar(
            &self,
            _id: &ExternalId,
            _kind: TitleKind,
        ) -> ProviderFuture<'_, Vec<SearchCandidate>> {
            Box::pin(async {
                Err(ProviderError::Unsupported {
                    provider: Provider::Tmdb,
                    capability: "trending",
                })
            })
        }
    }

    fn stub() -> Stub {
        use crate::capability::{Capabilities, Capability};
        use crate::descriptor::{CachePolicy, RatePolicy};

        let rate = RatePolicy::new(limit(5), std::time::Duration::from_secs(1), limit(1), false)
            .expect("well formed");
        let cache =
            CachePolicy::new(std::time::Duration::from_secs(60), None, false).expect("well formed");
        Stub {
            descriptor: ProviderDescriptor::new(
                Provider::Tmdb,
                "Stub",
                Capabilities::EMPTY.with(Capability::Search),
                rate,
                cache,
                None,
            )
            .expect("well formed"),
        }
    }

    #[test]
    fn the_port_can_be_used_behind_a_trait_object() {
        // The registry holds providers of different types in one list (PG-E5-12), which is
        // why the operation returns a boxed future instead of an `async fn` — that form does
        // not compile behind `dyn` on this toolchain.
        let provider: Box<dyn MetadataProvider> = Box::new(stub());

        assert_eq!(provider.descriptor().provider(), Provider::Tmdb);
    }

    #[test]
    fn a_query_carries_what_was_asked_for() {
        let query = SearchQuery::new(text("breaking bad"), limit(20), Some(TitleKind::Show));

        assert_eq!(query.text().as_str(), "breaking bad");
        assert_eq!(query.limit().get(), 20);
        assert_eq!(query.kind(), Some(TitleKind::Show));
    }

    #[test]
    fn a_query_for_either_kind_says_so_rather_than_guessing_one() {
        let query = SearchQuery::new(text("dune"), limit(5), None);

        assert!(query.kind().is_none());
    }

    #[test]
    fn a_candidate_keeps_the_provider_that_answered() {
        let id = ExternalId::new(Provider::Tmdb, "1396").expect("a valid id");
        let candidate = SearchCandidate::new(
            id.clone(),
            text("Breaking Bad"),
            TitleKind::Show,
            Some(Date::constant(2008, 1, 20)),
        );

        assert_eq!(candidate.external_id(), &id);
        assert_eq!(candidate.name().as_str(), "Breaking Bad");
        assert_eq!(candidate.kind(), TitleKind::Show);
        assert_eq!(candidate.released(), Some(Date::constant(2008, 1, 20)));
    }

    #[test]
    fn a_candidate_whose_provider_gave_no_date_is_still_a_candidate() {
        // Not every provider dates everything, and a candidate without a date is still
        // enough to list and to re-ask for.
        let candidate = SearchCandidate::new(
            ExternalId::new(Provider::Tvdb, "81189").expect("a valid id"),
            text("Breaking Bad"),
            TitleKind::Show,
            None,
        );

        assert!(candidate.released().is_none());
    }

    #[test]
    fn a_candidate_carries_the_artwork_and_the_score_a_list_answer_gave_it() {
        // What makes one request per title enough (`PG-E7-08`): the list answer already has both,
        // and the provider's own field names stop at the adapter.
        let candidate = SearchCandidate::new(
            ExternalId::new(Provider::Tmdb, "603").expect("a valid id"),
            text("The Matrix"),
            TitleKind::Movie,
            Some(Date::constant(1999, 3, 31)),
        )
        .with_artwork_path(Some("/poster.jpg".to_owned()))
        .with_rating(Some(8.2));

        assert_eq!(candidate.artwork_path(), Some("/poster.jpg"));
        assert_eq!(candidate.rating(), Some(8.2));
    }

    #[test]
    fn a_candidate_a_list_answer_gave_neither_artwork_nor_a_score_is_still_a_candidate() {
        // The default every constructor uses: a list endpoint that carries no poster still lists
        // titles, and a candidate that could not be built without artwork would drop them.
        let candidate = SearchCandidate::new(
            ExternalId::new(Provider::Tvdb, "81189").expect("a valid id"),
            text("Breaking Bad"),
            TitleKind::Show,
            None,
        );

        assert!(candidate.artwork_path().is_none());
        assert!(
            candidate.rating().is_none(),
            "an absent score is not a score of zero"
        );
        assert!(candidate.overview().is_none());
        assert!(
            candidate.votes().is_none(),
            "an absent count is not a count of zero"
        );
    }

    #[test]
    fn a_candidate_carries_the_synopsis_and_the_vote_count_a_list_answer_gave_it() {
        // What the hover preview draws (`PG-E7-05`): a list answer already sends both, so the panel
        // costs no second request — and both are optional, because not every provider writes prose.
        let candidate = SearchCandidate::new(
            ExternalId::new(Provider::Tmdb, "603").expect("a valid id"),
            text("The Matrix"),
            TitleKind::Movie,
            Some(Date::constant(1999, 3, 31)),
        )
        .with_overview(Some("A hacker learns the truth.".to_owned()))
        .with_votes(Some(24_000));

        assert_eq!(candidate.overview(), Some("A hacker learns the truth."));
        assert_eq!(candidate.votes(), Some(24_000));
    }

    #[test]
    fn a_provider_can_be_awaited_through_the_port() {
        // The future is `Send`, which is what lets a provider be called from the scheduler's
        // worker pool rather than from one thread.
        fn assert_send<T: Send>(_: &T) {}

        let stub = stub();
        let query = SearchQuery::new(text("anything"), limit(1), None);
        let future: Pin<Box<dyn Future<Output = _> + Send + '_>> = stub.search(&query);
        assert_send(&future);
    }

    #[test]
    fn every_rail_has_a_stable_name_and_the_kind_it_carries() {
        // The name crosses IPC and a journal payload, so a rename is a wire change rather than
        // copy; the kind is what tells an adapter which endpoint can answer a rail.
        let named: Vec<&str> = RailKind::ALL.iter().map(|rail| rail.as_str()).collect();

        assert_eq!(
            named,
            [
                "trending_week",
                "popular_movies",
                "top_rated_movies",
                "upcoming_movies",
                "popular_shows",
                "on_the_air_shows",
                "top_rated_shows",
            ]
        );
        assert_eq!(RailKind::PopularMovies.kind(), Some(TitleKind::Movie));
        assert_eq!(RailKind::TopRatedShows.kind(), Some(TitleKind::Show));
        assert!(
            RailKind::TrendingWeek.kind().is_none(),
            "the trending list mixes films and series, so its items carry their own kind"
        );
    }

    #[test]
    fn no_two_rails_share_a_name() {
        // Two rails with one name would be two lists a caller cannot tell apart, and a screen
        // would show the same one twice.
        let mut names: Vec<&str> = RailKind::ALL.iter().map(|rail| rail.as_str()).collect();
        names.sort_unstable();
        names.dedup();

        assert_eq!(names.len(), RailKind::ALL.len());
    }

    #[test]
    fn every_rail_name_round_trips_and_an_unknown_one_is_refused() {
        // The name is written to a manifest and read back at the next launch: a rail whose name
        // this build writes and cannot read would come back as no rail at all, silently.
        for rail in RailKind::ALL {
            assert_eq!(RailKind::parse(rail.as_str()), Some(rail));
        }
        assert_eq!(RailKind::parse("trending_day"), None);
        assert_eq!(RailKind::parse(""), None);
        // The check is on the stored spelling and not on a case-folded one: a name that reached
        // the database in another case is not one this build wrote.
        assert_eq!(RailKind::parse("Trending_Week"), None);
    }
}
