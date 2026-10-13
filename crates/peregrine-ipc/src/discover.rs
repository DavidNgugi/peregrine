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

use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;
use tauri::State;

use crate::error::AppError;
use crate::guard::guard_response;
use crate::jobs::JobStarted;
use crate::library::TitleMatchDto;
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

/// One title in a rail, as the screen draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
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
        }
    }
}

/// One rail: its stable identity and the titles in it.
///
/// **The heading is not here.** A rail's heading is a sentence — "On the air", "Top rated films" —
/// and a sentence is translated, so it lives in `packages/i18n` keyed by the rail's `rail` field.
/// Sending an English heading over IPC would put untranslatable copy in the core's mouth, which is
/// the mistake `AGENTS R7` names from the other direction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HomeRailDto {
    /// The rail's stable identity, which is what a translation is keyed by.
    pub rail: String,
    /// Its titles, in the order the provider listed them.
    pub items: Vec<RailItemDto>,
}

impl From<peregrine_app::HomeRail> for HomeRailDto {
    fn from(rail: peregrine_app::HomeRail) -> Self {
        Self {
            rail: rail.rail.as_str().to_owned(),
            items: rail.items.into_iter().map(RailItemDto::from).collect(),
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
) -> Result<Vec<TitleMatchDto>, AppError> {
    let query = query.trim().to_owned();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let found = state
        .runtime()?
        .discover(&query, limit.unwrap_or(20).clamp(1, 100))
        .await?;
    Ok(guard_response(
        "discover::search_providers",
        found.into_iter().map(TitleMatchDto::from).collect(),
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
/// # Errors
///
/// [`AppError`] when the runtime is not running. Nothing else: the read cannot fail, which is the
/// point of it.
#[tauri::command]
#[specta::specta]
pub async fn home_rails(state: State<'_, RuntimeState>) -> Result<Vec<HomeRailDto>, AppError> {
    let rails = state.runtime()?.home_rails()?;
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

#[cfg(test)]
mod tests {
    use peregrine_app::{HomeRail, RailItem, TitleKind};

    use super::*;

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
            }],
        };

        let dto = HomeRailDto::from(rail);

        assert_eq!(dto.rail, "on_the_air_shows");
        assert_eq!(dto.items.len(), 1);
        assert_eq!(dto.items[0].title_id, 12);
        assert_eq!(dto.items[0].kind, RailKindDto::Show);
        assert_eq!(dto.items[0].name, "Severance");
        assert_eq!(dto.items[0].year, Some(2022));
        assert_eq!(dto.items[0].poster.as_deref(), Some("/cache/images/abc"));
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
            }],
        };

        let dto = HomeRailDto::from(rail);

        assert_eq!(dto.items[0].poster, None);
        assert_eq!(dto.items[0].year, None);
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
            });
            assert_eq!(dto.rail, name);
        }
    }
}
