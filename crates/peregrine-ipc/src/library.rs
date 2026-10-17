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
///
/// **The poster travels with the row, and that is why the grid and the table are one read.** The screen
/// draws this read twice — a table of counts and a grid of tiles — and a tile cannot resolve the cache
/// key its poster is stored under, so the path is resolved by the core for every row (AGENTS R7). It is
/// a path and never bytes: `apps/desktop/src/lib/media.ts` is the only module that names the asset
/// protocol (AGENTS R11). `null` covers both "no provider has named one" and "one was named and the blob
/// is not here", which draw the same reserved box.
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
    /// The absolute path of the cached poster, or `null` when the cache holds none.
    ///
    /// A path this platform cannot spell as UTF-8 is reported as `null` rather than lossily: the value
    /// is the whole of what the webview gets, and a corrupted path is a request the asset protocol
    /// refuses — the state that looks like a rendering bug rather than like a title with no artwork.
    pub poster: Option<String>,
    /// Whether this title has no poster, so a frame of its own file is worth capturing.
    ///
    /// The core's answer, for the reason every availability decision is (AGENTS R7): it is "no provider has
    /// drawn one *and* no frame has been generated", which needs the catalogue row and two cache lookups —
    /// and a grid that decided it from `poster === null` would be wrong for a title whose poster exists but
    /// whose blob an eviction took.
    pub needs_poster: bool,
}

impl From<peregrine_app::LibraryTitleEntry> for LibraryTitleDto {
    fn from(entry: peregrine_app::LibraryTitleEntry) -> Self {
        let row = entry.row;
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
            poster: artwork_text(entry.poster.as_deref()),
            needs_poster: entry.needs_poster,
        }
    }
}

/// A resolved artwork path as the wire spells it, or `None` when the platform cannot spell it as text.
///
/// One function rather than a `.and_then(Path::to_str)` per field, because the rule is the same
/// everywhere and the failure it prevents is silent: a lossy path is a request the asset protocol
/// refuses, which renders as a missing poster and reports nothing.
pub(crate) fn artwork_text(path: Option<&std::path::Path>) -> Option<String> {
    path.and_then(std::path::Path::to_str).map(str::to_owned)
}

/// What a title's own page may offer, as the webview spells it.
///
/// The wire shape of `peregrine_domain::TitleOffer`, and its own type for the reason every DTO here
/// is: the domain crate cannot depend on `specta` (AGENTS R6), and a domain value renamed should not
/// silently rename a word the webview matches on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum TitleOfferDto {
    /// A copy is on disk, so there is something to play.
    Play,
    /// An acquisition for this title is in flight.
    Arriving,
    /// File rows exist and no copy of any of them is present.
    Absent,
    /// Nothing is held and nothing is arriving, so acquisition is what is left to offer.
    Acquire,
}

impl From<peregrine_app::TitleOffer> for TitleOfferDto {
    fn from(offer: peregrine_app::TitleOffer) -> Self {
        match offer {
            peregrine_app::TitleOffer::Play => Self::Play,
            peregrine_app::TitleOffer::Arriving => Self::Arriving,
            peregrine_app::TitleOffer::Absent => Self::Absent,
            peregrine_app::TitleOffer::Acquire => Self::Acquire,
        }
    }
}

/// One title as its own screen reads it: the library's answer, plus what a provider could still add.
///
/// Nested rather than a wider [`LibraryTitleDto`] because the two answer different questions and are
/// read by different screens: the list is "what the library holds", and this is "what the library
/// holds, and what a provider could still add". One DTO cannot mean two things — and the poster is on
/// the nested one rather than here for exactly that reason: it is a fact about the title the library
/// holds, so it is the same field on every screen that draws the title, and a second copy beside the
/// nested row would be a second value that could disagree with it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TitleDetailDto {
    /// The title itself, exactly as the library screen lists it, its cached poster included.
    pub title: LibraryTitleDto,
    /// The absolute path of the cached backdrop, or `null` when the cache holds none.
    ///
    /// The wide picture a hero draws behind the title, carried on the same terms as the poster: a path
    /// and never bytes (AGENTS R11), and `null` for a path this platform cannot spell as UTF-8.
    /// Deliberately not on [`LibraryTitleDto`]: no list draws a backdrop.
    pub backdrop: Option<String>,
    /// The provider's synopsis, when one has been written.
    ///
    /// `null` is the ordinary state of a title no provider has filled in, and the hero draws no
    /// paragraph for it rather than an empty one.
    pub overview: Option<String>,
    /// The provider's score, when it has one.
    pub rating: Option<f64>,
    /// When it came out, as `YYYY-MM-DD`, from whichever date column its kind uses.
    ///
    /// The core's rendering of the stored column rather than a date on the wire: which of the two
    /// columns holds it is a fact about the schema, and a screen that chose between them would be
    /// repeating a rule the `SQL` already expresses.
    pub released: Option<String>,
    /// Whether opening this screen should ask the provider for the title.
    ///
    /// **A rule, and computed in the core for that reason** (AGENTS R7): it is "no provider has ever
    /// filled this title in, or a provider is known to have artwork the cache does not hold". A
    /// screen that decided this for itself would spend a request on every visit to a title whose
    /// provider has no poster, which is the shape AGENTS R24 exists to prevent. The *staleness*
    /// policy — when an already-filled title is due again — is `PG-E6-15`'s and is not this flag.
    pub refresh_due: bool,
    /// What this title's page may offer, which is the core's rule rather than a screen's.
    ///
    /// The one field a Download control reads: `acquire` is the only value that offers acquisition,
    /// and `play` is the only one that offers playback, so a title the library holds cannot be
    /// offered for download however its counts read. See [`TitleOfferDto`].
    pub offer: TitleOfferDto,
    /// What pressing Play would actually do, or `null` when the library holds no copy on disk.
    ///
    /// **The reason Play can say why it will not work** rather than being inert with no explanation: a
    /// `<video>` element reports a failure *code* after it has failed, never a reason, and "this build
    /// cannot read that container yet" is a fact about the file that has to be known before the press
    /// (`ADR 0023 §5`). `null` and `unknown` mean different things: the first is nothing to play, the
    /// second is a file whose container this build cannot name.
    pub play_support: Option<LocalPlaybackDto>,
}

/// What this build will do with a local file, as the webview spells it.
///
/// The wire shape of `peregrine_domain::LocalPlayback`, and a separate type for the reason every DTO
/// here is: the domain cannot depend on `specta` (AGENTS R6), and a domain value renamed should not
/// silently rename a word the webview switches on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum LocalPlaybackDto {
    /// A container the webview's own media stack demuxes, so it plays with no sidecar.
    Webview,
    /// A container the webview does not demux, so this build cannot play it until mpv ships.
    ExternalPlayer,
    /// A name this build does not recognise as a media container at all.
    Unknown,
}

impl From<peregrine_app::LocalPlayback> for LocalPlaybackDto {
    fn from(support: peregrine_app::LocalPlayback) -> Self {
        match support {
            peregrine_app::LocalPlayback::Webview => Self::Webview,
            peregrine_app::LocalPlayback::ExternalPlayer => Self::ExternalPlayer,
            peregrine_app::LocalPlayback::Unknown => Self::Unknown,
        }
    }
}

/// The session a stop names.
///
/// **A stop that did not name one would close whatever is current**, and a renderer's effect runs twice
/// under `StrictMode`: the first mount's cleanup and the second mount's open are in flight together, so a
/// stop by "current" closes the session the screen is already reading. That produced a film that made one
/// request and then sat behind a black rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SessionRef {
    /// The session's id, as opening it returned it.
    #[specta(type = Number)]
    pub id: u64,
}

/// One open playback session: what is playing, and where the bytes come from.
///
/// **A URL and not a path**, and that is the surface `ADR 0023` decides: the asset protocol never
/// exposes the library root, so a local file is served over loopback HTTP and this is the address the
/// renderer's media element follows (AGENTS R11). The session token is inside the URL rather than a
/// field beside it, because a `<video>` element cannot set a header — one value, one place it can go
/// wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackSessionDto {
    /// Which session this is, and what a stop must name.
    #[specta(type = Number)]
    pub session_id: u64,
    /// The title this session belongs to, so a screen can confirm it is drawing the one it asked for.
    #[specta(type = Number)]
    pub title_id: i64,
    /// The library file being served.
    #[specta(type = Number)]
    pub media_file_id: i64,
    /// What this build is doing with the file's container.
    pub support: LocalPlaybackDto,
    /// The loopback URL a media element follows.
    pub url: String,
    /// The loopback URL a frame the renderer captured is posted to, or `null` when this session keeps no
    /// poster.
    ///
    /// **A second URL rather than something the renderer derives from the first.** Both carry the session
    /// token, and a renderer that swapped one path segment for another would be a second place the route
    /// is spelled — which is exactly the kind of drift a bearer credential makes expensive.
    pub thumbnail_url: Option<String>,
    /// Whether the renderer should capture a poster frame while it plays.
    ///
    /// The core's answer, and it is `true` only for a title with no poster to draw: capturing costs a
    /// decode of the film and a blob in the cache, and its whole purpose is the title a provider has drawn
    /// nothing for. A renderer that captured on every play would overwrite real artwork with a film still
    /// the first time a provider's poster was evicted.
    pub needs_poster: bool,
    /// Where the account left off, in milliseconds, or `null` when it never started this title.
    ///
    /// On the session rather than behind a second command because the player already holds this
    /// value's carrier, and a follow-up call would race the frame it is trying to position.
    #[specta(type = Option<Number>)]
    pub resume_position_ms: Option<i64>,
}

impl From<peregrine_app::StartedPlayback> for PlaybackSessionDto {
    fn from(started: peregrine_app::StartedPlayback) -> Self {
        Self {
            session_id: started.session_id.get(),
            title_id: started.title_id,
            media_file_id: started.media_file_id,
            support: LocalPlaybackDto::from(started.support),
            url: started.url,
            thumbnail_url: started.thumbnail_url,
            needs_poster: started.needs_poster,
            resume_position_ms: started.resume_position_ms,
        }
    }
}

/// One open capture session: where the renderer reads a file and where it posts the frame it drew.
///
/// The same shape as [`PlaybackSessionDto`] and a separate one, because the two answer different
/// questions: nothing here is about how a film would play, and a caller that wanted only a frame should
/// not be handed a container classification it has no use for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CaptureSessionDto {
    /// Which session this is, and what a stop must name.
    #[specta(type = Number)]
    pub session_id: u64,
    /// The title the frame is a poster of, so a caller can confirm it is captioned by the right title.
    #[specta(type = Number)]
    pub title_id: i64,
    /// The library file being served to the capture.
    #[specta(type = Number)]
    pub media_file_id: i64,
    /// The loopback URL the capturing element loads.
    pub url: String,
    /// Where the frame is posted, or `null` when this session keeps no poster.
    pub thumbnail_url: Option<String>,
}

impl From<peregrine_app::StartedCapture> for CaptureSessionDto {
    fn from(started: peregrine_app::StartedCapture) -> Self {
        Self {
            session_id: started.session_id.get(),
            title_id: started.title_id,
            media_file_id: started.media_file_id,
            url: started.url,
            thumbnail_url: started.thumbnail_url,
        }
    }
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

/// One episode of a title, as its card draws it.
///
/// **Widened from a totals row to a card.** The first shape carried the two numbers, the name, and four
/// counts, because the only surface in front of it was a table of what the library holds. A card shows
/// a still, an air date, a runtime, a score and a synopsis, so those are here — and every one of them
/// is optional, because an episode the library holds and no provider has described is the ordinary
/// state of a fresh scan and a card must draw it rather than refuse it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
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
    /// The provider's synopsis, when it sent one.
    pub overview: Option<String>,
    /// When it first aired, as `YYYY-MM-DD`, when the core can read the stored date.
    ///
    /// A string rather than a parsed date on the wire: the screen prints it, and the one place that
    /// knows how the column is spelled is the core.
    pub air_date: Option<String>,
    /// How long it runs, when the provider stated one.
    #[specta(type = Option<Number>)]
    pub runtime_minutes: Option<u32>,
    /// The provider's score for this episode alone, when it has one.
    pub rating: Option<f64>,
    /// The cached still's absolute path, or `null` when the cache holds none.
    ///
    /// A path and never bytes (AGENTS R11): the webview turns it into an `asset:` URL through
    /// `lib/media.ts`, which is the only module that names the protocol.
    pub still: Option<String>,
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

impl From<peregrine_app::TitleEpisodeDetail> for EpisodeDto {
    fn from(detail: peregrine_app::TitleEpisodeDetail) -> Self {
        let row = detail.row;
        Self {
            season_number: row.season_number.get(),
            episode_number: row.episode_number.get(),
            name: row.name,
            overview: row.overview,
            air_date: row.air_date.map(|date| date.to_string()),
            runtime_minutes: row.runtime_minutes,
            rating: row.rating,
            // A path this platform cannot spell as UTF-8 is no reference at all, for the reason the
            // title's poster states: a lossy path is a request the asset protocol refuses, which looks
            // like a rendering bug rather than like an episode with no still.
            still: detail
                .still
                .as_deref()
                .and_then(std::path::Path::to_str)
                .map(str::to_owned),
            items: row.items,
            files: row.files,
            bytes: row.bytes,
            missing_files: row.missing_files,
        }
    }
}

/// One season of a title, as its switcher draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SeasonDto {
    /// Its number, which zero is for a show's specials.
    #[specta(type = Number)]
    pub season_number: u32,
    /// Its name, when a provider sync has filled one in.
    pub name: Option<String>,
    /// How many episodes the provider says it has, when it said.
    ///
    /// The provider's count rather than a count of rows this build holds, for the reason the storage
    /// row states: a season nobody has fetched yet has a number, and a switcher reading `0` for it
    /// would offer a season that looks empty when it has simply not been read.
    #[specta(type = Option<Number>)]
    pub episode_count: Option<u32>,
    /// When it first aired, as `YYYY-MM-DD`, when the core can read the stored date.
    pub air_date: Option<String>,
    /// The cached poster's absolute path, or `null` when the cache holds none.
    pub poster: Option<String>,
}

impl From<peregrine_app::TitleSeasonDetail> for SeasonDto {
    fn from(detail: peregrine_app::TitleSeasonDetail) -> Self {
        let row = detail.row;
        Self {
            season_number: row.season_number.get(),
            name: row.name,
            episode_count: row.episode_count,
            air_date: row.air_date.map(|date| date.to_string()),
            poster: detail
                .poster
                .map(|path| path.to_string_lossy().into_owned()),
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

/// Starts playing one of a title's files, replacing whatever was playing.
///
/// The one command whose answer is a URL. It leaves the machine nowhere: the address it answers with is
/// this process's own loopback listener, opened for one file and closed when the session ends
/// (`ADR 0023`, SECURITY.md T10).
///
/// # Errors
///
/// [`AppError`] when the title is not here, holds no copy on disk, holds a copy outside every configured
/// folder, or the file cannot be served.
#[tauri::command]
#[specta::specta]
pub async fn start_playback(
    state: State<'_, RuntimeState>,
    title: TitleRef,
) -> Result<PlaybackSessionDto, AppError> {
    let started = state.runtime()?.start_playback(title.id).await?;
    Ok(guard_response(
        "library::start_playback",
        PlaybackSessionDto::from(started),
    ))
}

/// Plays one title's file in the external player (mpv), for a container the webview cannot demux.
///
/// One command rather than a flag on `start_playback`: the bytes never cross this process, and a
/// caller that could not tell the two apart would give the wrong one the wrong error path.
///
/// # Errors
///
/// [`AppError`] when the title is not here, holds no copy on disk, holds a copy outside every
/// configured folder, or when mpv is not installed.
#[tauri::command]
#[specta::specta]
pub async fn play_externally(
    state: State<'_, RuntimeState>,
    title: TitleRef,
) -> Result<(), AppError> {
    state.runtime()?.start_external_playback(title.id).await?;
    Ok(guard_response("library::play_externally", ()))
}

/// Opens a session for one title's file so the renderer can capture a poster frame from it.
///
/// **Not `start_playback` with a flag, and the name is the reason.** Both open the same listener for the
/// same file, and the difference is intent: nothing plays a capture session, and a reader who found
/// `start_playback` called by a poster backfill could not tell whether the film was meant to be playing. It
/// holds its own slot, so a backfill cannot stop a film and a film cannot stop a backfill.
///
/// # Errors
///
/// [`AppError`] when the title is not here, holds no copy on disk, holds a copy outside every configured
/// folder, or the file cannot be served.
#[tauri::command]
#[specta::specta]
pub async fn start_poster_capture(
    state: State<'_, RuntimeState>,
    title: TitleRef,
) -> Result<CaptureSessionDto, AppError> {
    let started = state.runtime()?.start_poster_capture(title.id).await?;
    Ok(guard_response(
        "library::start_poster_capture",
        CaptureSessionDto::from(started),
    ))
}

/// Stops the capture session `session` names, answering whether that one was open.
#[tauri::command]
#[specta::specta]
pub async fn stop_poster_capture(
    state: State<'_, RuntimeState>,
    session: SessionRef,
) -> Result<bool, AppError> {
    let stopped = state
        .runtime()?
        .stop_poster_capture(peregrine_app::SessionId::from_raw(session.id))
        .await;
    Ok(guard_response("library::stop_poster_capture", stopped))
}

/// Stops the playback session `session` names, answering whether that one was open.
///
/// Deliberately infallible: there is no state in which a caller asking for silence should be told no, and a
/// screen that has just lost its session needs an answer rather than an error to report.
#[tauri::command]
#[specta::specta]
pub async fn stop_playback(
    state: State<'_, RuntimeState>,
    session: SessionRef,
) -> Result<bool, AppError> {
    let stopped = state
        .runtime()?
        .stop_playback(peregrine_app::SessionId::from_raw(session.id))
        .await;
    Ok(guard_response("library::stop_playback", stopped))
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
        title: LibraryTitleDto::from(detail.entry),
        // A path this platform cannot spell as UTF-8 is no reference at all: the webview would have
        // to guess, and a lossy path is a request the asset protocol refuses.
        backdrop: artwork_text(detail.backdrop.as_deref()),
        overview: detail.overview,
        rating: detail.rating,
        released: detail.released.map(|date| date.to_string()),
        refresh_due: detail.refresh_due,
        offer: TitleOfferDto::from(detail.playback.offer),
        play_support: detail.playback.support.map(LocalPlaybackDto::from),
    }
}

/// A title's episodes, in the order a person watches them.
#[tauri::command]
#[specta::specta]
pub async fn list_episodes(
    state: State<'_, RuntimeState>,
    title: TitleRef,
    season: Option<u32>,
    limit: Option<u32>,
) -> Result<Vec<EpisodeDto>, AppError> {
    let episodes = state
        .runtime()?
        .title_episodes(title.id, season, limit.unwrap_or(500))
        .await?;
    let episodes = episodes.into_iter().map(EpisodeDto::from).collect();
    Ok(guard_response("library::list_episodes", episodes))
}

/// The seasons a title has, in the order its switcher lists them.
#[tauri::command]
#[specta::specta]
pub async fn list_seasons(
    state: State<'_, RuntimeState>,
    title: TitleRef,
    limit: Option<u32>,
) -> Result<Vec<SeasonDto>, AppError> {
    let seasons = state
        .runtime()?
        .title_seasons(title.id, limit.unwrap_or(100))
        .await?;
    let seasons = seasons.into_iter().map(SeasonDto::from).collect();
    Ok(guard_response("library::list_seasons", seasons))
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
            entry: peregrine_app::LibraryTitleEntry {
                row: peregrine_app::LibraryTitleRow {
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
                    // The provider's own reference, which the list row now carries: the fixture states it
                    // because it is what a grid's tile is resolved *from*, and a fixture that left it out
                    // could not tell a read that resolved the poster from one that invented it.
                    poster: Some(peregrine_app::ArtworkPath {
                        provider: peregrine_app::Provider::Tmdb,
                        path: "/poster.jpg".to_owned(),
                    }),
                },
                poster,
                // False, because the fixture above states a provider reference: a title a provider has
                // drawn a poster for is not one a capture improves on.
                needs_poster: false,
            },
            // Absent, because this fixture is about the poster's path: the hero's own fields are read
            // by their own test, and setting them here would make one fixture answer two questions.
            backdrop: None,
            overview: None,
            rating: None,
            released: None,
            refresh_due: true,
            // Playable, because the counts above say the library holds a present copy — and the offer
            // is the *core's* answer to that rather than the counts' own, which is the reason this
            // fixture sets it rather than leaving it to the reader. The support is what would happen on
            // the press, and an MP4 is the case the whole slice exists for.
            playback: peregrine_app::TitlePlayback {
                offer: peregrine_app::TitleOffer::Play,
                support: Some(peregrine_app::LocalPlayback::Webview),
            },
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

        // The poster is the nested row's, and there is deliberately no second copy beside it: one fact
        // on the wire is one value that cannot disagree with itself.
        assert_eq!(dto.title.poster.as_deref(), Some("/cache/images/abc.webp"));
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
        assert_eq!(
            dto.offer,
            TitleOfferDto::Play,
            "the offer travels as the core derived it: a held copy is playable and is never offered \
             for download"
        );

        // No poster is no reference, which is the state the screen draws its placeholder for.
        assert_eq!(title_detail(detail(None)).title.poster, None);
    }

    /// The offer's four values cross as the words the webview switches on, and the mapping from the
    /// domain's own enum is total — a new value in the core is a compile error here rather than a
    /// page that silently reads it as something else.
    #[test]
    fn every_offer_crosses_as_its_own_word() {
        for offer in peregrine_app::TitleOffer::ALL {
            let dto = TitleOfferDto::from(offer);
            assert_eq!(
                serde_json::to_value(dto).expect("serializes"),
                serde_json::Value::String(offer.as_str().to_owned())
            );
        }
        let mut fixture = detail(None);
        fixture.playback.offer = peregrine_app::TitleOffer::Acquire;
        assert_eq!(
            title_detail(fixture).offer,
            TitleOfferDto::Acquire,
            "a title nothing is held of is offered for acquisition"
        );
    }

    /// The wire shape of the episode and season reads a title page makes.
    ///
    /// Three things cross here that a screen cannot decide for itself: the still and the season poster
    /// are paths on this machine (AGENTS R11), the air date is the core's rendering of a stored column,
    /// and a runtime the column cannot hold is absent rather than wrapped.
    #[test]
    fn an_episode_read_carries_its_still_its_date_and_its_runtime() {
        let dto = EpisodeDto::from(peregrine_app::TitleEpisodeDetail {
            row: peregrine_app::TitleEpisodeRow {
                season_number: peregrine_app::SeasonNumber::new(1),
                episode_number: peregrine_app::EpisodeNumber::new(3),
                name: Some("Misplaced Redemption".to_owned()),
                overview: Some("Prose.".to_owned()),
                air_date: Some("2026-10-08".parse().expect("a fixture date")),
                runtime_minutes: Some(24),
                rating: Some(8.1),
                still: Some(peregrine_app::ArtworkPath {
                    provider: peregrine_app::Provider::Tmdb,
                    path: "/still.jpg".to_owned(),
                }),
                items: 2,
                files: 3,
                bytes: 4_000,
                missing_files: 1,
            },
            still: Some(std::path::PathBuf::from("/cache/images/still.webp")),
        });

        assert_eq!(dto.still.as_deref(), Some("/cache/images/still.webp"));
        assert_eq!(dto.air_date.as_deref(), Some("2026-10-08"));
        assert_eq!(dto.runtime_minutes, Some(24));
        assert_eq!(dto.rating, Some(8.1));
        assert_eq!(dto.name.as_deref(), Some("Misplaced Redemption"));
        assert_eq!(dto.items, 2);
        assert_eq!(dto.missing_files, 1);

        // **No still is no reference**, which is the reserved box a card draws — and it is the state an
        // episode the library holds and no provider has described arrives in, so it cannot be an error.
        let bare = EpisodeDto::from(peregrine_app::TitleEpisodeDetail {
            row: peregrine_app::TitleEpisodeRow {
                season_number: peregrine_app::SeasonNumber::new(1),
                episode_number: peregrine_app::EpisodeNumber::new(4),
                name: None,
                overview: None,
                air_date: None,
                runtime_minutes: None,
                rating: None,
                still: None,
                items: 0,
                files: 0,
                bytes: 0,
                missing_files: 0,
            },
            still: None,
        });

        assert_eq!(bare.still, None);
        assert_eq!(bare.air_date, None);
        assert_eq!(bare.runtime_minutes, None);
        assert_eq!(bare.name, None);
        assert_eq!(
            bare.items, 0,
            "zero items is a fact rather than a missing value"
        );
    }

    /// A season's own read, which is what the switcher draws.
    #[test]
    fn a_season_read_carries_its_count_its_date_and_its_poster() {
        let dto = SeasonDto::from(peregrine_app::TitleSeasonDetail {
            row: peregrine_app::TitleSeasonRow {
                season_number: peregrine_app::SeasonNumber::new(4),
                name: Some("Season 4".to_owned()),
                episode_count: Some(11),
                air_date: Some("2026-04-30".parse().expect("a fixture date")),
                poster: Some(peregrine_app::ArtworkPath {
                    provider: peregrine_app::Provider::Tmdb,
                    path: "/season.jpg".to_owned(),
                }),
            },
            poster: Some(std::path::PathBuf::from("/cache/images/season.webp")),
        });

        assert_eq!(dto.season_number, 4);
        assert_eq!(dto.name.as_deref(), Some("Season 4"));
        assert_eq!(
            dto.episode_count,
            Some(11),
            "the provider's count rather than a count of rows the library holds"
        );
        assert_eq!(dto.air_date.as_deref(), Some("2026-04-30"));
        assert_eq!(dto.poster.as_deref(), Some("/cache/images/season.webp"));
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

        assert_eq!(dto.title.poster, None);
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
        )
        // A command test never reaches the developer's keychain (`AGENTS R14`): a real read from a
        // freshly built binary raises a macOS access prompt and waits on a person.
        .with_secrets(Arc::new(crate::test_support::NoCredentials));
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
