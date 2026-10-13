//! The `library` namespace of the command surface (docs/ARCHITECTURE.md §7).
//!
//! Three commands, and the shape they establish is the one every later screen reuses: a read that
//! pages, a write that takes the thing the user chose, and a removal that says what it did not
//! touch. The DTOs are this module's rather than the domain's, because `peregrine-domain` cannot
//! depend on `specta` (AGENTS R6) and the wire shape is a contract of its own — a domain field
//! renamed should not silently rename a field the webview reads.

use peregrine_app::{LibraryRootKind, ScanMode, Timestamp};
use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;
use tauri::State;

use crate::error::AppError;
use crate::guard::guard_response;
use crate::jobs::JobStarted;
use crate::runtime::RuntimeState;

/// What a root is expected to hold, as the webview spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum RootKind {
    /// Films.
    Movies,
    /// Series.
    Shows,
    /// Both, or neither — a folder the user has not organised.
    Mixed,
    /// A download directory, which is watched rather than browsed.
    Downloads,
}

impl From<LibraryRootKind> for RootKind {
    fn from(kind: LibraryRootKind) -> Self {
        match kind {
            LibraryRootKind::Movies => Self::Movies,
            LibraryRootKind::Shows => Self::Shows,
            LibraryRootKind::Mixed => Self::Mixed,
            LibraryRootKind::Downloads => Self::Downloads,
        }
    }
}

impl From<RootKind> for LibraryRootKind {
    fn from(kind: RootKind) -> Self {
        match kind {
            RootKind::Movies => Self::Movies,
            RootKind::Shows => Self::Shows,
            RootKind::Mixed => Self::Mixed,
            RootKind::Downloads => Self::Downloads,
        }
    }
}

/// When a root is scanned, as the webview spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum RootScanMode {
    /// A watcher keeps it current.
    Incremental,
    /// On a schedule the settings own.
    Scheduled,
    /// Only when the user asks.
    Manual,
}

impl From<ScanMode> for RootScanMode {
    fn from(mode: ScanMode) -> Self {
        match mode {
            ScanMode::Incremental => Self::Incremental,
            ScanMode::Scheduled => Self::Scheduled,
            ScanMode::Manual => Self::Manual,
        }
    }
}

impl From<RootScanMode> for ScanMode {
    fn from(mode: RootScanMode) -> Self {
        match mode {
            RootScanMode::Incremental => Self::Incremental,
            RootScanMode::Scheduled => Self::Scheduled,
            RootScanMode::Manual => Self::Manual,
        }
    }
}

/// One title the library holds, as the library screen shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LibraryTitleDto {
    /// The row id, which is the paging cursor.
    #[specta(type = Number)]
    pub id: i64,
    /// The display name.
    pub name: String,
    /// Film or series, as the webview spells it.
    pub kind: RootKind,
    /// How complete the metadata is: a title the library holds is a stub until a sync fills it in,
    /// and a screen that showed a stub as complete would promise artwork and a synopsis it does not
    /// have (`PG-E5-06`).
    pub stub: bool,
    /// How many items of it the library holds.
    #[specta(type = Number)]
    pub items: i64,
    /// How many files those items have.
    #[specta(type = Number)]
    pub files: i64,
    /// What those files add up to, in bytes.
    #[specta(type = Number)]
    pub bytes: i64,
    /// How many of them are marked missing.
    #[specta(type = Number)]
    pub missing_files: i64,
}

impl From<peregrine_app::LibraryTitleRow> for LibraryTitleDto {
    fn from(row: peregrine_app::LibraryTitleRow) -> Self {
        Self {
            id: row.title.id,
            name: row.title.name,
            kind: match row.title.kind {
                peregrine_app::TitleKind::Movie => RootKind::Movies,
                peregrine_app::TitleKind::Show => RootKind::Shows,
            },
            stub: row.title.metadata_state != peregrine_app::MetadataState::Complete,
            items: row.items,
            files: row.files,
            bytes: row.bytes,
            missing_files: row.missing_files,
        }
    }
}

/// One title as its own screen reads it: the library's answer, plus the artwork.
///
/// Nested rather than a wider [`LibraryTitleDto`] because the two answer different questions and are
/// read by different screens: the list is "what the library holds", and this is "what the library
/// holds, and what a provider could still add". A hundred-row page must not carry a poster path it
/// never draws, and one DTO cannot mean two things.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TitleDetailDto {
    /// The title itself, exactly as the library screen lists it.
    pub title: LibraryTitleDto,
    /// The absolute path of the cached poster, or `null` when the cache holds none.
    ///
    /// A filesystem path rather than a URL or bytes: `apps/desktop/src/lib/media.ts` is the only
    /// module that turns one into something an `<img>` can load, and the bytes themselves travel over
    /// the asset protocol and never over IPC (AGENTS R11). A path this platform cannot spell as UTF-8
    /// is reported as `null` rather than lossily — a corrupted path is a request the asset protocol
    /// refuses, which is the state that looks like a rendering bug.
    pub poster: Option<String>,
    /// Whether opening this screen should ask the provider for the title.
    ///
    /// **A rule, and computed in the core for that reason** (AGENTS R7): it is "no provider has ever
    /// filled this title in, or a provider is known to have artwork the cache does not hold". A
    /// screen that decided this for itself would spend a request on every visit to a title whose
    /// provider has no poster, which is the shape AGENTS R24 exists to prevent. The *staleness*
    /// policy — when an already-filled title is due again — is `PG-E6-15`'s and is not this flag.
    pub refresh_due: bool,
}

/// One title a search found, as the picker lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TitleMatchDto {
    /// The title's id, which is what choosing one names.
    #[specta(type = Number)]
    pub id: i64,
    /// Its name.
    pub name: String,
    /// Whether it is a film or a series.
    pub kind: RootKind,
}

impl From<peregrine_app::TitleIdentity> for TitleMatchDto {
    fn from(identity: peregrine_app::TitleIdentity) -> Self {
        Self {
            id: identity.id,
            name: identity.name,
            kind: match identity.kind {
                peregrine_app::TitleKind::Movie => RootKind::Movies,
                peregrine_app::TitleKind::Show => RootKind::Shows,
            },
        }
    }
}

/// The title a command acts on.
///
/// A DTO rather than a bare `i64` argument, for the reason `RootRef` gives: specta refuses to export
/// a large integer as a command argument, so a bare id would be the one thing in the contract with no
/// type on the other side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TitleRef {
    /// The title's id, as a listing returned it.
    #[specta(type = Number)]
    pub id: i64,
}

/// One episode of a title, as its screen lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeDto {
    /// The season it sits in, which zero is for a show's specials.
    #[specta(type = Number)]
    pub season_number: u32,
    /// Its number within the season.
    #[specta(type = Number)]
    pub episode_number: u32,
    /// Its name, when a provider sync has filled one in.
    pub name: Option<String>,
    /// How many items the library holds for it.
    #[specta(type = Number)]
    pub items: i64,
    /// How many files those items have.
    #[specta(type = Number)]
    pub files: i64,
    /// What those files weigh together.
    #[specta(type = Number)]
    pub bytes: i64,
    /// How many of them the last scan could not find.
    #[specta(type = Number)]
    pub missing_files: i64,
}

impl From<peregrine_app::TitleEpisodeRow> for EpisodeDto {
    fn from(row: peregrine_app::TitleEpisodeRow) -> Self {
        Self {
            season_number: row.season_number.get(),
            episode_number: row.episode_number.get(),
            name: row.name,
            items: row.items,
            files: row.files,
            bytes: row.bytes,
            missing_files: row.missing_files,
        }
    }
}

/// One rule a user's correction became, as the screen lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchRuleDto {
    /// The row id, which is what a removal names.
    #[specta(type = Number)]
    pub id: i64,
    /// The title a name read, which is what later names are matched by.
    pub pattern: String,
    /// The title those names are placed under.
    pub title_name: String,
}

impl From<peregrine_app::MatchRuleRow> for MatchRuleDto {
    fn from(row: peregrine_app::MatchRuleRow) -> Self {
        Self {
            id: row.id,
            pattern: row.pattern,
            title_name: row.title_name,
        }
    }
}

/// The rule a command forgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchRuleRef {
    /// The row id, as the list returned it.
    #[specta(type = Number)]
    pub id: i64,
}

/// The parked file a command acts on.
///
/// A DTO rather than a bare `i64` argument, for the reason `RootRef` gives: specta refuses to export
/// a large integer as a command argument, so a bare id would be the one thing in the contract with
/// no type on the other side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UnmatchedRef {
    /// The row id, as the queue returned it.
    #[specta(type = Number)]
    pub id: i64,
    /// Which title the answer files the file under: the one its name read, the one the scan offered,
    /// or the one a person chose.
    pub as_title: MatchAsDto,
    /// The chosen title's id, when `asTitle` is `chosen` and the answer names one.
    #[specta(type = Option<Number>)]
    pub title_id: Option<i64>,
}

/// The two titles an answer can file a parked file under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum MatchAsDto {
    /// What the name read, which is what the row's own values say.
    Parsed,
    /// What the scan offered, which joins a title the library already holds when there is one.
    Suggested,
    /// A title a person chose from a search.
    Chosen,
}

impl UnmatchedRef {
    /// The placement this answer asks for.
    ///
    /// A `chosen` answer with no id is the parse's reading rather than a failure: the request is
    /// malformed, and the honest thing to do with a malformed request that still names a file is to
    /// place it the way its own name says rather than to refuse the row.
    fn as_title(self) -> peregrine_app::MatchAs {
        match (self.as_title, self.title_id) {
            (MatchAsDto::Chosen, Some(title_id)) => peregrine_app::MatchAs::Title(title_id),
            (MatchAsDto::Chosen, None) | (MatchAsDto::Parsed, _) => peregrine_app::MatchAs::Parsed,
            (MatchAsDto::Suggested, _) => peregrine_app::MatchAs::Suggested,
        }
    }
}

/// One file a scan could not place, as the queue shows it.
///
/// The parse's own fields are here rather than a re-derivation of them: this row exists because the
/// parse *failed*, so what a person is being asked is which of the values it managed to read should
/// be trusted. Showing the file's own name beside them is what makes that answerable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UnmatchedFileDto {
    /// The row id, which is what a match names.
    #[specta(type = Number)]
    pub id: i64,
    /// The file's name, without the directories it sits in.
    pub name: String,
    /// The title the parse read out of it, when it read one.
    pub parsed_title: Option<String>,
    /// The year it read, when the name stated one.
    #[specta(type = Option<Number>)]
    pub parsed_year: Option<u16>,
    /// The season it read, when the name stated one.
    #[specta(type = Option<Number>)]
    pub parsed_season: Option<u32>,
    /// The episode it read, when the name stated one.
    #[specta(type = Option<Number>)]
    pub parsed_episode: Option<u32>,
    /// Why the scan could not place it.
    pub reason: Option<String>,
    /// The title the scan offered for it, when one is close enough and this account may see it.
    ///
    /// `None` covers both "nothing was close enough" and "the title offered is out of scope", which
    /// are the same thing to the person answering (I37).
    pub suggested_title_name: Option<String>,
    /// What the file weighs, in bytes.
    #[specta(type = Number)]
    pub size_bytes: i64,
}

impl From<peregrine_app::UnmatchedFileRow> for UnmatchedFileDto {
    fn from(row: peregrine_app::UnmatchedFileRow) -> Self {
        let file = row.file;
        // The name alone: the queue is a list of decisions about *files*, and two files in different
        // folders with the same name are answered by the same reading of that name.
        let name = file
            .path
            .as_str()
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(file.path.as_str())
            .to_owned();
        Self {
            id: row.id,
            name,
            parsed_title: file.parsed_title,
            parsed_year: file.parsed_year,
            parsed_season: file.parsed_season.map(peregrine_app::SeasonNumber::get),
            parsed_episode: file.parsed_episode.map(peregrine_app::EpisodeNumber::get),
            reason: file.reason,
            suggested_title_name: row.suggested_title_name,
            size_bytes: i64::try_from(file.size_bytes).unwrap_or(i64::MAX),
        }
    }
}

/// The root a command acts on.
///
/// A DTO rather than a bare `i64` argument, because specta refuses to export a large integer as a
/// command argument — a bare cursor or id would be the one thing in the contract with no type on
/// the other side. The same reason `jobs::list` shows one page and takes no cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RootRef {
    /// The row id, as the list returned it.
    #[specta(type = Number)]
    pub id: i64,
}

/// One library root as the settings screen shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LibraryRootDto {
    /// The row id, which is the paging cursor and what a removal names.
    ///
    /// `Number` opts this `i64` into a TypeScript `number`, which specta otherwise refuses rather
    /// than silently losing precision. A row id and a Unix-millisecond instant both stay well
    /// inside the 2^53 a JavaScript number represents exactly.
    #[specta(type = Number)]
    pub id: i64,
    /// The absolute, canonical directory.
    pub path: String,
    /// What it is expected to hold.
    pub kind: RootKind,
    /// When it is scanned.
    pub scan_mode: RootScanMode,
    /// When it was last scanned, in Unix milliseconds, or nothing when it never was.
    #[specta(type = Option<Number>)]
    pub last_scan_at: Option<i64>,
    /// When it was added, in Unix milliseconds.
    #[specta(type = Number)]
    pub created_at: i64,
}

/// Starts filling one title from its provider, artwork included, and answers with the job.
///
/// A job and not a command that waits, because it leaves the machine twice — once for the title's
/// details and once for its poster — and a screen that blocked on that would be a screen that froze on
/// a provider having a slow day (`ARCHITECTURE §7` rule 2). Keyed by the title, so opening the same
/// screen twice waits on one refresh.
///
/// # Errors
///
/// [`AppError`] when the journal or the queue refuses the job.
#[tauri::command]
#[specta::specta]
pub async fn refresh_title(
    state: State<'_, RuntimeState>,
    title: TitleRef,
    kind: RootKind,
) -> Result<JobStarted, AppError> {
    let kind = match kind {
        RootKind::Shows => peregrine_app::TitleKind::Show,
        _ => peregrine_app::TitleKind::Movie,
    };
    let started = state
        .runtime()?
        .start_provider_refresh(title.id, kind)
        .await?;
    Ok(guard_response(
        "library::refresh_title",
        JobStarted::from(started),
    ))
}

/// One page of library roots, newest first.
#[tauri::command]
#[specta::specta]
pub async fn list_roots(
    state: State<'_, RuntimeState>,
    limit: Option<u32>,
) -> Result<Vec<LibraryRootDto>, AppError> {
    let roots = state
        .runtime()?
        .library_roots(None, limit.unwrap_or(50))
        .await?;
    Ok(guard_response(
        "library::list_roots",
        roots
            .into_iter()
            .map(|row| LibraryRootDto {
                id: row.root.id,
                path: row.root.path.as_str().to_owned(),
                kind: row.root.kind.into(),
                scan_mode: row.root.scan_mode.into(),
                last_scan_at: row.root.last_scan_at.map(Timestamp::as_millisecond),
                created_at: row.root.created_at.as_millisecond(),
            })
            .collect(),
    ))
}

/// Adds a folder as a library root and answers with the row it wrote.
///
/// The path is text the shell's picker handed over without validating it (PG-E1-12), so the only
/// thing this command does with it is pass it to the service that decides. A folder that is
/// already a root comes back as an error naming the row that holds it, which is what lets the
/// screen point at the existing entry instead of only refusing.
#[tauri::command]
#[specta::specta]
pub async fn add_root(
    state: State<'_, RuntimeState>,
    path: String,
    kind: RootKind,
    scan_mode: RootScanMode,
) -> Result<LibraryRootDto, AppError> {
    let added = state
        .runtime()?
        .add_library_root(&path, kind.into(), scan_mode.into())
        .await?;
    Ok(guard_response(
        "library::add_root",
        LibraryRootDto {
            id: added.id,
            path: added.path.as_str().to_owned(),
            kind: added.kind.into(),
            scan_mode: added.scan_mode.into(),
            last_scan_at: added.last_scan_at.map(Timestamp::as_millisecond),
            created_at: added.created_at.as_millisecond(),
        },
    ))
}

/// A page of the titles the library holds, newest first.
///
/// The first read of the library's own content rather than of its configuration, and the one the
/// library screen is built on. A title the library holds is a *stub* until a provider sync fills it
/// in, which the DTO says plainly: a screen can then show what it has without implying it has more.
#[tauri::command]
#[specta::specta]
pub async fn list_titles(
    state: State<'_, RuntimeState>,
    limit: Option<u32>,
) -> Result<Vec<LibraryTitleDto>, AppError> {
    let titles = state
        .runtime()?
        .library_titles(None, limit.unwrap_or(100))
        .await?;
    let titles = titles.into_iter().map(LibraryTitleDto::from).collect();
    Ok(guard_response("library::list_titles", titles))
}

/// A page of titles that match the user's own text.
///
/// The picker's read. The query is refused rather than run when it is empty: an empty search is not a
/// search, and answering it with a page of everything would be the screen's own list again.
#[tauri::command]
#[specta::specta]
pub async fn search_titles(
    state: State<'_, RuntimeState>,
    query: String,
    limit: Option<u32>,
) -> Result<Vec<TitleMatchDto>, AppError> {
    if query.trim().is_empty() {
        return Ok(guard_response("library::search_titles", Vec::new()));
    }
    let found = state
        .runtime()?
        .search_titles(&query, limit.unwrap_or(20))
        .await?;
    let found = found.into_iter().map(TitleMatchDto::from).collect();
    Ok(guard_response("library::search_titles", found))
}

/// One title, with the counts its screen shows and the artwork the cache holds for it, or `null`
/// when it is not here.
///
/// `null` rather than an error for the two cases that read the same to a person: a title this
/// account may not see, and an id that names nothing (I37). A route naming either opens a screen
/// that says the title is not here.
#[tauri::command]
#[specta::specta]
pub async fn get_title(
    state: State<'_, RuntimeState>,
    title: TitleRef,
) -> Result<Option<TitleDetailDto>, AppError> {
    let title = state.runtime()?.library_title(title.id).await?;
    Ok(guard_response(
        "library::get_title",
        title.map(title_detail),
    ))
}

/// The wire shape of one title's own read.
fn title_detail(detail: peregrine_app::LibraryTitleDetail) -> TitleDetailDto {
    TitleDetailDto {
        title: LibraryTitleDto::from(detail.title),
        // A path this platform cannot spell as UTF-8 is no reference at all: the webview would have
        // to guess, and a lossy path is a request the asset protocol refuses.
        poster: detail
            .poster
            .as_deref()
            .and_then(std::path::Path::to_str)
            .map(str::to_owned),
        refresh_due: detail.refresh_due,
    }
}

/// A title's episodes, in the order a person watches them.
#[tauri::command]
#[specta::specta]
pub async fn list_episodes(
    state: State<'_, RuntimeState>,
    title: TitleRef,
    limit: Option<u32>,
) -> Result<Vec<EpisodeDto>, AppError> {
    let episodes = state
        .runtime()?
        .title_episodes(title.id, limit.unwrap_or(500))
        .await?;
    let episodes = episodes.into_iter().map(EpisodeDto::from).collect();
    Ok(guard_response("library::list_episodes", episodes))
}

/// A page of the rules a user's corrections became, newest first.
#[tauri::command]
#[specta::specta]
pub async fn list_match_rules(
    state: State<'_, RuntimeState>,
    limit: Option<u32>,
) -> Result<Vec<MatchRuleDto>, AppError> {
    let rules = state.runtime()?.match_rules(limit.unwrap_or(100)).await?;
    let rules = rules.into_iter().map(MatchRuleDto::from).collect();
    Ok(guard_response("library::list_match_rules", rules))
}

/// Removes one rule, reporting whether it was there.
#[tauri::command]
#[specta::specta]
pub async fn forget_match_rule(
    state: State<'_, RuntimeState>,
    rule: MatchRuleRef,
) -> Result<bool, AppError> {
    let removed = state.runtime()?.forget_match_rule(rule.id).await?;
    Ok(guard_response("library::forget_match_rule", removed))
}

/// A page of the files a scan could not place, newest first.
#[tauri::command]
#[specta::specta]
pub async fn list_unmatched(
    state: State<'_, RuntimeState>,
    limit: Option<u32>,
) -> Result<Vec<UnmatchedFileDto>, AppError> {
    let files = state
        .runtime()?
        .unmatched_files(limit.unwrap_or(100))
        .await?;
    let files = files.into_iter().map(UnmatchedFileDto::from).collect();
    Ok(guard_response("library::list_unmatched", files))
}

/// Answers one parked file by placing it as the title its parse read.
///
/// Answers whether it placed anything: a file a scan reached while the row waited is already
/// indexed, and clearing the stale row is the whole of that answer rather than a failure.
#[tauri::command]
#[specta::specta]
pub async fn match_unmatched(
    state: State<'_, RuntimeState>,
    file: UnmatchedRef,
) -> Result<bool, AppError> {
    let placed = state
        .runtime()?
        .match_unmatched_file(file.id, file.as_title())
        .await?;
    Ok(guard_response("library::match_unmatched", placed))
}

/// Starts a scan of every library root.
///
/// Answers with the job a caller watches, which is the same shape `jobs::start_hello` answers with,
/// because it is the same thing: a journal row and the scheduler's identity for the work. A scan
/// asked for while one is running is refused with `Conflict` rather than queued behind it — the
/// walk is the expensive part and a queue of identical walks is not a feature.
#[tauri::command]
#[specta::specta]
pub async fn scan_now(state: State<'_, RuntimeState>) -> Result<JobStarted, AppError> {
    let started = state.runtime()?.start_library_scan().await?;
    Ok(guard_response(
        "library::scan_now",
        JobStarted::from(started),
    ))
}

/// Removes a library root, reporting whether it was there.
///
/// This is the command whose name is the most dangerous thing about it, so the answer is explicit:
/// removing a root stops it being scanned and removes nothing else. The files stay where they are,
/// the items and their watch progress stay in the library, and a file that comes back is the same
/// item rather than a new one (PG-E14-01, PG-E14-13).
#[tauri::command]
#[specta::specta]
pub async fn remove_root(state: State<'_, RuntimeState>, root: RootRef) -> Result<bool, AppError> {
    let removed = state.runtime()?.remove_library_root(root.id).await?;
    Ok(guard_response("library::remove_root", removed))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use peregrine_app::events::{Event, EventSink, EventSinkError};
    use peregrine_app::{Observability, Runtime, RuntimeConfig};
    use tauri::Manager as _;
    use tempfile::TempDir;

    use super::*;

    /// What the runtime answers for one title, with only the artwork varying.
    fn detail(poster: Option<std::path::PathBuf>) -> peregrine_app::LibraryTitleDetail {
        peregrine_app::LibraryTitleDetail {
            title: peregrine_app::LibraryTitleRow {
                title: peregrine_app::TitleIdentity {
                    id: 7,
                    kind: peregrine_app::TitleKind::Movie,
                    subtype: peregrine_app::TitleSubtype::Standard,
                    name: "Alien".to_owned(),
                    sort_name: "alien".to_owned(),
                    metadata_state: peregrine_app::MetadataState::Stub,
                    adult: false,
                },
                items: 1,
                files: 2,
                bytes: 4_000_000_000,
                missing_files: 1,
            },
            poster,
            refresh_due: true,
        }
    }

    /// The wire shape of the one read a title screen makes.
    ///
    /// Everything the screen acts on crosses here, and two of the three fields are decided by the
    /// core rather than by the screen: the poster is a path on this machine, and `refresh_due` is the
    /// rule for whether opening the page should leave it (AGENTS R7).
    #[test]
    fn a_title_read_carries_its_poster_path_and_the_core_s_rule() {
        let dto = title_detail(detail(Some(std::path::PathBuf::from(
            "/cache/images/abc.webp",
        ))));

        assert_eq!(dto.poster.as_deref(), Some("/cache/images/abc.webp"));
        assert!(
            dto.refresh_due,
            "the core's answer travels with the read rather than being recomputed by a screen"
        );
        assert_eq!(dto.title.id, 7);
        assert_eq!(dto.title.name, "Alien");
        assert_eq!(dto.title.kind, RootKind::Movies);
        assert_eq!(dto.title.items, 1);
        assert_eq!(dto.title.files, 2);
        assert_eq!(dto.title.bytes, 4_000_000_000);
        assert_eq!(dto.title.missing_files, 1);
        assert!(
            dto.title.stub,
            "a title no provider has filled in is a stub, and the flag travels with the read"
        );

        // No poster is no reference, which is the state the screen draws its placeholder for.
        assert_eq!(title_detail(detail(None)).poster, None);
    }

    /// A cache path this platform cannot spell as UTF-8 is reported as no poster rather than lossily.
    ///
    /// The path is the whole of what the screen gets, so a lossy one is a request the asset protocol
    /// refuses — the state that looks like a rendering bug rather than like a title with no artwork.
    #[cfg(unix)]
    #[test]
    fn a_poster_path_that_is_not_text_is_reported_as_absent() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt as _;

        let poster = std::path::PathBuf::from(OsStr::from_bytes(b"/cache/images/\xff\xfe.webp"));
        let dto = title_detail(detail(Some(poster)));

        assert_eq!(dto.poster, None);
    }

    /// A sink that discards every event: these tests are about the command surface, and the bus's
    /// own delivery is tested in the crate that owns it.
    struct SilentSink;

    impl EventSink for SilentSink {
        fn emit(&self, _event: &Event) -> Result<(), EventSinkError> {
            Ok(())
        }
    }

    /// Starts a runtime into a mock app and returns the app and a folder that can be added, so a
    /// command can be called the way Tauri calls it: with managed state injected.
    async fn served(directory: &TempDir) -> (tauri::App<tauri::test::MockRuntime>, String) {
        let app = tauri::test::mock_app();
        let config = RuntimeConfig::new(
            directory.path(),
            directory.path().join("artwork"),
            Arc::new(SilentSink),
        );
        let (subscriber, logging) = Observability::build(&config.log_config()).unwrap();
        drop(subscriber);
        let runtime = Runtime::start(config, logging).await.unwrap();
        app.manage(RuntimeState::for_test(Arc::new(runtime)));

        let folder = directory.path().join("Movies");
        fs::create_dir_all(&folder).unwrap();
        (app, folder.to_string_lossy().into_owned())
    }

    #[tokio::test]
    async fn a_root_can_be_added_listed_and_removed() {
        let directory = TempDir::new().unwrap();
        let (app, folder) = served(&directory).await;
        let state = app.state::<RuntimeState>();

        assert_eq!(list_roots(state.clone(), None).await.unwrap().len(), 0);

        let added = add_root(
            state.clone(),
            folder.clone(),
            RootKind::Movies,
            RootScanMode::Incremental,
        )
        .await
        .unwrap();
        assert_eq!(added.path, folder);
        assert_eq!(added.kind, RootKind::Movies);
        assert_eq!(added.scan_mode, RootScanMode::Incremental);
        assert_eq!(
            added.last_scan_at, None,
            "a root that was never scanned says so"
        );
        assert!(added.created_at > 0);

        let listed = list_roots(state.clone(), None).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0], added);

        assert!(
            remove_root(state.clone(), RootRef { id: added.id })
                .await
                .unwrap()
        );
        assert!(
            !remove_root(state.clone(), RootRef { id: added.id })
                .await
                .unwrap(),
            "removing it twice reports that the second one did nothing"
        );
        assert_eq!(list_roots(state.clone(), None).await.unwrap().len(), 0);

        crate::runtime::shutdown(app.handle()).await;
    }

    #[tokio::test]
    async fn a_folder_that_is_already_a_root_is_refused_with_a_reason() {
        let directory = TempDir::new().unwrap();
        let (app, folder) = served(&directory).await;
        let state = app.state::<RuntimeState>();
        add_root(
            state.clone(),
            folder.clone(),
            RootKind::Movies,
            RootScanMode::Incremental,
        )
        .await
        .unwrap();

        let refused = add_root(state.clone(), folder, RootKind::Shows, RootScanMode::Manual)
            .await
            .expect_err("the same folder twice is refused");

        assert_eq!(
            refused.code,
            crate::error::ErrorCode::Conflict,
            "a folder that is already a root is a conflict, not a bad request: {refused:?}"
        );
        assert_eq!(list_roots(state.clone(), None).await.unwrap().len(), 1);

        crate::runtime::shutdown(app.handle()).await;
    }

    #[tokio::test]
    async fn a_path_that_cannot_be_a_root_is_refused_without_writing_anything() {
        // The picker hands over text it has not validated, so a relative path has to be refused
        // here rather than half-written and discovered later by the scanner.
        let directory = TempDir::new().unwrap();
        let (app, _folder) = served(&directory).await;
        let state = app.state::<RuntimeState>();

        let refused = add_root(
            state.clone(),
            "Movies".to_owned(),
            RootKind::Movies,
            RootScanMode::Incremental,
        )
        .await
        .expect_err("a relative path cannot be a root");

        assert_eq!(
            refused.code,
            crate::error::ErrorCode::InvalidInput,
            "{refused:?}"
        );
        assert_eq!(list_roots(state.clone(), None).await.unwrap().len(), 0);

        crate::runtime::shutdown(app.handle()).await;
    }

    #[tokio::test]
    async fn removing_a_root_leaves_the_folder_and_its_contents_alone() {
        // The command's answer is a bare `bool`, so the claim its documentation makes — "this
        // removes a configuration entry and nothing else" — is asserted against a filesystem.
        let directory = TempDir::new().unwrap();
        let (app, folder) = served(&directory).await;
        let state = app.state::<RuntimeState>();
        let film = directory.path().join("Movies/Alien (1979).mkv");
        fs::write(&film, b"a film").unwrap();
        let added = add_root(
            state.clone(),
            folder,
            RootKind::Movies,
            RootScanMode::Incremental,
        )
        .await
        .unwrap();

        remove_root(state.clone(), RootRef { id: added.id })
            .await
            .unwrap();

        assert!(film.is_file(), "the user's files are theirs");
        crate::runtime::shutdown(app.handle()).await;
    }

    #[tokio::test]
    async fn a_command_with_no_running_runtime_reports_that_rather_than_panicking() {
        // Every command shares this path: `RuntimeState::failed` is the state startup leaves behind
        // (PG-E2-02), and a command that unwrapped here would take the window with it.
        let app = tauri::test::mock_app();
        app.manage(RuntimeState::failed("the database could not be opened"));
        let state = app.state::<RuntimeState>();

        let refused = list_roots(state, None)
            .await
            .expect_err("no runtime, no roots");

        assert!(
            !refused.message.is_empty(),
            "and the failure says something a user can read: {refused:?}"
        );
    }

    #[tokio::test]
    async fn roots_are_listed_newest_first() {
        // Two roots, listed newest first: the order the screen shows them in, and the order a
        // cursor would walk when the list is long enough to need one.
        let directory = TempDir::new().unwrap();
        let (app, _folder) = served(&directory).await;
        let state = app.state::<RuntimeState>();
        for name in ["Movies", "Shows"] {
            let folder = directory.path().join(name);
            fs::create_dir_all(&folder).unwrap();
            add_root(
                state.clone(),
                folder.to_string_lossy().into_owned(),
                RootKind::Movies,
                RootScanMode::Incremental,
            )
            .await
            .unwrap();
        }

        let listed = list_roots(state.clone(), Some(10)).await.unwrap();

        assert_eq!(listed.len(), 2);
        assert!(
            listed[0].id > listed[1].id,
            "newest first, which is the order the screen shows them in"
        );

        crate::runtime::shutdown(app.handle()).await;
    }
}
