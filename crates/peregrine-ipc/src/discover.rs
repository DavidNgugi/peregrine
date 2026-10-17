//! The `discover` namespace of the command surface (docs/ARCHITECTURE.md §7).
//!
//! Two reads and a job, and the split between them is the whole design: [`home_rails`] answers from
//! the catalogue and the artwork cache and never touches the network (`FLOWS F2`), while
//! [`refresh_rails`] starts the job that fills both. A screen paints what is here and asks for more
//! behind what it painted — which is why there is no command here that blocks on a provider.
//!
//! What a rail carries is a *title this application owns*: a provider id, a name, and a year are
//! not enough to open anything, so the item carries the catalogue's row id and the absolute path of
//! its cached poster. The bytes never cross IPC (`AGENTS R11`), and neither does the provider's own
//! path.

use std::num::NonZeroU32;

use peregrine_app::{BrowseRequest, BrowseSort, GenreRef};
use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;
use tauri::State;

use crate::error::AppError;
use crate::guard::guard_response;
use crate::jobs::JobStarted;
use crate::library::{TitleMatchDto, TitleRef};
use crate::runtime::RuntimeState;

/// Which of the two a rail item is, as the wire spells it.
///
/// Not `library::RootKind`: a rail holds titles, and `RootKind` also spells `mixed` and
/// `downloads` — folder kinds that no provider can publish. A screen branching on `RootKind` for a
/// poster tile would have to invent an answer for two cases that cannot occur, and a type that says
/// what cannot occur is cheaper than a branch nobody can test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum RailKindDto {
    /// A film.
    Movie,
    /// A series.
    Show,
}

impl From<RailKindDto> for peregrine_app::TitleKind {
    fn from(kind: RailKindDto) -> Self {
        match kind {
            RailKindDto::Movie => Self::Movie,
            RailKindDto::Show => Self::Show,
        }
    }
}

/// One title in a rail, as the screen draws it.
// Not `Eq`: the score a provider reports is an `f64`, and a float has no total equality.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RailItemDto {
    /// The catalogue row, which is what opening the title needs.
    #[specta(type = Number)]
    pub title_id: i64,
    /// Film or series.
    pub kind: RailKindDto,
    /// The name to draw.
    pub name: String,
    /// The release year, when the provider or the catalogue stated one.
    pub year: Option<i32>,
    /// Where the cached poster is on this machine, as an absolute path, or `null` while it has not
    /// been fetched.
    ///
    /// A path rather than a URL: the webview turns it into an `asset:` URL, and the scheme the shell
    /// scopes is the shell's business rather than this layer's.
    pub poster: Option<String>,
    /// Where the cached backdrop is on this machine, as an absolute path, or `null` while it has not
    /// been fetched.
    ///
    /// The wide image Discover's hero draws, and the second variant the same cache holds (`w780`
    /// against the poster's `w342`). `null` is a state the hero draws — the poster cropped to the
    /// band, and the reserved surface when there is no poster either — rather than a reason to
    /// stretch a portrait image across it.
    pub backdrop: Option<String>,
    /// Whether the library holds a file for this title that is not known to be missing.
    ///
    /// **A fact about the library, not a promise about playback**: it says a blob is on disk, not
    /// that it decodes or that this build has a player (`PG-E15`). It rides here because the hero's
    /// Play must be disabled for a reason the core stated rather than for one a screen assumed
    /// (`AGENTS R7`), and a control that could not be honoured had to be absent instead
    /// (`PG-E1-07`).
    pub playable: bool,
    /// The provider's own score for the title, or `null` when the list answer carried none.
    ///
    /// `null` is not `0.0`: a provider that sends no score, and one whose value for an unrated item
    /// the adapter mapped to absent, both arrive here as "no badge" — which is what the tile draws,
    /// and a `0.0` badge would be a claim the provider never made.
    pub rating: Option<f64>,
    /// The short synopsis the list answer carried, or `null` when it carried none.
    ///
    /// It is what the hover preview draws inside the enlarged card (`PG-E7-05`), and it is the
    /// *list* answer's prose rather than the details read's: a title whose details have been filled
    /// in may have a longer one, and this field is not that one.
    pub overview: Option<String>,
    /// How many people voted on it, or `null` when the answer carried no count.
    ///
    /// `null` is not `0`: a provider that sends nothing, and one that sends the zero it uses for an
    /// item nobody has voted on, both arrive as "no count to draw" — and "0 votes" beside a star is
    /// a claim the provider never made.
    ///
    /// Exported as `Number` rather than Specta's forbidden `u64`, which is the same precision
    /// argument `title_id` makes: a webview counts in `f64`, and no vote count will ever reach
    /// 2^53.
    #[specta(type = Option<Number>)]
    pub votes: Option<u64>,
}

impl From<peregrine_app::RailItem> for RailItemDto {
    fn from(item: peregrine_app::RailItem) -> Self {
        Self {
            title_id: item.title_id,
            kind: match item.kind {
                peregrine_app::TitleKind::Movie => RailKindDto::Movie,
                peregrine_app::TitleKind::Show => RailKindDto::Show,
            },
            name: item.name,
            year: item.year,
            poster: item.poster.map(|path| path.to_string_lossy().into_owned()),
            backdrop: item
                .backdrop
                .map(|path| path.to_string_lossy().into_owned()),
            playable: item.playable,
            rating: item.rating,
            overview: item.overview,
            votes: item.votes,
        }
    }
}

/// What the Discover screen's rails are doing.
///
/// **The screen's whole vocabulary for this section**, and it is the core's to state (`AGENTS R7`):
/// an installation nobody has given a key to, one whose fetch is in the air, and one with rails to
/// draw are three different sentences, and "the items are empty" is what the first two look like
/// from here. A screen that derived this would tell a user who already set a key up to go and set
/// one up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum RailsStateDto {
    /// No provider holds a credential, so nothing can fill a rail until one is set up in Settings.
    NoProvider,
    /// A provider is configured and the fetch that fills the rails is under way.
    Fetching,
    /// The rails hold what the providers answered.
    Ready,
}

impl From<peregrine_app::RailsState> for RailsStateDto {
    fn from(state: peregrine_app::RailsState) -> Self {
        match state {
            peregrine_app::RailsState::NoProvider => Self::NoProvider,
            peregrine_app::RailsState::Fetching => Self::Fetching,
            peregrine_app::RailsState::Ready => Self::Ready,
        }
    }
}

/// One rail: its stable identity, the titles in it, and what the rails are doing.
///
/// **The heading is not here.** A rail's heading is a sentence — "On the air", "Top rated films" —
/// and a sentence is translated, so it lives in `packages/i18n` keyed by the rail's `rail` field.
/// Sending an English heading over IPC would put untranslatable copy in the core's mouth, which is
/// the mistake `AGENTS R7` names from the other direction.
///
/// **`state` is one fact about the read rather than about this rail**: whether a provider is
/// configured at all, and whether the fetch that fills the rails is in the air, is the same for all
/// seven. It rides on every rail because a list of rails is what this answer is, and the core
/// answers every rail — so a screen reads it off the answer it already has instead of inventing one
/// for a list that cannot be empty, and a screen renders it instead of deciding it (`AGENTS R7`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HomeRailDto {
    /// The rail's stable identity, which is what a translation is keyed by.
    pub rail: String,
    /// Its titles, in the order the provider listed them.
    pub items: Vec<RailItemDto>,
    /// Whether the rails are empty because nothing can fill them, empty because a fetch is in the
    /// air, or here.
    pub state: RailsStateDto,
}

impl From<peregrine_app::HomeRail> for HomeRailDto {
    fn from(rail: peregrine_app::HomeRail) -> Self {
        Self {
            rail: rail.rail.as_str().to_owned(),
            items: rail.items.into_iter().map(RailItemDto::from).collect(),
            state: RailsStateDto::from(rail.state),
        }
    }
}

/// How a browse page is ordered, as the wire spells it.
///
/// Its own type rather than the port's, for the reason [`RailKindDto`] gives: a wire spelling is a
/// contract this crate owns, and the two are free to move apart — a rename of the port's identifier
/// would otherwise be a silent change to a URL a webview had bookmarked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum BrowseSortDto {
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

impl From<BrowseSortDto> for BrowseSort {
    fn from(sort: BrowseSortDto) -> Self {
        match sort {
            BrowseSortDto::Popularity => Self::Popularity,
            BrowseSortDto::Rating => Self::Rating,
            BrowseSortDto::Newest => Self::Newest,
            BrowseSortDto::Oldest => Self::Oldest,
        }
    }
}

/// What a title's recommendation rail holds, and the genre shelves it implies.
///
/// The genres ride along rather than being asked for separately because they are derived from the same
/// answer: a title page draws the rail and then offers "more of this genre", and that genre is read off
/// the titles the provider listed around this one. **They are the provider's genre identifiers**, which
/// is what a browse request takes; the names come from [`genres`], which a filter control reads anyway.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RecommendationsDto {
    /// The rail's titles.
    pub items: Vec<RailItemDto>,
    /// The genres the rail shares, most-shared first and already named; empty when none is shared.
    pub genres: Vec<GenreDto>,
}

impl From<peregrine_app::Recommendations> for RecommendationsDto {
    fn from(found: peregrine_app::Recommendations) -> Self {
        Self {
            items: found.items.into_iter().map(RailItemDto::from).collect(),
            // The binding is what lets `GenreDto::from` resolve: the blanket `From<T> for T` makes a
            // bare path ambiguous when the target type is only known through the struct literal.
            genres: found
                .genres
                .into_iter()
                .map(GenreDto::from)
                .collect::<Vec<_>>(),
        }
    }
}

/// One genre a provider filters by, as a filter control draws it.
///
/// The id is the *provider's* own, which is what a browse request hands back to it, and the name is
/// the label. It is not a catalogue genre: `peregrine-storage`'s `genre` table numbers rows and this
/// numbers a provider's vocabulary, and the two are not the same set — TMDB's film and series
/// vocabularies disagree, and the table has no column that could say which one a row came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GenreDto {
    /// The provider's own identifier, which its filter takes.
    pub id: u32,
    /// The label a filter control draws.
    pub name: String,
}

impl From<GenreRef> for GenreDto {
    fn from(genre: GenreRef) -> Self {
        Self {
            id: genre.id(),
            name: genre.name().as_str().to_owned(),
        }
    }
}

/// One page of a browsed list: the titles on it and whether another page follows.
///
/// **`has_more` is the provider's answer rather than a screen's arithmetic.** A screen that inferred
/// it from the number of tiles would either stop one page early or ask for a page past the
/// provider's end on every scroll, and neither is something a reader can tell from the page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BrowsePageDto {
    /// The titles on this page, drawn exactly as a rail item is.
    pub items: Vec<RailItemDto>,
    /// Whether asking for the next page would find anything.
    pub has_more: bool,
}

impl From<peregrine_app::BrowsePage> for BrowsePageDto {
    fn from(page: peregrine_app::BrowsePage) -> Self {
        Self {
            items: page.items.into_iter().map(RailItemDto::from).collect(),
            has_more: page.has_more,
        }
    }
}

/// Searches the configured providers, writes what they know, and answers with what the catalogue holds.
///
/// An empty query is an empty result rather than an error: the search box calls this as the user types,
/// and a refusal to answer nothing would be a failure state on an empty box.
///
/// # Errors
///
/// [`AppError`] when a provider or the catalogue fails. A provider that is slow, rate-limited, or
/// unreachable is reported rather than hidden, and the screen keeps whatever it already had.
#[tauri::command]
#[specta::specta]
pub async fn search_providers(
    state: State<'_, RuntimeState>,
    query: String,
    limit: Option<u32>,
    kind: Option<RailKindDto>,
) -> Result<Vec<TitleMatchDto>, AppError> {
    let query = query.trim().to_owned();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let found = state
        .runtime()?
        .discover(
            &query,
            limit.unwrap_or(20).clamp(1, 100),
            kind.map(Into::into),
        )
        .await?;
    Ok(guard_response(
        "discover::search_providers",
        found.into_iter().map(TitleMatchDto::from).collect(),
    ))
}

/// Searches the configured providers and answers with the poster tiles the Browse grid draws.
///
/// **The tile-shaped sibling of [`search_providers`].** That one answers identities for a text list;
/// this one answers what a poster needs — the year, the score, the synopsis, and the cached artwork
/// path. Both write what the provider found into the catalogue before answering, and an empty query
/// is an empty result rather than a failure on an empty box.
///
/// # Errors
///
/// [`AppError`] when the runtime is not running, or when a provider or the catalogue fails. A provider
/// that is slow, rate-limited, or unreachable is reported rather than hidden.
#[tauri::command]
#[specta::specta]
pub async fn search_tiles(
    state: State<'_, RuntimeState>,
    query: String,
    limit: Option<u32>,
    kind: Option<RailKindDto>,
) -> Result<Vec<RailItemDto>, AppError> {
    let query = query.trim().to_owned();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let found = state
        .runtime()?
        .search_tiles(
            &query,
            limit.unwrap_or(20).clamp(1, 100),
            kind.map(Into::into),
        )
        .await?;
    Ok(guard_response(
        "discover::search_tiles",
        found.into_iter().map(RailItemDto::from).collect(),
    ))
}

/// Every rail the Discover screen draws, from what the catalogue and the cache already hold.
///
/// **Cache-first, and it never waits for a provider.** The rails were filled by
/// [`refresh_rails`]'s job; this read is a lock and a clone, which is what makes the first paint of
/// the screen possible before any request has left the machine (`FLOWS F2`). A rail nobody has
/// fetched yet is answered as an empty rail rather than omitted, so the screen has its geometry
/// from the first frame.
///
/// **A cold read also starts the fetch** (`FLOWS F2`), through the same call [`refresh_rails`]
/// makes, so a screen with a configured provider fills without anyone pressing a button. What each
/// rail's `state` adds is how the screen tells a cold fetch from a missing key rather than assuming
/// one from an empty list.
///
/// # Errors
///
/// [`AppError`] when the runtime is not running, or when the cold read cannot start the fetch.
#[tauri::command]
#[specta::specta]
pub async fn home_rails(
    state: State<'_, RuntimeState>,
    kind: Option<RailKindDto>,
) -> Result<Vec<HomeRailDto>, AppError> {
    let rails = state.runtime()?.home_rails(kind.map(Into::into)).await?;
    Ok(guard_response(
        "discover::home_rails",
        rails.into_iter().map(HomeRailDto::from).collect(),
    ))
}

/// Fills the rails the configured providers publish, artwork included, and answers with the job.
///
/// A job and not a command that waits, because it is one request per rail and then one per title it
/// has artwork for — a screen that blocked on that would freeze on a provider having a slow day
/// (`ARCHITECTURE §7` rule 2). Two visits inside one fetch share it, and the answer says which
/// happened through [`JobStarted::coalesced`].
///
/// # Errors
///
/// [`AppError`] when the journal or the queue refuses the job.
#[tauri::command]
#[specta::specta]
pub async fn refresh_rails(state: State<'_, RuntimeState>) -> Result<JobStarted, AppError> {
    let started = state.runtime()?.start_discover_rails().await?;
    Ok(guard_response("discover::refresh_rails", started.into()))
}

/// One page of a list composed from filters, from the configured providers.
///
/// **A command and not a job**, which is a deliberate reading of `ARCHITECTURE §7` rule 2 rather
/// than a shortcut around it: the rule keeps a screen from waiting on work it did not ask for, and
/// this read *is* the work the reader asked for. `search_providers` is the same shape and the
/// precedent — one provider request, written into the catalogue, answered while the caller waits. A
/// job would answer with an id and no page, leaving the screen with nothing to draw until a job it
/// cannot name stops, which is the full-page spinner `PG-E7-02` forbids.
///
/// `page` counts from one, every filter is optional, and `min_rating` is a whole score out of ten —
/// the scale the filter control draws. Both numbers are clamped rather than refused: a page of zero
/// and a floor above the scale are a caller's arithmetic rather than hostile input, and the closest
/// legal request is a better answer than a failure.
///
/// # Errors
///
/// [`AppError`] when the runtime is not running, when no provider is configured, or when a provider
/// or the catalogue fails. A provider that is slow, rate-limited, or unreachable is reported rather
/// than hidden, and the screen keeps whatever it already had.
#[tauri::command]
#[specta::specta]
pub async fn browse(
    state: State<'_, RuntimeState>,
    kind: RailKindDto,
    page: u32,
    sort: Option<BrowseSortDto>,
    genre: Option<u32>,
    min_rating: Option<u8>,
) -> Result<BrowsePageDto, AppError> {
    let request = BrowseRequest::new(
        kind.into(),
        NonZeroU32::new(page.max(1)).unwrap_or(NonZeroU32::MIN),
        sort.unwrap_or_default().into(),
        genre,
        min_rating.map(|floor| floor.min(10)),
    );
    let found = state.runtime()?.browse(&request).await?;
    Ok(guard_response(
        "discover::browse",
        BrowsePageDto::from(found),
    ))
}

/// The genres the configured providers filter by for `kind`.
///
/// The core answers this once per session: a provider's genre vocabulary changes on the provider's
/// clock, and a filter control that re-asked on every visit would spend a request per visit for an
/// answer nothing had changed (`AGENTS R24`).
///
/// # Errors
///
/// [`AppError`] when the runtime is not running, when no provider is configured, or when the
/// provider fails.
#[tauri::command]
#[specta::specta]
pub async fn genres(
    state: State<'_, RuntimeState>,
    kind: RailKindDto,
) -> Result<Vec<GenreDto>, AppError> {
    let list = state.runtime()?.genres(kind.into()).await?;
    // The binding's type is what lets `GenreDto::from` resolve: the blanket `From<T> for T` makes the
    // bare path ambiguous when the target type is only known through `guard_response`'s parameter.
    let dtos: Vec<GenreDto> = list.into_iter().map(GenreDto::from).collect();
    Ok(guard_response("discover::genres", dtos))
}

/// The titles the configured provider considers like one the catalogue holds.
///
/// A rail on a title's own page (`PG-E7-11`, `PG-E7-15`), and one provider request. **Empty is a
/// real answer rather than a failure**: a title no configured provider holds an identifier for has no
/// recommendations to fetch — a title the library inferred from a filename, or one another provider
/// described — and the screen draws no rail for it instead of an error.
///
/// # Errors
///
/// [`AppError`] when the runtime is not running, when no provider is configured, or when the
/// provider fails.
#[tauri::command]
#[specta::specta]
pub async fn recommendations(
    state: State<'_, RuntimeState>,
    title: TitleRef,
) -> Result<RecommendationsDto, AppError> {
    let found = state.runtime()?.similar(title.id).await?;
    Ok(guard_response(
        "discover::recommendations",
        RecommendationsDto::from(found),
    ))
}

#[cfg(test)]
mod tests {

    use super::*;
    use peregrine_app::{HomeRail, RailItem, TitleKind};

    /// The rail kinds the core names, as this layer sends them.
    ///
    /// Spelled here rather than taken from `peregrine_providers`: this crate may not depend on the
    /// adapter crate (`AGENTS R6`), and a *wire* spelling is a contract this crate owns — the test
    /// that the core's own names are these is the providers crate's.
    const RAIL_NAMES: [&str; 7] = [
        "trending_week",
        "popular_movies",
        "top_rated_movies",
        "upcoming_movies",
        "popular_shows",
        "on_the_air_shows",
        "top_rated_shows",
    ];

    /// The wire shape is the contract the screen is written against, so a rename is a deliberate
    /// edit here rather than a silent break there.
    #[test]
    fn a_rail_maps_its_identity_and_its_items() {
        let rail = HomeRail {
            rail: peregrine_app::RailKind::OnTheAirShows,
            items: vec![RailItem {
                title_id: 12,
                kind: TitleKind::Show,
                name: "Severance".to_owned(),
                year: Some(2022),
                poster: Some(std::path::PathBuf::from("/cache/images/abc")),
                backdrop_source: None,
                backdrop: Some(std::path::PathBuf::from("/cache/images/def")),
                playable: true,
                rating: Some(8.7),
                overview: Some("A severed employee questions the work.".to_owned()),
                votes: Some(3_100),
            }],
            state: peregrine_app::RailsState::Ready,
        };

        let dto = HomeRailDto::from(rail);

        assert_eq!(dto.rail, "on_the_air_shows");
        assert_eq!(dto.items.len(), 1);
        assert_eq!(dto.items[0].title_id, 12);
        assert_eq!(dto.items[0].kind, RailKindDto::Show);
        assert_eq!(dto.items[0].name, "Severance");
        assert_eq!(dto.items[0].year, Some(2022));
        assert_eq!(dto.items[0].poster.as_deref(), Some("/cache/images/abc"));
        assert_eq!(
            dto.items[0].backdrop.as_deref(),
            Some("/cache/images/def"),
            "the hero's wide image is a second path rather than the poster's"
        );
        assert!(
            dto.items[0].playable,
            "the hero's Play is enabled from the core's fact and not from a screen's guess"
        );
        assert_eq!(dto.items[0].rating, Some(8.7));
        assert_eq!(
            dto.items[0].overview.as_deref(),
            Some("A severed employee questions the work."),
            "the preview's prose is the core's answer, so the screen computes no rule"
        );
        assert_eq!(dto.items[0].votes, Some(3_100));
        assert_eq!(dto.state, RailsStateDto::Ready);
    }

    #[test]
    fn a_title_with_no_artwork_yet_says_so_rather_than_naming_a_path() {
        // The tile's reserved box is what a `null` poster draws, and a screen that received an
        // empty string instead would ask the asset protocol for the cache directory itself.
        let rail = HomeRail {
            rail: peregrine_app::RailKind::PopularMovies,
            items: vec![RailItem {
                title_id: 1,
                kind: TitleKind::Movie,
                name: "Dune".to_owned(),
                year: None,
                poster: None,
                backdrop_source: None,
                backdrop: None,
                playable: false,
                rating: None,
                overview: None,
                votes: None,
            }],
            state: peregrine_app::RailsState::Ready,
        };

        let dto = HomeRailDto::from(rail);

        assert_eq!(dto.items[0].poster, None);
        assert_eq!(
            dto.items[0].backdrop, None,
            "a title with no wide image draws a reserved box, not the poster stretched"
        );
        assert!(
            !dto.items[0].playable,
            "nothing on disk means Play has a reason to be disabled"
        );
        assert_eq!(dto.items[0].year, None);
        assert_eq!(
            dto.items[0].rating, None,
            "a list answer with no score is no badge, not a badge of zero"
        );
        assert_eq!(
            dto.items[0].overview, None,
            "a list answer with no prose draws no line rather than an empty one"
        );
        assert_eq!(
            dto.items[0].votes, None,
            "an absent count is no count, not a count of zero"
        );
        assert_eq!(dto.items[0].kind, RailKindDto::Movie);
    }

    /// Every rail the core names must have a spelling this layer sends, because the screen keys a
    /// translation on it and a rail that arrived unnamed could not be drawn at all.
    #[test]
    fn every_rail_kind_survives_the_crossing() {
        for name in RAIL_NAMES {
            let rail = peregrine_app::RailKind::ALL
                .into_iter()
                .find(|rail| rail.as_str() == name)
                .unwrap_or_else(|| panic!("the core knows no rail called {name}"));
            let dto = HomeRailDto::from(HomeRail {
                rail,
                items: Vec::new(),
                state: peregrine_app::RailsState::NoProvider,
            });
            assert_eq!(dto.rail, name);
        }
    }

    /// The three states the screen switches copy on, as the core states them.
    #[test]
    fn every_rails_state_survives_the_crossing() {
        for (state, expected) in [
            (
                peregrine_app::RailsState::NoProvider,
                RailsStateDto::NoProvider,
            ),
            (peregrine_app::RailsState::Fetching, RailsStateDto::Fetching),
            (peregrine_app::RailsState::Ready, RailsStateDto::Ready),
        ] {
            let dto = HomeRailDto::from(HomeRail {
                rail: peregrine_app::RailKind::TrendingWeek,
                items: Vec::new(),
                state,
            });

            assert_eq!(dto.state, expected);
        }
    }

    /// The wire spelling is what the screen switches on, so a rename is a deliberate edit here
    /// rather than a screen that silently matches no state and draws nothing.
    #[test]
    fn the_wire_spelling_of_a_state_is_pinned() {
        let spelled = |state| {
            serde_json::to_value(HomeRailDto::from(HomeRail {
                rail: peregrine_app::RailKind::TrendingWeek,
                items: Vec::new(),
                state,
            }))
            .expect("a state serializes")["state"]
                .clone()
        };

        assert_eq!(spelled(peregrine_app::RailsState::NoProvider), "noProvider");
        assert_eq!(spelled(peregrine_app::RailsState::Fetching), "fetching");
        assert_eq!(spelled(peregrine_app::RailsState::Ready), "ready");
    }
}
