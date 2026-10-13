//! The `discover` namespace of the command surface (docs/ARCHITECTURE.md §7).
//!
//! One command, and it is the first that leaves the machine. What it returns is what the *catalogue*
//! holds after asking the providers, not what a provider said — the same DTO the library's search
//! returns, because a result is a title this application owns.

use tauri::State;

use crate::error::AppError;
use crate::guard::guard_response;
use crate::library::TitleMatchDto;
use crate::runtime::RuntimeState;

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
