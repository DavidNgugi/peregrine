//! The Phase 0 runtime: the composition root that connects logging, storage, and the
//! scheduler, and the one place a job is started (PG-E0-19).
//!
//! Everything below this module is a library that can be built and tested on its own;
//! nothing above it — the IPC command surface and the desktop shell — is allowed to
//! assemble services itself. The runtime is therefore the only thing that knows the
//! database's path, which scheduler a job runs on, and where the logs are.
//!
//! Startup order is the design, not a detail: the log directory is opened by the
//! composition root first, because every later failure has to be reportable; the database
//! is opened second, so a migration failure stops the app before any job can be accepted;
//! and the event bus is last, so nothing is published into a bus whose sink is not ready.

use std::collections::{BTreeMap, HashMap};
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::Duration;

use std::num::NonZeroU32;

use crate::notify::{CataloguePreferences, NoOsNotifier, NotificationService, NotifyOffers};
use peregrine_domain::centre::{CentreEntry, CentreError, NotificationCentre as _, NotificationId};
use peregrine_domain::notify::{Focus, NotificationEvent, Platform};
use peregrine_domain::text::NonEmptyText;
use peregrine_domain::{
    AccountScope, Clock, IdGenerator, LibraryRoot, LibraryRootKind, LocalPlayback,
    PlaybackPosition, PreferredQuality, ProgressSource, SafePath, ScanMode, TokenSource,
    WatchProgress,
};
use peregrine_platform::{SystemClock, SystemTrash, TokenMinter, UlidGenerator};
use peregrine_secrets::{Secret, SecretStore};
use peregrine_storage::NotificationRepository;
use peregrine_storage::{
    Cursor, Database, JobChange, JobPhase, JobRecord, NewJob, PageRequest, StorageError,
    WatchProgressRow,
};
use peregrine_sync::{
    JobHandle, JobSpec, Scheduler, SchedulerConfig, SchedulerError, SchedulerMetrics,
};
use peregrine_torrent::{EngineLimits, LibrqbitEngine, TorrentEngine, TorrentMetadataFetch};
use serde_json::json;

use crate::acquisition::AcquisitionService;
use crate::jobs::library_scan::{self, LibraryScanJob};
use crate::jobs::provider_credential;
use crate::library::{LibraryRootRow, LibraryService};
use crate::providers::BuildError;
use crate::volume::SystemVolume;
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};
use tokio::time::Instant;
use tracing::{info, warn};
use ulid::Ulid;

use crate::events::{Event, EventBus, EventSink, EventStats, NOTIFICATION_RAISED};
use crate::jobs::hello::{self, HelloJob};
use crate::jobs::{CANCELLED_CODE, INTERNAL_CODE};
use crate::observability::{LogConfig, Observability};

/// The scope every user-facing read in this build runs under.
///
/// Accounts are `PG-E35`'s epic, and the migrations seed no account row: a fresh installation has
/// nothing to select between, so there is no account to resolve. What a scope *carries* is a
/// ceiling and a curation flag, and this build has neither — so the scope is unrestricted, and the
/// account id it names is inert for both predicates an unrestricted scope applies (`hidden_by_scope`
/// is zero, which turns the predicate into a no-op; `peregrine-storage`'s own tests have relied on
/// exactly that since the first scoped read).
///
/// **When the accounts epic lands, this becomes a resolution rather than a constant:** the runtime
/// reads the default account and builds the scope from its `rating_ceiling` and `curated_only`, and
/// every call site that passes this keeps passing a scope — which is why the constant lives here
/// with the reason rather than at each call site with none.
#[must_use]
pub fn installation_scope() -> peregrine_domain::AccountScope {
    peregrine_domain::AccountScope::unrestricted(
        peregrine_domain::AccountId::new(1).expect("account one is a positive id"),
    )
}

/// The database file inside the data directory.
const DATABASE_FILE: &str = "peregrine.sqlite";

/// The log directory inside the data directory.
const LOG_DIRECTORY: &str = "logs";

/// The directory downloads are written to when no root has been configured for them.
const DOWNLOAD_DIRECTORY: &str = "downloads";

/// The room the admission floor reserves on the download volume.
///
/// A settings-registry entry (`PG-E20-01`) will own it; until then it is one constant here rather
/// than a number spelled at a call site with none of the reasoning.
const FREE_SPACE_FLOOR: u64 = 5 * 1024 * 1024 * 1024;

/// The directives the runtime logs with when the caller does not choose.
const DEFAULT_DIRECTIVES: &str = "info";

/// The most log lines one tail request may return.
pub const MAX_LOG_LINES: u32 = 500;

/// The most bytes of log file a tail request reads (the IPC payload budget, × 1 KiB).
const LOG_TAIL_BYTES: u64 = 64 * 1024;

/// The prefix and suffix `peregrine-app`'s rotated files are named with.
const LOG_FILE_PREFIX: &str = "peregrine.";
const LOG_FILE_SUFFIX: &str = ".jsonl";

/// How the runtime is wired before it is started.
///
/// The sink is handed in rather than constructed here: the Tauri emitter lives in
/// `peregrine-ipc`, which is above this crate, and a test wants a sink it can read.
pub struct RuntimeConfig {
    data_directory: PathBuf,
    /// Where cached artwork is written, which is the one directory the shell's asset protocol
    /// serves (PG-E2-08).
    ///
    /// Handed in rather than derived, and deliberately: the directory used to be derived from the
    /// data directory while the protocol was scoped to the OS cache directory, so every blob the
    /// cache wrote was written where nothing could serve it. `PG-E6-09` states the contract from the
    /// other side — "the directory it lives in is the one the shell's asset protocol is scoped to,
    /// and nothing else may write there" — and a second derivation here would be a second answer.
    artwork_directory: PathBuf,
    sink: Arc<dyn EventSink>,
    directives: String,
    tray_enabled: bool,
    /// Where provider credentials are kept.
    ///
    /// Injected rather than constructed inside [`Runtime::start`] so a test never reaches the
    /// developer's own keychain, and never raises an OS prompt for it (AGENTS R14). The default is
    /// the real one; [`RuntimeConfig::with_secrets`] is what a test replaces.
    secrets: Arc<dyn SecretStore>,
    /// The external player. Injected for the same reason: the real one opens a window, and a unit
    /// test must not; [`RuntimeConfig::with_player`] is what a test replaces.
    player: Arc<dyn peregrine_domain::PlayerBackend>,
}

impl RuntimeConfig {
    /// Builds a configuration for `data_directory`, writing cached artwork under
    /// `artwork_directory` and delivering events to `sink`.
    #[must_use]
    pub fn new(
        data_directory: impl Into<PathBuf>,
        artwork_directory: impl Into<PathBuf>,
        sink: Arc<dyn EventSink>,
    ) -> Self {
        Self {
            data_directory: data_directory.into(),
            artwork_directory: artwork_directory.into(),
            sink,
            directives: DEFAULT_DIRECTIVES.to_owned(),
            tray_enabled: true,
            secrets: Arc::new(peregrine_secrets::KeyringStore::new()),
            player: Arc::new(peregrine_playback::MpvPlayer::new()),
        }
    }

    /// Replaces the log filter directives.
    #[must_use]
    pub fn with_directives(mut self, directives: impl Into<String>) -> Self {
        self.directives = directives.into();
        self
    }

    /// Replaces where provider credentials are kept.
    ///
    /// **The reason this is a parameter is a test.** The default is the OS keyring, and a test that
    /// used it would read and write the developer's own login keychain — and, on macOS, raise an OS
    /// prompt that measured 11.7 s on its first call from a new binary (AGENTS R14, PG-E5-12).
    #[must_use]
    pub fn with_secrets(mut self, secrets: Arc<dyn SecretStore>) -> Self {
        self.secrets = secrets;
        self
    }

    /// Turns the shell's tray icon on or off.
    ///
    /// The user's answer will come from the settings registry (PG-E20-01); until that
    /// exists the runtime is built with the default, and the off path is a value a test
    /// can set rather than a branch nobody has run (PG-E1-09's "hidden when disabled").
    #[must_use]
    pub fn with_tray_enabled(mut self, tray_enabled: bool) -> Self {
        self.tray_enabled = tray_enabled;
        self
    }

    /// Uses an explicit mpv binary, which is how a bundled sidecar wins over `PATH`.
    #[must_use]
    pub fn with_player_binary(mut self, binary: impl Into<PathBuf>) -> Self {
        self.player = Arc::new(peregrine_playback::MpvPlayer::with_binary(binary));
        self
    }

    /// Replaces the external player, so a test can assert the hand-off without opening a window.
    #[must_use]
    pub fn with_player(mut self, player: Arc<dyn peregrine_domain::PlayerBackend>) -> Self {
        self.player = player;
        self
    }

    /// The directory everything the app owns lives under.
    #[must_use]
    pub fn data_directory(&self) -> &Path {
        &self.data_directory
    }

    /// The `SQLite` file.
    #[must_use]
    pub fn database_path(&self) -> PathBuf {
        self.data_directory.join(DATABASE_FILE)
    }

    /// The directory cached artwork is written into.
    ///
    /// The same directory the shell's asset protocol serves, so a path the cache reports is a path
    /// the webview can load (PG-E2-08). Nothing else may write here.
    #[must_use]
    pub fn artwork_directory(&self) -> &Path {
        &self.artwork_directory
    }

    /// The directory the rotated JSON lines are written to.
    #[must_use]
    pub fn logs_directory(&self) -> PathBuf {
        self.data_directory.join(LOG_DIRECTORY)
    }

    /// The logging configuration for [`Observability::install`](crate::Observability).
    #[must_use]
    pub fn log_config(&self) -> LogConfig {
        LogConfig {
            directory: self.logs_directory(),
            directives: self.directives.clone(),
        }
    }
}

/// A job the runtime has accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartedJob {
    /// The scheduler's identity for the job; the key progress events carry.
    ///
    /// **The nil ULID when a submission coalesced onto a live job.** The scheduler owns that
    /// identity and the journal does not store it, so a second caller that joined work already
    /// happening has no id of its own to be given — it has the *journal* row instead, which every
    /// `job://state` and `job://progress` payload carries beside the scheduler id. A caller
    /// following a job therefore matches on `journal_id` and not on this, which is correct for a
    /// fresh submission too: the two are equal in what they identify.
    pub job: Ulid,
    /// The journal row this job reports into.
    pub journal_id: i64,
    /// Whether this caller joined a job that was already live rather than starting one.
    ///
    /// Stated rather than inferred from the nil ULID, because a caller that wanted to tell the user
    /// "a refresh is already running" needs a fact and not a sentinel to decode.
    pub coalesced: bool,
}

/// What to do with one provider's credential.
///
/// The candidate travels as a value rather than as a string a caller could accidentally log: it is
/// moved into the job, proved, and dropped.
#[derive(Debug)]
pub enum CredentialRequest {
    /// Ask the provider whether the stored credential still works.
    TestConnection,
    /// Prove a candidate and store it only when the provider accepts it.
    Replace(Secret),
    /// Remove whatever is stored.
    Forget,
}

/// What a title's own page needs to decide about playing it.
///
/// Two answers from one read of the evidence, because they are two questions about the same rows: what
/// the page may *offer* (acquisition, playback, or nothing), and what this build would actually do with
/// the container of the copy the library holds. Splitting them would read the same items and files
/// twice and let the two answers disagree about which file they were describing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TitlePlayback {
    /// What the page may offer, which is `peregrine_domain::TitleOffer`'s rule.
    pub offer: peregrine_domain::TitleOffer,
    /// What this build would do with the file it would play, or `None` when the library holds no copy
    /// that is on disk.
    ///
    /// `None` and `Some(LocalPlayback::Unknown)` are different: the first says there is nothing to
    /// play, the second says there is a file whose container this build cannot name.
    pub support: Option<LocalPlayback>,
}

/// The identity of one session, which is what a stop names.
///
/// **A stop has to say *which* session, and this is why.** A renderer's effect runs twice under
/// `StrictMode` — mount, cleanup, mount — so the first mount's cleanup and the second mount's open are in
/// flight together. Without an identity the cleanup's stop closes *whatever is current*, which by then is
/// the session the second mount just opened and the element is already reading: the first request
/// succeeds, the socket closes, and the screen is a black rectangle with no error anybody can see. Found in
/// the running app, with a session log showing exactly one request answered and nothing after it.
///
/// A counter rather than a random id: the runtime mints it, nothing needs it to be unguessable, and
/// `AGENTS R14` keeps randomness in an adapter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(u64);

impl SessionId {
    /// The number, as the wire spells it.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The id a wire value names.
    ///
    /// **No check, and the absence is deliberate.** Zero is never minted — the counter starts at one — so a
    /// zero from the wire is an id that matches no session, which the stop already answers `false` for. A
    /// validating constructor here would turn "this is not yours" into an error the caller has to handle
    /// for the same outcome.
    #[must_use]
    pub const fn from_raw(value: u64) -> Self {
        Self(value)
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// One capture session: where the renderer can read a file, and where it posts the frame it drew.
///
/// A separate type from [`StartedPlayback`] because it carries a different set of facts: nothing here is
/// about how a film would play, and a caller that wanted only the two URLs would otherwise be handed a
/// container classification it has no use for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedCapture {
    /// Which session this is, which a stop must name.
    pub session_id: SessionId,
    /// The title the frame is a poster of.
    pub title_id: i64,
    /// The library file being served to the capture.
    pub media_file_id: i64,
    /// The loopback URL the capturing element loads.
    pub url: String,
    /// Where the frame is posted, or `None` when this session keeps no poster.
    pub thumbnail_url: Option<String>,
}

/// One playback session a screen can draw: what is playing, and where the bytes come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedPlayback {
    /// Which session this is, which a stop must name.
    pub session_id: SessionId,
    /// The title the session belongs to.
    pub title_id: i64,
    /// The episode this session opened, or `None` for a whole title.
    pub episode_id: Option<i64>,
    /// The library file being served.
    pub media_file_id: i64,
    /// What this build is doing with the file's container.
    pub support: LocalPlayback,
    /// The loopback URL a media element follows.
    ///
    /// A URL rather than a path or bytes, and that is the surface ADR 0023 decides: the asset protocol
    /// never exposes the library root, so the bytes travel over loopback HTTP (AGENTS R11).
    pub url: String,
    /// Where the renderer posts a frame it captured, or `None` when this session keeps no poster.
    ///
    /// Built by the core rather than spliced together by the renderer, because the token is in both URLs
    /// and a second place the route is spelled is a second place it can drift.
    pub thumbnail_url: Option<String>,
    /// Whether this title has no poster for the renderer to capture one for.
    ///
    /// **The rule that stops a film still overwriting real artwork.** A capture costs a decode of the
    /// film and a blob in the cache, and it exists for the title a provider has drawn nothing for — so the
    /// core answers whether one is wanted rather than the renderer capturing on every play and hoping the
    /// read prefers the provider's. This is `AGENTS R7` applied to the last surface that could decide it:
    /// the answer needs the catalogue row, the cache, and the local key, none of which a renderer has.
    pub needs_poster: bool,
    /// Where playback should resume, in milliseconds, when the account has a stored position.
    ///
    /// Read here rather than by a second command: the player already holds this session, and a
    /// follow-up round trip would race the frame it is trying to position. It is the account's own
    /// row for the title, which is why it is scoped rather than global.
    pub resume_position_ms: Option<i64>,
}

/// One title as the library screen lists it: what the library holds, and where the cached poster is.
///
/// The list's own unit, and the pair both surfaces built on it need — the table of counts and the grid
/// of tiles are two drawings of one read, and a tile that had to fetch its own poster would be a second
/// derivation of the cache key. The row and the path travel together for the reason
/// [`TitleEpisodeDetail`] states: a picture nothing can caption is not useful, and neither is a row
/// whose tile is blank for a reason the screen cannot see.
///
/// `None` covers both "no provider ever named a poster" and "one was named and the blob is not here",
/// which are the same thing to a reader and the two cases `artwork::cached_image` folds together.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryTitleEntry {
    /// The identity, the counts, and the provider's poster reference.
    pub row: peregrine_storage::LibraryTitleRow,
    /// Where the cached poster is, or `None` when the cache holds none (AGENTS R11).
    pub poster: Option<PathBuf>,
    /// Whether this title has no poster at all, so a frame of its own file is worth capturing.
    ///
    /// **Carried on the list rather than asked per tile**, because the answer needs the catalogue row and
    /// two cache lookups: a grid of a hundred tiles asking a hundred times would be a hundred round trips
    /// for a fact the read already had. It is `true` only when neither a provider's poster nor a generated
    /// one exists, which is the same rule [`Runtime::needs_poster`] answers on the command path.
    pub needs_poster: bool,
}

/// One title's own screen: what the library holds, where the cached poster is, whether the
/// screen should ask the provider for it, and what its page may offer.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryTitleDetail {
    /// The identity, the counts, and the cached poster — exactly what the list answers, so a title's
    /// page and its tile cannot disagree about what the library holds or what it has to show.
    pub entry: LibraryTitleEntry,
    /// Where the cached backdrop is, or `None` when the cache holds none.
    ///
    /// The wide picture the hero draws behind the title, resolved here for the reason the poster is: the
    /// cache key is a hash this layer computes, and whether the blob is on disk is a `stat` only this
    /// layer can make (AGENTS R7). Deliberately not on [`LibraryTitleEntry`]: no list draws a backdrop,
    /// and the hero is the only surface that does.
    pub backdrop: Option<PathBuf>,
    /// The provider's synopsis, when one has been written.
    pub overview: Option<String>,
    /// The provider's score, when it has one.
    pub rating: Option<f64>,
    /// When it came out, from whichever date column its kind uses.
    pub released: Option<jiff::civil::Date>,
    /// Whether opening the screen should ask a provider for this title, which is
    /// [`crate::artwork::refresh_due`]'s rule and not a screen's.
    pub refresh_due: bool,
    /// What this title's own page may offer, and what this build would do with its copy.
    ///
    /// **The offer is the reason a title the library holds never draws a Download control.** The rule is
    /// the core's for the reason every availability decision is (AGENTS R7): it reads the items, their
    /// files, and the title's jobs, three sets a screen does not have and would have to re-derive
    /// from a count — which is exactly how a page comes to offer a download of something it owns.
    ///
    /// **The support is the reason Play can say why it will not work** rather than being disabled with
    /// no explanation: a `<video>` element reports a failure *code* after it has failed, never a reason,
    /// and "this build cannot read that container yet" is a fact about the file that has to be known
    /// before the press (ADR 0023 §5).
    pub playback: TitlePlayback,
}

/// One episode as its card draws it: what the provider said, what the library holds, and where the
/// cached still is.
///
/// The row and the path travel together because neither is useful alone: a card shows a still *for* an
/// episode, and a path without the episode it belongs to is a picture nothing can caption.
#[derive(Debug, Clone, PartialEq)]
pub struct TitleEpisodeDetail {
    /// The provider's fields and the library's counts.
    pub row: peregrine_storage::TitleEpisodeRow,
    /// Where the cached still is, or `None` when the cache holds none.
    ///
    /// An absolute path on this machine rather than a URL or bytes (AGENTS R11). `None` covers both
    /// "no provider ever named one" and "one was named and the blob is not here", which a card draws
    /// identically as its reserved box — the same two cases `LibraryTitleEntry::poster` folds
    /// together, and for the same reason: the screen cannot act differently on them.
    pub still: Option<PathBuf>,
}

/// One season as a switcher draws it, with the artwork the core resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleSeasonDetail {
    /// The number, the name, the count, and the air date.
    pub row: peregrine_storage::TitleSeasonRow,
    /// Where the cached poster is, or `None` when the cache holds none.
    pub poster: Option<PathBuf>,
}

/// One rail the Discover screen draws: its identity, the copy's fallback name, and its items.
///
/// The screen renders what the core answers (`AGENTS R7`), so the rail's *membership* is here and
/// the rail's *heading* is copy in `packages/i18n` — the heading is translated and a translated
/// string is not a stable key. [`RailKind`](peregrine_providers::RailKind)'s own name travels
/// beside it so the screen can key a translation on something that does not change when the words
/// do.
/// Not `Eq`, because an item carries the provider's score as a `f64`: a float has no total equality,
/// and the rail is a projection of answers rather than a value to compare for identity.
#[derive(Debug, Clone, PartialEq)]
pub struct HomeRail {
    /// The rail's stable identity.
    pub rail: peregrine_providers::RailKind,
    /// The titles in it, in the provider's order.
    pub items: Vec<crate::jobs::discover_rails::RailItem>,
    /// What the rails are doing, which is one fact about the whole read rather than about this rail:
    /// whether a provider is configured at all, and whether the fetch that fills them is in the air,
    /// is the same for all seven. It rides on every rail because a rail is what this answer is made
    /// of and the core answers every rail — so a screen reads it without inventing an answer for a
    /// list that cannot be empty.
    pub state: RailsState,
}

/// What the Discover screen's rails are doing.
///
/// **One fact about the whole answer**, and the screen renders it rather than reading it off an
/// empty list (`AGENTS R7`): an empty rail is what a missing key and a fetch in flight both look
/// like, and telling a user who already set a key up to go and set one up is the answer this keeps
/// apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RailsState {
    /// No provider holds a credential, so nothing can fill a rail until the user sets a key up in
    /// Settings.
    NoProvider,
    /// A provider is configured and the fetch that fills the rails is under way. The rails are
    /// answered empty and drawn anyway, which is the geometry rule (`DESIGN_SYSTEM §11` rule 2).
    Fetching,
    /// The rails hold what the providers answered.
    Ready,
}

/// What a title's recommendation rail holds, and the genres it implies.
///
/// The two travel together because the second is derived from the first and costs no request: a title
/// page draws the rail and then offers a shelf of *more of the same genre*, and that genre is read off
/// the candidates the provider just answered with rather than asked for separately.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Recommendations {
    /// The rail's tiles, in the provider adapter's order.
    pub items: Vec<crate::jobs::discover_rails::RailItem>,
    /// The genres the rail shares, most-shared first, **named**.
    ///
    /// Named rather than identified because a shelf's heading is the only thing it has to say and a
    /// screen that had to look the name up would be a screen that drew nothing until it arrived. The
    /// names come from the kind's vocabulary, which `Runtime::genres` reads once per kind per session
    /// and caches, so the second title page of a kind pays nothing for them.
    pub genres: Vec<peregrine_providers::GenreRef>,
}

/// How many genre shelves a title page may offer.
///
/// Two, because a page that listed every shared genre would be a page of genre shelves with a title
/// buried in it, and because the first two are the ones a reader recognises the title by.
const MAX_GENRE_SHELVES: usize = 2;

/// The genres a recommendation list implies, most-shared first.
///
/// **Inferred, and stated as an inference rather than as a fact about the title.** A list endpoint
/// names each item's genres and a details endpoint names the title's own, and this build reads
/// *neither* into the catalogue — nothing writes `title_genre` — so the shelves a title page offers are
/// derived from what the provider listed *around* the title. For a franchise film or a genre picture
/// that is the same answer as the title's own genres; for an odd one it is a hint, which is why the
/// shelves are labelled by genre rather than described as the title's.
///
/// The rule: a genre must appear on at least a quarter of the items **and at least twice**, so one
/// unusual neighbour cannot invent a shelf, and ties break by identifier so that two runs over the
/// same answer produce the same shelves in the same order (`AGENTS R14`).
fn shared_genres(candidates: &[peregrine_providers::provider::SearchCandidate]) -> Vec<u32> {
    let mut counts: BTreeMap<u32, usize> = BTreeMap::new();
    for candidate in candidates {
        for genre in candidate.genre_ids() {
            *counts.entry(*genre).or_default() += 1;
        }
    }
    let floor = (candidates.len() / 4).max(2);
    let mut kept: Vec<(u32, usize)> = counts
        .into_iter()
        .filter(|(_, count)| *count >= floor)
        .collect();
    kept.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    kept.into_iter()
        .map(|(genre, _)| genre)
        .take(MAX_GENRE_SHELVES)
        .collect()
}

/// One page of a browsed list, as a screen draws it.
/// **A page and not a list, because the screen has to know whether to keep scrolling.** `has_more`
/// is the provider's own answer (`peregrine_providers::ProviderPage`), carried rather than derived:
/// a screen that guessed from the number of tiles would either stop one page early or ask for a page
/// past the provider's end on every scroll.
///
/// The items are [`RailItem`](crate::jobs::discover_rails::RailItem)s, which is the shape a rail
/// already draws — a poster, a name, a year, a score — because a browse grid and a rail are the same
/// tiles in a different arrangement, and a second type would be a second mapping of the same row.
#[derive(Debug, Clone, PartialEq)]
pub struct BrowsePage {
    /// The titles on this page, in the provider's order.
    pub items: Vec<crate::jobs::discover_rails::RailItem>,
    /// Whether asking for the next page would find anything.
    pub has_more: bool,
}

/// The genre lists this session has read, one slot per kind.
///
/// **In memory, and deliberately not in the `genre` table.** That table holds a *catalogue's*
/// genres by name, and a provider's vocabularies are per kind with the provider's own identifiers —
/// TMDB numbers even the names its two lists share differently — so what a filter control needs is
/// an answer about the provider's current vocabulary rather than a row. The cost is one request per
/// kind per launch; the alternative is writing a provider's numbers into a table whose `name`
/// column cannot say which kind it describes.
#[derive(Debug, Default)]
struct Genres {
    /// One slot per kind, indexed by [`Genres::slot`]; `None` until a read has answered.
    lists: RwLock<[Option<Vec<peregrine_providers::GenreRef>>; 2]>,
}

impl Genres {
    /// Which slot a kind uses.
    fn slot(kind: peregrine_domain::catalogue::TitleKind) -> usize {
        match kind {
            peregrine_domain::catalogue::TitleKind::Movie => 0,
            peregrine_domain::catalogue::TitleKind::Show => 1,
        }
    }

    /// The list this session already read for `kind`, if it read one.
    fn get(
        &self,
        kind: peregrine_domain::catalogue::TitleKind,
    ) -> Option<Vec<peregrine_providers::GenreRef>> {
        self.lists.read().expect("the lock is not poisoned")[Self::slot(kind)].clone()
    }

    /// Records what a provider answered for `kind`.
    fn put(
        &self,
        kind: peregrine_domain::catalogue::TitleKind,
        list: Vec<peregrine_providers::GenreRef>,
    ) {
        self.lists.write().expect("the lock is not poisoned")[Self::slot(kind)] = Some(list);
    }
}

/// What the diagnostics surface reports about the running process.
#[derive(Debug, Clone)]
pub struct Health {
    /// How long startup took, from the first directory check to a ready runtime.
    pub startup: Duration,
    /// The scheduler's counters.
    pub scheduler: SchedulerMetrics,
    /// The event bus's counters.
    pub events: EventStats,
    /// Whether the boot-time integrity check had to rebuild the database, which means the
    /// library it held is gone and has to be rebuilt by scanning (ARCHITECTURE §15).
    pub rescan_required: bool,
    /// Which metadata providers hold a credential (`PG-E5-12`). A provider nobody has configured is
    /// not an error, and the interface shows an empty state rather than a failure for it.
    pub providers: crate::providers::ProviderStatus,
}

/// What the shell's tray shows about background work (PG-E1-09).
///
/// The shell asks the runtime for this rather than assembling it from webview events:
/// the job journal and the scheduler are the only owners of "how much work is in
/// flight" (ARCHITECTURE §6, "State ownership"), and a count computed in the UI would
/// be wrong the moment a job started while the window was closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellStatus {
    /// Jobs the scheduler still tracks: ready, deferred, or running.
    pub active_downloads: u32,
    /// Torrents seeding. Zero until a torrent engine is composed into the runtime
    /// (PG-E10): `peregrine-torrent` is documentation-only today, so there is no source
    /// for a different number and inventing one would be the lie the tray menu must not
    /// tell.
    pub seeding: u32,
    /// Whether the shell should show a tray icon at all.
    pub tray_enabled: bool,
}

/// Classifies a playback refusal into the one a person can act on.
///
/// The two refusals that are *about the file* get their own variants, because the sentence beside a
/// Play control is different for each: a file that moved is a drive to check, a path that is not a file
/// is a library problem, and a listener that will not bind is neither. Everything else keeps the stream
/// layer's own error, unwrapped, so its message reaches the log intact.
fn playback_error(error: peregrine_playback::PlaybackError) -> RuntimeError {
    match &error {
        peregrine_playback::PlaybackError::Serve(peregrine_stream::StreamError::Missing {
            ..
        }) => RuntimeError::MediaMissing,
        peregrine_playback::PlaybackError::Serve(peregrine_stream::StreamError::NotAFile {
            ..
        }) => RuntimeError::MediaNotAFile,
        // Every other reason a listener would not serve: one this process could not open, bind, or write.
        // Ours rather than the user's, and carried through unchanged so the log keeps the detail.
        peregrine_playback::PlaybackError::Serve(_) => RuntimeError::Playback(error),
    }
}

/// The file of a title that a Play press would use, or `None` when the library holds none that is on
/// disk.
///
/// **An item's own preference wins over the read order.** A library holds several copies of a film
/// routinely — a remux and a smaller rip — and `media_item.preferred_file_id` is where the library's
/// answer to "which one" lives, so following read order would play an arbitrary one of two. Missing
/// files are filtered either way, because a copy the last scan could not find is a copy this build
/// cannot serve and a session opened for one would fail at the first frame.
///
/// **What this deliberately does not do is rank.** Nothing here scores quality: `quality_label` is
/// `PG-E14-11`'s and no build writes it yet, so a rule that read it would be a rule about a column that
/// is always empty, and a "best copy" that was really the first row would be worse than no claim.
fn playable_file(
    evidence: &peregrine_storage::TitleEvidence,
    episode_id: Option<i64>,
) -> Option<&peregrine_domain::MediaFile> {
    // A title-level play may draw on any item; an episode-level one may only draw on the items the
    // import linked to that episode. Filtering at the one place a file is chosen keeps the title's
    // first file from being served for a different episode.
    let items = || {
        evidence
            .items
            .iter()
            .filter(move |item| episode_id.is_none() || item.item.episode_id == episode_id)
    };
    let preferred = items().find_map(|item| {
        let wanted = item.item.preferred_file_id?;
        item.files
            .iter()
            .find(|file| file.id == wanted && file.missing_since.is_none())
    });
    preferred.or_else(|| {
        items()
            .flat_map(|item| item.files.iter())
            .find(|file| file.missing_since.is_none())
    })
}

/// Hands a player's position to the task that writes it.
///
/// A channel rather than a direct write because the sink is called from the player's own reader
/// thread and the repository is async; the receiving task owns the storage edge.
#[derive(Debug)]
struct ChannelSink(tokio::sync::mpsc::UnboundedSender<(i64, Option<i64>)>);

impl peregrine_domain::PositionSink for ChannelSink {
    fn position(&self, position_ms: i64, duration_ms: Option<i64>) {
        // A closed receiver means the app is shutting down; dropping the sample is correct then.
        let _ = self.0.send((position_ms, duration_ms));
    }
}

/// The running application.
pub struct Runtime {
    config: RuntimeConfig,
    database: Arc<Database>,
    scheduler: Arc<Scheduler>,
    bus: Arc<EventBus>,
    clock: Arc<dyn Clock + Send + Sync>,
    live: Mutex<HashMap<Ulid, LiveJob>>,
    // Never read: held for its `Drop`, which flushes the queued log lines and stops the
    // writer thread. Losing it before shutdown would silently drop the lines that explain
    // the shutdown.
    _logging: Observability,
    /// Which providers hold a credential, refreshed off the command path (see `providers`).
    providers: Arc<crate::providers::ProviderRegistry>,
    /// Stubs the matcher has already queued this session.
    ///
    /// A title the provider cannot name stays a stub, and without this the automatic pass would
    /// search for it on every tick. A restart tries once more, which is when a new key or a fixed
    /// provider should get its chance.
    attempted_matches: Mutex<std::collections::HashSet<i64>>,
    /// Where provider credentials are kept, off the command path for the same reason.
    secrets: Arc<dyn SecretStore>,
    /// Where fetched artwork lives, content-addressed (see `peregrine_sync::ImageCache`).
    artwork: Arc<peregrine_sync::ImageCache>,
    /// What the Discover screen reads and the rails job writes, in memory for this slice (see
    /// `crate::jobs::discover_rails::HomeRails` for the cold-start cost).
    rails: Arc<crate::jobs::discover_rails::HomeRails>,
    /// The genre vocabularies this session has read (see [`Genres`]).
    genres: Arc<Genres>,
    /// Posters this machine generated from the films themselves, and the sink a capture route stores
    /// through. Held beside the artwork cache because that is where its blobs live.
    posters: Arc<crate::local_posters::LocalPosters>,
    /// Where session tokens come from. Injected for the reason the clock is: the domain may not touch a
    /// random source, so the composition root is the only place that picks the adapter (AGENTS R14).
    tokens: Arc<dyn TokenSource + Send + Sync>,
    /// The next session id to hand out, shared by both slots so no id is reused while a stale stop could
    /// still be in flight.
    next_session: std::sync::atomic::AtomicU64,
    /// Which session each slot is holding, so a stop can say whether it is the one it meant.
    capture_id: Mutex<Option<SessionId>>,
    /// The capture session that is open, if any.
    ///
    /// **Deliberately not the playback slot.** A poster backfill runs while a reader may be watching
    /// something else, and one slot for both would mean opening a backfill session stopped the film. Two
    /// listeners on two ports is exactly what the design allows.
    capture: Mutex<Option<peregrine_playback::PlaybackSession>>,
    /// Which playback session the slot is holding, for the same reason.
    session_id: Mutex<Option<SessionId>>,
    /// The playback session that is open, if any.
    ///
    /// At most one, and the field is the reason: a second press replaces the first session, which is
    /// what makes the listener's lifetime the session's rather than the process's. The lock is a
    /// `std::sync::Mutex` and is therefore **never held across an `await`** — every use takes the value
    /// out, drops the guard, and only then awaits closing it.
    session: Mutex<Option<peregrine_playback::PlaybackSession>>,
    /// The external player (mpv) behind the Play action for a container the webview cannot demux.
    ///
    /// Held for this session so a second press replaces the first window and shutdown closes it
    /// rather than leaving a player behind.
    external_player: Arc<dyn peregrine_domain::PlayerBackend>,
    startup: Duration,
    /// The torrent session, or `None` when it could not be started (see [`Runtime::start`]).
    ///
    /// An `Option` rather than a hard failure because a session that cannot bind its listeners or
    /// open its storage must not take the library, providers, and playback down with it. A caller
    /// that needs the engine asks [`Runtime::torrent_engine`] and gets a typed refusal.
    torrent_engine: Option<Arc<dyn TorrentEngine>>,
    /// The same session as [`Runtime::torrent_engine`], as the metadata-only fetch `FLOWS` F6
    /// sequences before an add. One `Arc<LibrqbitEngine>` coerced to both traits, so the fetch
    /// that produces a file list and the session that downloads it cannot be two engines.
    metadata_fetch: Option<Arc<dyn TorrentMetadataFetch>>,
    /// The acquisition service built over [`Runtime::torrent_engine`], and `None` with it.
    acquisition: Option<Arc<AcquisitionService>>,
    /// The managed indexer manager (docs/adr/0024). Installed and started only when asked, so a
    /// launch that never opens Settings pays for neither.
    prowlarr: Arc<crate::jobs::prowlarr::Managed>,
    /// The notification service, behind the one capability the acquisition path uses. The centre
    /// row is the record; the OS toast is a no-op until `PG-E1-14` writes the adapter.
    notifications: Arc<dyn NotifyOffers>,
}

/// The notification service this runtime composes.
///
/// The storage repository appears twice because it implements both ports the service needs: the
/// centre that stores the row, and the catalogue question asked before a title may be named.
type Notifications = NotificationService<
    NotificationRepository,
    CataloguePreferences,
    NoOsNotifier,
    NotificationRepository,
>;

/// The acquire path's view of the notification service.
///
/// A job holds this rather than the service so it does not have to name four generic ports, and
/// so what it can do is one capability rather than the whole service.
struct OfferNotifier {
    service: Arc<Notifications>,
    bus: Arc<EventBus>,
}

impl NotifyOffers for OfferNotifier {
    fn offers_available<'a>(
        &'a self,
        scope: AccountScope,
        title_id: i64,
        label: Option<String>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            // The centre row is the record and the toast is a bonus (R31), so a failure here is
            // logged and never changes the acquire job's outcome.
            match self
                .service
                .notify(
                    scope,
                    NotificationEvent::ReleaseAlternatives,
                    title_id,
                    label.as_deref(),
                    Focus::default(),
                )
                .await
            {
                Ok(_) => {
                    // A trigger to re-read rather than the row itself: the bell asks the centre,
                    // so a subscriber can never render copy the repository has not validated.
                    // Published only on success, because a row that was not written is not news.
                    self.bus.publish(Event::new(
                        NOTIFICATION_RAISED,
                        title_id.to_string(),
                        serde_json::json!({ "event": "release.alternatives" }),
                    ));
                }
                Err(error) => warn!(%error, "the alternatives notification could not be recorded"),
            }
        })
    }
}

/// The runtime's own clock, handed to the notification service so both read one source.
struct SharedClock(Arc<dyn Clock + Send + Sync>);

impl Clock for SharedClock {
    fn now(&self) -> jiff::Timestamp {
        self.0.now()
    }
}

/// A job the runtime can still cancel.
struct LiveJob {
    handle: JobHandle,
    journal_id: i64,
}

/// What a settings screen draws about the managed indexer manager. Never the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProwlarrStatus {
    /// Whether this build has a pinned Prowlarr for the running platform.
    pub supported: bool,
    /// Whether the verified pack is on disk.
    pub installed: bool,
    /// Whether a managed instance is running.
    pub running: bool,
    /// The pinned version this build installs.
    pub version: &'static str,
    /// The loopback endpoint, when one is running.
    pub base_url: Option<String>,
    /// What the managed instance is doing: `idle`, `installing`, or `starting`.
    pub phase: &'static str,
    /// Install progress in permille (0..=1000), reported only while installing.
    pub progress_permille: u16,
}

impl Runtime {
    /// Opens the data directory, the database, and the scheduler.
    ///
    /// The logging handle is passed in already installed, so that a caller which cannot
    /// install a process-wide subscriber — a test — can still start a runtime without
    /// either failing or writing into the developer's log directory.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::DataDirectory`] when the directory cannot be created,
    /// [`RuntimeError::Storage`] when the database cannot be opened or migrated. Both
    /// happen before any job can be accepted, so a failure here leaves nothing running.
    #[allow(clippy::too_many_lines)] // Startup is one ordered sequence; a split would hide the order.
    pub async fn start(
        config: RuntimeConfig,
        logging: Observability,
    ) -> Result<Self, RuntimeError> {
        let started = Instant::now();
        tokio::fs::create_dir_all(config.data_directory())
            .await
            .map_err(|source| RuntimeError::DataDirectory {
                path: config.data_directory().to_path_buf(),
                source,
            })?;

        // The managed Prowlarr keeps its key in the app's own `prowlarr-data/config.xml`, and the
        // keyring can refuse to hand it back; recording the directory here is what lets search read
        // that file as a fallback (crate::indexers).
        crate::indexers::set_managed_data_dir(config.data_directory().to_path_buf());

        // A recovery is logged where it happens, in `Database::open`; the runtime carries
        // the signal on to health.
        let database = Arc::new(Database::open(config.database_path()).await?);
        // The composition root is the one place the production adapters are chosen
        // (AGENTS R8); the scheduler is handed both ports rather than reaching for them.
        let clock: Arc<dyn Clock + Send + Sync> = Arc::new(SystemClock);
        // A fresh installation has no quality profile, and ranking cannot run without one, so the
        // built-in default is written the first time the runtime composes. A failure is not fatal:
        // the acquire job reports "no profile" in its own words.
        match database
            .ensure_default_quality_profile(clock.now().as_millisecond())
            .await
        {
            Ok(_) => {}
            Err(error) => warn!(%error, "the built-in quality profile could not be seeded"),
        }
        let ids: Arc<dyn IdGenerator + Send + Sync> = Arc::new(UlidGenerator);
        // The third adapter the domain may not reach for itself: a token has to come from a real random
        // source, and `cargo xtask check-determinism` permits one in `peregrine-platform` and nowhere
        // else.
        let tokens: Arc<dyn TokenSource + Send + Sync> = Arc::new(TokenMinter);
        let scheduler = Arc::new(Scheduler::new(SchedulerConfig::new(
            Arc::clone(&clock),
            ids,
        )));
        let bus = Arc::new(EventBus::new(Arc::clone(&config.sink)));

        // Read before the struct literal moves `config`. Not created here: the shell creates it when
        // it scopes the asset protocol to it, and a cache that makes directories on construction does
        // filesystem work on a path a session may never use.
        let artwork_root = config.artwork_directory().to_path_buf();
        let secrets = Arc::clone(&config.secrets);
        let providers = Arc::new(crate::providers::ProviderRegistry::new());
        // Off the start-up path and off the command path: a keyring read is blocking and can raise an OS
        // prompt, so it happens once, on a blocking thread, and everything else reads the cache.
        {
            let providers = Arc::clone(&providers);
            let secrets = Arc::clone(&secrets);
            tokio::task::spawn_blocking(move || {
                if let Err(error) = providers.refresh(secrets.as_ref()) {
                    tracing::debug!(%error, "no credential store answered, so no provider is configured");
                }
            });
        }

        let artwork = Arc::new(peregrine_sync::ImageCache::new(artwork_root));
        // Built here because it needs the cache, the database, and the clock together, and because the
        // capture route that uses it has no other context to reach them through.
        let posters = Arc::new(crate::local_posters::LocalPosters::new(
            Arc::clone(&artwork),
            Arc::clone(&database),
            Arc::clone(&clock),
        ));
        let rails = Arc::new(crate::jobs::discover_rails::HomeRails::new());
        let genres = Arc::new(Genres::default());
        // The rails the last fetch published, so a launch draws them rather than seven empty
        // headings it then re-fetches the same lists to fill (PG-E7-08). A read that fails is not a
        // failure to start: the map stays empty, the first read of the screen starts a fetch, and the
        // launch behaves exactly as it did before this manifest existed.
        match database.discover_rails(installation_scope()).await {
            Ok(rows) => rails.hydrate(&crate::jobs::discover_rails::manifest_rails(rows, &artwork)),
            Err(error) => {
                warn!(%error, "the rails manifest could not be read; the first fetch will fill the rails");
            }
        }

        // Measured after the manifest read, because that read is part of what a launch does before
        // the screen can be drawn from it: a number that excluded it would understate the cold start
        // this budget exists to hold (`AGENTS` §6).
        let startup = started.elapsed();
        info!(
            startup_ms = startup.as_millis(),
            database = %config.database_path().display(),
            "the runtime is ready"
        );

        // The download root. `RuntimeConfig` carries no configured download directory — library
        // roots live in the database and are read per scan rather than cached here — so the session
        // writes under the data directory, the one directory the app already owns and creates. A
        // downloads root the user configured can replace it once the settings registry lands
        // (PG-E20-01); this is deliberately not a second reading of that table.
        let download_root = config.data_directory().join(DOWNLOAD_DIRECTORY);
        let mut torrent_engine: Option<Arc<dyn TorrentEngine>> = None;
        let mut metadata_fetch: Option<Arc<dyn TorrentMetadataFetch>> = None;
        let mut acquisition: Option<Arc<AcquisitionService>> = None;
        match tokio::fs::create_dir_all(&download_root).await {
            Err(error) => {
                warn!(%error, "the download directory could not be created; downloads are unavailable this session");
            }
            Ok(()) => match peregrine_domain::PathGuard::new(&download_root) {
                Err(error) => {
                    warn!(%error, "the download root is not a usable path; downloads are unavailable this session");
                }
                Ok(destination) => {
                    match LibrqbitEngine::start(download_root, &EngineLimits::unlimited()).await {
                        Err(error) => {
                            // Fail soft: the engine is one feature. The error's own `Display` carries
                            // no path, magnet, or hash (PG-E10-15), so the line is safe to log.
                            warn!(%error, "the torrent engine did not start; downloads are unavailable this session");
                        }
                        Ok(engine) => {
                            // One concrete session coerced to both traits: the fetch that produces a
                            // file list and the session that then downloads it must be the same
                            // engine, and cloning one `Arc` twice is what makes that structural.
                            let engine = Arc::new(engine);
                            let torrent: Arc<dyn TorrentEngine> = engine.clone();
                            let fetch: Arc<dyn TorrentMetadataFetch> = engine;
                            let service = AcquisitionService::new(
                                Arc::clone(&database),
                                Arc::clone(&torrent),
                                Arc::clone(&clock),
                                destination,
                                Arc::new(SystemVolume),
                                FREE_SPACE_FLOOR,
                                Arc::clone(&bus),
                                Arc::new(SystemTrash),
                            );
                            torrent_engine = Some(torrent);
                            metadata_fetch = Some(fetch);
                            acquisition = Some(Arc::new(service));
                        }
                    }
                }
            },
        }

        let prowlarr = Arc::new(crate::jobs::prowlarr::Managed::discover(
            config.data_directory(),
        ));

        // The notification service, composed here because this is the only place that may pick the
        // adapters. The OS notifier is the placeholder until PG-E1-14 and the preferences answer the
        // catalogue defaults until PG-E20-01; the centre row is written either way, which is R31's
        // order and the reason a refused toast is not an error.
        let notifications: Arc<dyn NotifyOffers> = Arc::new(OfferNotifier {
            service: Arc::new(NotificationService::new(
                NotificationRepository::new(Arc::clone(&database)),
                CataloguePreferences,
                NoOsNotifier,
                NotificationRepository::new(Arc::clone(&database)),
                Box::new(SharedClock(Arc::clone(&clock))),
                Platform::current(),
            )),
            bus: Arc::clone(&bus),
        });

        let external_player = Arc::clone(&config.player);
        let runtime = Self {
            config,
            database,
            scheduler,
            bus,
            clock,
            live: Mutex::new(HashMap::new()),
            _logging: logging,
            providers,
            attempted_matches: Mutex::new(std::collections::HashSet::new()),
            secrets,
            artwork,
            rails,
            genres,
            posters,
            tokens,
            next_session: std::sync::atomic::AtomicU64::new(1),
            capture_id: Mutex::new(None),
            capture: Mutex::new(None),
            session_id: Mutex::new(None),
            session: Mutex::new(None),
            external_player,
            startup,
            torrent_engine,
            metadata_fetch,
            acquisition,
            prowlarr,
            notifications,
        };

        // The engine is pull-only: it transfers without telling anyone, so a queue row and the
        // download mark on a tile only move because something asks. The pump is started here,
        // after the engine exists, and stopped with the bus in `Runtime::shutdown`.
        if let Some(service) = &runtime.acquisition {
            // Before the pump: finish the jobs a previous process left at a whole payload, so a
            // download that completed while the app was closed is not stuck at 100 % for ever.
            if let Err(error) = service.reconcile_finished().await {
                tracing::warn!(%error, "the finished downloads could not be reconciled");
            }
            service.start_pump();
        }

        // An installed managed Prowlarr starts with the app (docs/adr/0024), so the user does not
        // press Start on every launch. Submitting is a journal row and a channel send; the 90 s
        // health wait belongs to the job, not to this path. A submit that fails is logged and
        // forgotten: a Prowlarr that cannot run must never fail or delay the launch, and the
        // existing dedupe key and live-supervisor checks keep a second start from being spawned.
        if runtime.prowlarr.is_installed()
            && !runtime.prowlarr.is_running()
            && let Err(error) = runtime.start_prowlarr().await
        {
            tracing::warn!(%error, "the managed Prowlarr did not start automatically");
        }

        Ok(runtime)
    }

    /// The configuration this runtime was built with.
    #[must_use]
    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }

    /// The torrent session, or the typed refusal that says it did not start.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::TorrentEngineUnavailable`] when the session is down (see [`Runtime::start`]).
    pub fn torrent_engine(&self) -> Result<Arc<dyn TorrentEngine>, RuntimeError> {
        self.torrent_engine
            .clone()
            .ok_or(RuntimeError::TorrentEngineUnavailable)
    }

    /// The acquisition service built over [`Runtime::torrent_engine`].
    ///
    /// # Errors
    ///
    /// [`RuntimeError::TorrentEngineUnavailable`] when the session is down.
    pub fn acquisition(&self) -> Result<Arc<AcquisitionService>, RuntimeError> {
        self.acquisition
            .clone()
            .ok_or(RuntimeError::TorrentEngineUnavailable)
    }

    /// The same session as [`Runtime::torrent_engine`], as the metadata-only fetch.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::TorrentEngineUnavailable`] when the session is down.
    pub fn metadata_fetch(&self) -> Result<Arc<dyn TorrentMetadataFetch>, RuntimeError> {
        self.metadata_fetch
            .clone()
            .ok_or(RuntimeError::TorrentEngineUnavailable)
    }

    /// The managed indexer manager's state, for a settings screen. Never the key.
    ///
    /// Infallible on purpose: a screen asks this before deciding what to offer, and an error here
    /// would leave it with nothing to draw.
    #[must_use]
    pub fn prowlarr_status(&self) -> ProwlarrStatus {
        let phase = self.prowlarr.phase();
        // Only the install has a byte total to divide by; the start is a health wait with no
        // measurable fraction, so it reports the phase and no bar.
        let progress_permille = match phase {
            crate::jobs::prowlarr::Phase::Installing => {
                let total = crate::prowlarr::pinned_size().unwrap_or(0);
                if total == 0 {
                    0
                } else {
                    let done = self.prowlarr.downloaded_bytes().min(total);
                    u16::try_from(done.saturating_mul(1000) / total).unwrap_or(1000)
                }
            }
            _ => 0,
        };
        ProwlarrStatus {
            supported: crate::prowlarr::is_supported(),
            installed: self.prowlarr.is_installed(),
            running: self.prowlarr.is_running(),
            version: crate::prowlarr::VERSION,
            base_url: self.prowlarr.base_url(),
            phase: phase.as_str(),
            progress_permille,
        }
    }

    /// Downloads, verifies, and unpacks the pinned Prowlarr pack, as a job.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Storage`] when the journal cannot be written, and
    /// [`RuntimeError::Scheduler`] when the queue refuses the job.
    pub async fn start_prowlarr_install(&self) -> Result<StartedJob, RuntimeError> {
        let now = self.clock.now().as_millisecond();
        let kind = crate::jobs::prowlarr::INSTALL_KIND;
        let priority = crate::jobs::prowlarr::PRIORITY;
        let journal_id = self
            .database
            .insert_job(NewJob {
                kind: kind.to_owned(),
                priority,
                dedupe_key: Some(kind.to_owned()),
                payload_json: json!({ "version": crate::prowlarr::VERSION }).to_string(),
                max_attempts: 1,
                created_at: now,
            })
            .await?;
        let spec = JobSpec {
            kind: kind.to_owned(),
            priority,
            dedupe_key: Some(kind.to_owned()),
            max_attempts: 1,
        };
        let job = crate::jobs::prowlarr::ProwlarrInstallJob::new(
            Arc::clone(&self.database),
            Arc::clone(&self.bus),
            Arc::clone(&self.prowlarr),
            self.config.data_directory().to_path_buf(),
            journal_id,
        );
        match self.scheduler.submit(spec, job).await {
            Ok(handle) => {
                let started = StartedJob {
                    job: handle.id(),
                    journal_id,
                    coalesced: false,
                };
                self.remember(handle, journal_id);
                Ok(started)
            }
            Err(error) => {
                let _ = self
                    .database
                    .update_job(
                        journal_id,
                        JobChange::Finished {
                            at: now,
                            phase: JobPhase::Failed,
                            error_code: Some("INTERNAL".to_owned()),
                            error: Some(error.to_string()),
                        },
                    )
                    .await;
                Err(RuntimeError::Scheduler(error))
            }
        }
    }

    /// Starts the installed Prowlarr and stores the endpoint and key it generated, as a job.
    ///
    /// # Errors
    ///
    /// The same two as [`Runtime::start_prowlarr_install`].
    pub async fn start_prowlarr(&self) -> Result<StartedJob, RuntimeError> {
        let now = self.clock.now().as_millisecond();
        let kind = crate::jobs::prowlarr::START_KIND;
        let priority = crate::jobs::prowlarr::PRIORITY;
        let journal_id = self
            .database
            .insert_job(NewJob {
                kind: kind.to_owned(),
                priority,
                dedupe_key: Some(kind.to_owned()),
                payload_json: json!({ "version": crate::prowlarr::VERSION }).to_string(),
                max_attempts: 1,
                created_at: now,
            })
            .await?;
        let spec = JobSpec {
            kind: kind.to_owned(),
            priority,
            dedupe_key: Some(kind.to_owned()),
            max_attempts: 1,
        };
        let job = crate::jobs::prowlarr::ProwlarrStartJob::new(
            Arc::clone(&self.database),
            Arc::clone(&self.bus),
            Arc::clone(&self.prowlarr),
            Arc::clone(&self.secrets),
            self.config.data_directory().to_path_buf(),
            journal_id,
        );
        match self.scheduler.submit(spec, job).await {
            Ok(handle) => {
                let started = StartedJob {
                    job: handle.id(),
                    journal_id,
                    coalesced: false,
                };
                self.remember(handle, journal_id);
                Ok(started)
            }
            Err(error) => {
                let _ = self
                    .database
                    .update_job(
                        journal_id,
                        JobChange::Finished {
                            at: now,
                            phase: JobPhase::Failed,
                            error_code: Some("INTERNAL".to_owned()),
                            error: Some(error.to_string()),
                        },
                    )
                    .await;
                Err(RuntimeError::Scheduler(error))
            }
        }
    }

    /// Stops the managed Prowlarr, if one is running. Idempotent.
    pub fn stop_prowlarr(&self) {
        self.prowlarr.stop();
    }

    /// Starts the hello job (`PG-E0-19`) and returns its identities.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::InvalidRequest`] when `steps` is outside
    /// [`MIN_STEPS`](hello::MIN_STEPS)..=[`MAX_STEPS`](hello::MAX_STEPS), because a caller
    /// that asks for an unbounded amount of work is a caller bug and the message says
    /// which bound it broke.
    pub async fn start_hello(&self, steps: u32) -> Result<StartedJob, RuntimeError> {
        if !(hello::MIN_STEPS..=hello::MAX_STEPS).contains(&steps) {
            return Err(RuntimeError::InvalidRequest {
                detail: format!(
                    "steps must be between {} and {}",
                    hello::MIN_STEPS,
                    hello::MAX_STEPS
                ),
            });
        }

        let now = self.clock.now().as_millisecond();
        let journal_id = self
            .database
            .insert_job(NewJob {
                kind: hello::KIND.to_owned(),
                priority: hello::PRIORITY,
                dedupe_key: None,
                payload_json: json!({ "steps": steps }).to_string(),
                max_attempts: 1,
                created_at: now,
            })
            .await?;

        let spec = JobSpec {
            kind: hello::KIND.to_owned(),
            priority: hello::PRIORITY,
            dedupe_key: None,
            max_attempts: 1,
        };
        let job = HelloJob::new(
            Arc::clone(&self.database),
            Arc::clone(&self.bus),
            Arc::clone(&self.clock),
            journal_id,
            steps,
        );

        let handle = match self.scheduler.submit(spec, job).await {
            Ok(handle) => handle,
            Err(error) => {
                // The row exists and will never run, so leaving it `queued` would show a
                // job waiting forever in the UI. Closing it here is the same rule the
                // scheduler applies to a failed attempt: a job that cannot run reaches a
                // terminal state.
                let _ = self
                    .database
                    .update_job(
                        journal_id,
                        JobChange::Finished {
                            at: now,
                            phase: JobPhase::Failed,
                            error_code: Some(INTERNAL_CODE.to_owned()),
                            error: Some(error.to_string()),
                        },
                    )
                    .await;
                return Err(RuntimeError::Scheduler(error));
            }
        };

        let started = StartedJob {
            job: handle.id(),
            journal_id,
            coalesced: false,
        };
        info!(
            job = %started.job,
            journal_id,
            steps,
            "accepted the hello job"
        );
        self.remember(handle, journal_id);
        Ok(started)
    }

    /// Asks the job to stop.
    ///
    /// The request is journaled before the token trips, so a job that is never observed
    /// again still shows that it was asked to stop.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::UnknownJob`] when no job with that id is live — a job that already
    /// finished, or an id from another process.
    pub async fn cancel(&self, job: Ulid) -> Result<(), RuntimeError> {
        let journal_id = {
            let live = self.live();
            live.get(&job)
                .map(|entry| entry.journal_id)
                .ok_or(RuntimeError::UnknownJob { job })?
        };

        // The row is the authority on whether a job is still a job. A job writes its own terminal
        // row and the scheduler resolves its handle afterwards, so there is a window in which the
        // journal says `canceled` and the handle still looks live — and asking to cancel in that
        // window used to answer "ok" about a job that had already stopped, which is the one answer
        // a caller cannot tell apart from a real cancellation. Reading the phase closes the window
        // at the point where the two sources of truth meet. A read that fails leaves the handle as
        // the only answer rather than failing the request: refusing to cancel because a journal
        // read failed would be strictly worse.
        if let Ok(Some(row)) = self.database.job(journal_id).await
            && row.state.is_terminal()
        {
            self.live().remove(&job);
            return Err(RuntimeError::UnknownJob { job });
        }

        if let Err(error) = self
            .database
            .update_job(journal_id, JobChange::CancelRequested)
            .await
        {
            // The cancellation itself must not depend on the audit write succeeding.
            warn!(job = %job, %error, "the cancellation request could not be journaled");
        }

        let mut live = self.live();
        if let Some(entry) = live.get_mut(&job) {
            entry.handle.cancel();
        }
        info!(job = %job, "cancellation requested");
        Ok(())
    }

    /// A page of the rules a user's corrections became, newest first.
    ///
    /// Read under [`installation_scope`], like every user-facing read: this is a list a person
    /// reviews and edits.
    pub async fn match_rules(
        &self,
        limit: u32,
    ) -> Result<Vec<peregrine_storage::MatchRuleRow>, RuntimeError> {
        Ok(self
            .database
            .list_match_rules(installation_scope(), PageRequest::first(limit))
            .await?)
    }

    /// Removes one rule, reporting whether it was there.
    ///
    /// What makes a wrong answer recoverable: a rule places every later file that reads like the one
    /// it was made from, so withdrawing it is the only way back for a user who answered wrongly. The
    /// files it already placed stay, because removing a file is a different action with its own
    /// confirmation (`PG-E15`).
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Library`] when the database refuses, which is the only failure this has.
    pub async fn forget_match_rule(&self, id: i64) -> Result<bool, RuntimeError> {
        Ok(self.database.remove_match_rule(id).await?)
    }

    /// A page of the files a scan could not place, newest first.
    ///
    /// Read under [`installation_scope`], like every user-facing read: this is a list a person
    /// answers, so a file belonging to a title an account may not see is not offered to it.
    pub async fn unmatched_files(
        &self,
        limit: u32,
    ) -> Result<Vec<peregrine_storage::UnmatchedFileRow>, RuntimeError> {
        Ok(self
            .database
            .list_unmatched_files(installation_scope(), PageRequest::first(limit))
            .await?)
    }

    /// Answers one parked file by placing it as the title its parse read.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Library`] when the database refuses, which is the only failure this has.
    /// Answers one parked file: under the title its parse read, or under the title the scan offered
    /// when the caller asks for that one.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Library`] when the database refuses, which is the only failure this has.
    pub async fn match_unmatched_file(
        &self,
        id: i64,
        as_title: crate::library::MatchAs,
    ) -> Result<bool, RuntimeError> {
        let matched = crate::library::match_unmatched(
            &self.database,
            installation_scope(),
            id,
            as_title,
            self.clock.now(),
        )
        .await?;
        Ok(matches!(matched, crate::library::Matched::Placed))
    }

    /// A page of titles `scope` may see that match the user's own text.
    ///
    /// The picker's read. The query reaches `FTS5` through the storage layer, which is the only place
    /// it is turned into a `MATCH` expression and the only place hostile input is answered rather than
    /// refused: a query the grammar would refuse comes back as an empty page.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Library`] when the database refuses, which is the only failure this has.
    pub async fn search_titles(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<peregrine_storage::TitleIdentity>, RuntimeError> {
        Ok(self
            .database
            .search(installation_scope(), query, PageRequest::first(limit))
            .await?)
    }

    /// Every rail the Discover screen draws, and the fetch that fills them.
    ///
    /// **It reads no network and answers from what is here** (`FLOWS F2`): the membership and the
    /// resolved artwork paths were written by the rails job, so this is a lock read and a clone
    /// rather than a query or a request. A rail nobody has fetched yet is answered as an empty rail
    /// rather than omitted, which is what gives the screen its geometry on the first frame of a
    /// cold start (`DESIGN_SYSTEM §11` rule 2).
    ///
    /// **A cold read is also what starts the fetch.** `FLOWS F2` has the core answer the rail shells
    /// and enqueue the refresh in the same breath, and the Refresh command is not a second path to
    /// that: every rail empty with a provider configured goes through
    /// [`start_discover_rails`](Self::start_discover_rails), the same call the screen's button makes.
    /// That call reads the journal by its dedupe key before it writes anything, so a read while the
    /// fetch is in flight — or a read racing a press of Refresh — joins the live job rather than
    /// writing a second row. With rails already in hand there is nothing to fetch.
    ///
    /// **`NoProvider` is answered only by a credential read that has finished**, which is the one
    /// thing this read must not get wrong: the read of the credential store is blocking and runs off
    /// the startup path (a keychain can raise an OS prompt), so the first read of a session routinely
    /// arrives before it lands. A status that is empty because nobody has looked yet is not a status
    /// that says nobody has configured a provider — answering `NoProvider` there told a user with a
    /// key to go and set one up, and it never started the fetch that key would have filled the rails
    /// with, so the only way through was to press Refresh once the prompt had been answered. The
    /// fetch is enqueued instead, and the job waits for that same read before deciding it has
    /// nothing to ask.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Storage`] when the journal cannot be read or written and
    /// [`RuntimeError::Scheduler`] when the queue refuses the job. Reported rather than swallowed,
    /// because a read that answered "fetching" while nothing was queued would be a screen waiting
    /// for a job that does not exist.
    pub async fn home_rails(
        &self,
        kind: Option<peregrine_domain::catalogue::TitleKind>,
    ) -> Result<Vec<HomeRail>, RuntimeError> {
        let rails = self.rails.read();
        let state = if rails.iter().any(|(_, items)| !items.is_empty()) {
            RailsState::Ready
        } else if self.providers.refreshed() && self.providers.status().is_empty() {
            RailsState::NoProvider
        } else {
            self.start_discover_rails().await?;
            RailsState::Fetching
        };
        Ok(crate::jobs::discover_rails::rails_of_kind(rails, kind)
            .into_iter()
            .map(|(rail, items)| HomeRail { rail, items, state })
            .collect())
    }

    /// Starts filling the Discover screen's rails, artwork included.
    ///
    /// A job rather than a command because it is one request per rail and then one per title it has
    /// artwork to fetch for (`ARCHITECTURE §7` rule 2). Two attempts, because every failure it can
    /// report is a provider having a bad minute.
    ///
    /// **The dedupe key is one key for the screen.** `Scheduler::submit_if_absent` is documented as
    /// coalescing a second submission onto the live job that owns the key — and the engine does
    /// drop the second submission's work — but it still answers `Ok(Some(handle))`, so a caller
    /// cannot tell a coalesced submission from a fresh one. What this method therefore reads is the
    /// *journal*: a live row with this key means a run is already queued or running, and its id is
    /// answered rather than a second row being written. Two visits to Discover inside one fetch
    /// share it; the `insert_job` partial unique index would refuse the second row anyway, which is
    /// why the read comes first.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Storage`] when the journal cannot be read or written and
    /// [`RuntimeError::Scheduler`] when the queue refuses the job.
    pub async fn start_discover_rails(&self) -> Result<StartedJob, RuntimeError> {
        let now = self.clock.now().as_millisecond();
        let key = crate::jobs::discover_rails::dedupe_key();
        if let Some(live) = self.live_job_for(&key).await? {
            info!(
                journal_id = live.journal_id,
                "the discover rails job is already running"
            );
            return Ok(live);
        }

        let journal_id = self
            .database
            .insert_job(NewJob {
                kind: crate::jobs::discover_rails::KIND.to_owned(),
                priority: crate::jobs::discover_rails::PRIORITY,
                dedupe_key: Some(key.clone()),
                payload_json: "{}".to_owned(),
                max_attempts: 2,
                created_at: now,
            })
            .await?;

        let spec = JobSpec {
            kind: crate::jobs::discover_rails::KIND.to_owned(),
            priority: crate::jobs::discover_rails::PRIORITY,
            dedupe_key: Some(key),
            max_attempts: 2,
        };
        let job = crate::jobs::discover_rails::DiscoverRailsJob::new(
            Arc::clone(&self.database),
            Arc::clone(&self.bus),
            Arc::clone(&self.clock),
            Arc::clone(&self.providers),
            Arc::clone(&self.artwork),
            Arc::clone(&self.rails),
            journal_id,
        );
        match self.scheduler.submit(spec, job).await {
            Ok(handle) => {
                let started = StartedJob {
                    job: handle.id(),
                    journal_id,
                    coalesced: false,
                };
                self.remember(handle, journal_id);
                Ok(started)
            }
            Err(error) => {
                let _ = self
                    .database
                    .update_job(
                        journal_id,
                        JobChange::Finished {
                            at: now,
                            phase: JobPhase::Failed,
                            error_code: Some(INTERNAL_CODE.to_owned()),
                            error: Some(error.to_string()),
                        },
                    )
                    .await;
                Err(RuntimeError::Scheduler(error))
            }
        }
    }

    /// The journal row a live job holds `dedupe_key` under, if one does.
    ///
    /// The journal's partial unique index is on `(kind, dedupe_key)` over the three non-terminal
    /// phases, and `list_jobs` pages newest first, so the newest page is where a live row for a key
    /// is. A page is bounded by `MAX_PAGE_ROWS`, which is well above the number of jobs a screen
    /// starts, and a row older than that page is not live anyway.
    async fn live_job_for(&self, key: &str) -> Result<Option<StartedJob>, RuntimeError> {
        let page = self
            .database
            .list_jobs(PageRequest::first(peregrine_storage::MAX_PAGE_ROWS))
            .await?;
        Ok(page
            .into_iter()
            .find(|row| {
                row.dedupe_key.as_deref() == Some(key)
                    && matches!(
                        row.state,
                        JobPhase::Queued | JobPhase::Running | JobPhase::Deferred
                    )
            })
            .map(|row| StartedJob {
                // The scheduler's own id is what a *fresh* submission's events carry, and the
                // journal does not store it — so a caller that joined a job already running is
                // answered the nil ULID and the journal row instead. Every `job://state` and
                // `job://progress` payload carries both ids, and a screen following a job matches
                // on the journal one, which exists before the job runs and outlives it.
                job: Ulid::from_parts(0, 0),
                journal_id: row.id,
                coalesced: true,
            }))
    }

    /// Starts filling one title from its provider, artwork included.
    ///
    /// A job rather than a command, because it leaves the machine twice — once for the title's details
    /// and once for its poster — and `ARCHITECTURE §7` rule 2 puts anything that slow behind the
    /// scheduler. Keyed by the title, so opening the same screen twice waits on one refresh rather
    /// than spending two requests on the same answer.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Storage`] when the journal cannot be written and [`RuntimeError::Scheduler`] when
    /// the queue refuses the job.
    pub async fn start_provider_refresh(
        &self,
        title_id: i64,
        kind: peregrine_domain::catalogue::TitleKind,
    ) -> Result<StartedJob, RuntimeError> {
        let now = self.clock.now().as_millisecond();
        let key = crate::jobs::provider_refresh::dedupe_key(title_id);
        let journal_id = self
            .database
            .insert_job(NewJob {
                kind: crate::jobs::provider_refresh::KIND.to_owned(),
                priority: crate::jobs::provider_refresh::PRIORITY,
                dedupe_key: Some(key.clone()),
                payload_json: json!({ "titleId": title_id }).to_string(),
                max_attempts: 2,
                created_at: now,
            })
            .await?;

        let spec = JobSpec {
            kind: crate::jobs::provider_refresh::KIND.to_owned(),
            priority: crate::jobs::provider_refresh::PRIORITY,
            dedupe_key: Some(key),
            max_attempts: 2,
        };
        let job = crate::jobs::provider_refresh::ProviderRefreshJob::new(
            Arc::clone(&self.database),
            Arc::clone(&self.bus),
            Arc::clone(&self.clock),
            Arc::clone(&self.providers),
            Arc::clone(&self.artwork),
            journal_id,
            crate::jobs::provider_refresh::Target { title_id, kind },
        );
        match self.scheduler.submit(spec, job).await {
            Ok(handle) => {
                let started = StartedJob {
                    job: handle.id(),
                    journal_id,
                    coalesced: false,
                };
                self.remember(handle, journal_id);
                Ok(started)
            }
            Err(error) => {
                let _ = self
                    .database
                    .update_job(
                        journal_id,
                        JobChange::Finished {
                            at: now,
                            phase: JobPhase::Failed,
                            error_code: Some("INTERNAL".to_owned()),
                            error: Some(error.to_string()),
                        },
                    )
                    .await;
                Err(RuntimeError::Scheduler(error))
            }
        }
    }

    /// Finds one title's best release and queues its download, as a job.
    ///
    /// When `episode` names one, the search is narrowed to that episode and only a release that is
    /// exactly it can be chosen; `None` acquires the title as a whole.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the journal cannot be written, `RuntimeError::Scheduler` when the
    /// queue refuses the job, and `RuntimeError::TorrentEngineUnavailable` when the torrent session
    /// did not start.
    pub async fn start_acquire(
        &self,
        account: peregrine_domain::AccountScope,
        title_id: i64,
        episode: Option<crate::jobs::acquire::EpisodePick>,
        min_seeders: u32,
    ) -> Result<StartedJob, RuntimeError> {
        self.submit_acquire(
            account,
            crate::jobs::acquire::Target {
                title_id,
                episode,
                chosen: None,
            },
            min_seeders,
        )
        .await
    }

    /// Queues the release the user picked from the offers, without a second search.
    ///
    /// # Errors
    ///
    /// `RuntimeError::InvalidRequest` when the offer is gone or was already acted on, plus the same
    /// storage, scheduler and engine errors that `start_acquire` reports.
    pub async fn acquire_release(
        &self,
        account: peregrine_domain::AccountScope,
        offer_id: i64,
        min_seeders: u32,
    ) -> Result<StartedJob, RuntimeError> {
        let chosen = self
            .database
            .choose_release_offer(offer_id)
            .await?
            .ok_or_else(|| RuntimeError::InvalidRequest {
                detail: "that offer is gone or was already used".to_owned(),
            })?;
        let source: peregrine_indexers::TorrentSource = serde_json::from_str(&chosen.source_json)
            .map_err(|_| {
            RuntimeError::InvalidRequest {
                detail: "that offer's release is not usable".to_owned(),
            }
        })?;
        self.submit_acquire(
            account,
            crate::jobs::acquire::Target {
                title_id: chosen.title_id,
                episode: None,
                chosen: Some(crate::jobs::acquire::ChosenOffer {
                    raw_title: chosen.release_title,
                    indexer: chosen.indexer,
                    size_bytes: chosen
                        .size_bytes
                        .and_then(|value| u64::try_from(value).ok()),
                    seeders: chosen.seeders.and_then(|value| u32::try_from(value).ok()),
                    source,
                }),
            },
            min_seeders,
        )
        .await
    }

    /// The offers a search left for one title.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the read fails.
    pub async fn release_offers(
        &self,
        title_id: i64,
    ) -> Result<Vec<peregrine_storage::OfferRow>, RuntimeError> {
        self.database
            .list_release_offers(title_id)
            .await
            .map_err(RuntimeError::from)
    }

    /// Dismisses one offer so a later search does not offer it again.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the write fails.
    pub async fn dismiss_release_offer(&self, offer_id: i64) -> Result<bool, RuntimeError> {
        self.database
            .dismiss_release_offer(offer_id)
            .await
            .map_err(RuntimeError::from)
    }

    /// The one submit path both entry points use.
    async fn submit_acquire(
        &self,
        account: peregrine_domain::AccountScope,
        target: crate::jobs::acquire::Target,
        min_seeders: u32,
    ) -> Result<StartedJob, RuntimeError> {
        let title_id = target.title_id;
        let now = self.clock.now().as_millisecond();
        let key = crate::jobs::acquire::dedupe_key(title_id, target.episode);
        let journal_id = self
            .database
            .insert_job(NewJob {
                kind: crate::jobs::acquire::KIND.to_owned(),
                priority: crate::jobs::acquire::PRIORITY,
                dedupe_key: Some(key.clone()),
                payload_json: json!({ "titleId": title_id }).to_string(),
                max_attempts: 2,
                created_at: now,
            })
            .await?;

        let spec = JobSpec {
            kind: crate::jobs::acquire::KIND.to_owned(),
            priority: crate::jobs::acquire::PRIORITY,
            dedupe_key: Some(key),
            max_attempts: 2,
        };
        let job = crate::jobs::acquire::AcquireJob::new(
            Arc::clone(&self.database),
            Arc::clone(&self.bus),
            self.secret_store(),
            self.metadata_fetch()?,
            self.acquisition()?,
            Arc::clone(&self.clock),
            account,
            journal_id,
            target,
            min_seeders,
            Arc::clone(&self.notifications),
        );
        match self.scheduler.submit(spec, job).await {
            Ok(handle) => {
                let started = StartedJob {
                    job: handle.id(),
                    journal_id,
                    coalesced: false,
                };
                self.remember(handle, journal_id);
                Ok(started)
            }
            Err(error) => {
                let _ = self
                    .database
                    .update_job(
                        journal_id,
                        JobChange::Finished {
                            at: now,
                            phase: JobPhase::Failed,
                            error_code: Some("INTERNAL".to_owned()),
                            error: Some(error.to_string()),
                        },
                    )
                    .await;
                Err(RuntimeError::Scheduler(error))
            }
        }
    }

    /// The download queue, newest first, under the scope every user-facing read uses.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the read fails.
    pub async fn download_jobs(
        &self,
    ) -> Result<Vec<peregrine_storage::DownloadJobRow>, RuntimeError> {
        Ok(self
            .database
            .list_download_jobs(
                installation_scope(),
                peregrine_domain::DownloadState::ALL,
                PageRequest::first(200),
            )
            .await?)
    }

    /// One download job, or None when no such row exists or the scope may not see its title.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the read fails.
    pub async fn download_job(
        &self,
        id: i64,
    ) -> Result<Option<peregrine_storage::DownloadJobRow>, RuntimeError> {
        Ok(self.database.download_job(installation_scope(), id).await?)
    }

    /// Proves, stores, or clears one provider's credential, and answers the job doing it.
    ///
    /// **The candidate is validated before it is stored** (`PG-E5-20`): a `Replace` probes it
    /// against the provider and writes it to the keyring only when the provider accepted it, so a
    /// key that does not work never reaches the keychain. The credential itself travels in
    /// `action` and lives in the job's memory; the journal row names the provider and nothing else,
    /// because a row is a file, a file is a backup, and a backup is not where a secret belongs
    /// (SECURITY.md T13).
    ///
    /// The keyring work happens inside the job, off the runtime: a read blocks and on macOS the
    /// first one from a new binary raises an OS prompt that measured 11.7 s (`PG-E5-12`), which is
    /// why nothing here touches a store directly.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::InvalidRequest`] when this build knows no provider by that name, or when a
    /// candidate cannot even be turned into an adapter — neither is work worth a journal row.
    /// [`RuntimeError::Provider`] when the provider refuses the configuration it was given.
    /// [`RuntimeError::Storage`] when the row cannot be written, and [`RuntimeError::Scheduler`]
    /// when the queue refuses the job.
    pub async fn start_provider_credential(
        &self,
        provider: &str,
        action: CredentialRequest,
    ) -> Result<StartedJob, RuntimeError> {
        let Some(entry) = crate::providers::entry(provider) else {
            return Err(RuntimeError::InvalidRequest {
                detail: format!("`{provider}` is not a metadata provider this build knows"),
            });
        };
        // A candidate no adapter can be built from is a bad request rather than a job: there is
        // nothing to probe, and a row would only record what the caller already got wrong. The job
        // builds its own adapter from the same candidate when it probes.
        //
        // `BuildError` has no `RuntimeError` of its own: the two failures `build_with` can answer
        // with are told apart instead — a provider that refuses its configuration is a provider
        // failure, and anything else is a request this build cannot serve. `build_with` reads no
        // keyring, so its store arm cannot arise here; it is matched rather than wildcarded so a
        // variant added to `BuildError` later fails the build instead of becoming a bad request.
        if let CredentialRequest::Replace(candidate) = &action {
            match crate::providers::build_with(entry, candidate) {
                Ok(_) => {}
                Err(BuildError::Provider(error)) => return Err(RuntimeError::Provider(error)),
                Err(error @ (BuildError::Unknown { .. } | BuildError::Store(_))) => {
                    return Err(RuntimeError::InvalidRequest {
                        detail: error.to_string(),
                    });
                }
            }
        }
        let (action, candidate) = match action {
            CredentialRequest::TestConnection => (provider_credential::Action::Verify, None),
            CredentialRequest::Replace(candidate) => {
                (provider_credential::Action::Replace, Some(candidate))
            }
            CredentialRequest::Forget => (provider_credential::Action::Forget, None),
        };

        let now = self.clock.now().as_millisecond();
        // Keyed by the provider, so a double-click on "test connection" waits on one probe rather
        // than spending two requests on one answer.
        let key = provider_credential::dedupe_key(provider);
        let journal_id = self
            .database
            .insert_job(NewJob {
                kind: provider_credential::KIND.to_owned(),
                priority: provider_credential::PRIORITY,
                dedupe_key: Some(key.clone()),
                payload_json: provider_credential::payload(provider),
                max_attempts: 1,
                created_at: now,
            })
            .await?;

        let spec = JobSpec {
            kind: provider_credential::KIND.to_owned(),
            priority: provider_credential::PRIORITY,
            dedupe_key: Some(key),
            max_attempts: 1,
        };
        let job = provider_credential::ProviderCredentialJob::new(
            Arc::clone(&self.bus),
            Arc::clone(&self.clock),
            Arc::clone(&self.database),
            Arc::clone(&self.providers),
            Arc::clone(&self.secrets),
            journal_id,
            entry,
            action,
            candidate,
            crate::providers::build_with,
        );
        match self.scheduler.submit(spec, job).await {
            Ok(handle) => {
                let started = StartedJob {
                    job: handle.id(),
                    journal_id,
                    coalesced: false,
                };
                info!(
                    job = %started.job,
                    journal_id,
                    provider,
                    "accepted the provider credential job"
                );
                self.remember(handle, journal_id);
                Ok(started)
            }
            Err(error) => {
                // The row exists and will never run, so leaving it `queued` would show a check
                // waiting forever in the UI. Closed here for the same reason the scan and refresh
                // close theirs.
                let _ = self
                    .database
                    .update_job(
                        journal_id,
                        JobChange::Finished {
                            at: now,
                            phase: JobPhase::Failed,
                            error_code: Some(INTERNAL_CODE.to_owned()),
                            error: Some(error.to_string()),
                        },
                    )
                    .await;
                Err(RuntimeError::Scheduler(error))
            }
        }
    }

    /// Searches the providers this installation has configured, writes what they know into the
    /// catalogue, and answers with the titles those results name.
    ///
    /// **The answer is the provider's order, and not a second local search.** The local index
    /// tokenises "Spider-Man" into `spider` and `man`, so the reader who types "spiderman" has no
    /// term the index can match — even after the title is here. Re-searching that text would answer
    /// zero for a title the provider just found. The sync records the candidates it was given, and
    /// this reads those titles back by the ids it attached: catalogue-owned rows with an id a screen
    /// can open, in the order the provider ranked them.
    ///
    /// **A provider answer with nothing readable in it falls back to the local index.** A provider
    /// that is configured but does not carry a title the catalogue already holds, or one whose
    /// candidates the matching rule all left as questions, leaves the local read to answer rather
    /// than turning that into a claim that the library is empty.
    ///
    /// With no provider configured this is that local search, which is the honest answer rather than
    /// an error: an installation with no API key still has a library.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Provider`] when a provider fails, and [`RuntimeError::Storage`] when the
    /// catalogue cannot be read or written.
    pub async fn discover(
        &self,
        query: &str,
        limit: u32,
        kind: Option<peregrine_domain::catalogue::TitleKind>,
    ) -> Result<Vec<peregrine_storage::TitleIdentity>, RuntimeError> {
        let adapters = self.providers.adapters();
        if let Some(provider) = adapters.first() {
            let text = NonEmptyText::new(query).map_err(|_| RuntimeError::EmptyQuery)?;
            let limit = NonZeroU32::new(limit).unwrap_or(NonZeroU32::MIN);
            let search = peregrine_providers::provider::SearchQuery::new(text, limit, kind);
            let at = self.clock.now().as_millisecond();
            let outcome =
                peregrine_sync::sync_search(provider.as_ref(), &self.database, &search, at).await?;
            let scope = installation_scope();
            // Two candidates can settle on the same local row — a provider can answer with two ids
            // for one title — and a screen keys its rows by that row, so the repeat is dropped here
            // rather than drawn twice.
            let mut seen = std::collections::HashSet::new();
            let mut found = Vec::new();
            for external in &outcome.found {
                let Some(id) = self.database.title_for_external_id(external).await? else {
                    continue;
                };
                if !seen.insert(id) {
                    continue;
                }
                if let Some(identity) = self.database.title_identity(scope, id).await? {
                    found.push(identity);
                }
                if found.len() >= limit.get() as usize {
                    break;
                }
            }
            if !found.is_empty() {
                return Ok(found);
            }
        }
        Ok(self
            .database
            .search_kind(installation_scope(), query, PageRequest::first(limit), kind)
            .await?)
    }

    /// Searches the configured providers and answers with tiles the grid can draw.
    ///
    /// **The tile-shaped sibling of [`Runtime::discover`].** That read answers identities for a text
    /// list; a poster needs the year, the score, the synopsis, and a cached path, and those are what a
    /// list answer carries. Each candidate goes through [`Runtime::listed_item`] — the browse writer —
    /// so a search result and a browse page describe a title the same way, and the artwork pass is the
    /// same [`Runtime::fill_posters`] the grid already uses. The provider is asked directly rather
    /// than through the local index first, because the index tokenises "Spider-Man" into two terms and
    /// cannot match a reader who types "spiderman"; the provider that published the title can.
    ///
    /// With no provider configured this is a local read of the index, drawn from the catalogue rows
    /// the library already holds. **A provider that fails is the same case**: the local read is
    /// attempted, and its answer is returned if it has one. The provider's error is only reported
    /// when there is nothing local to show either — a hiccup in one reader must not tell a man with
    /// fifteen Spider-Man titles in his catalogue that nothing matched.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Provider`] when the provider fails **and** the local index has nothing,
    /// [`RuntimeError::Storage`] when the catalogue cannot be read or written, and
    /// [`RuntimeError::EmptyQuery`] for a blank query.
    pub async fn search_tiles(
        &self,
        query: &str,
        limit: u32,
        kind: Option<peregrine_domain::catalogue::TitleKind>,
    ) -> Result<Vec<crate::jobs::discover_rails::RailItem>, RuntimeError> {
        // The query's text is not logged: a search is the reader's own words, and the count is what
        // diagnoses a call without carrying them into a log file.
        info!(
            query_chars = query.chars().count(),
            ?kind,
            "search_tiles asked"
        );
        let adapters = self.providers.adapters();
        let mut provider_error = None;
        if let Some(provider) = adapters.first() {
            let provider_result: Result<Vec<crate::jobs::discover_rails::RailItem>, RuntimeError> =
                async {
                    let text = NonEmptyText::new(query).map_err(|_| RuntimeError::EmptyQuery)?;
                    let limit = NonZeroU32::new(limit).unwrap_or(NonZeroU32::MIN);
                    let search = peregrine_providers::provider::SearchQuery::new(text, limit, kind);
                    let candidates = provider.search(&search).await?;
                    let at = self.clock.now().as_millisecond();
                    let mut items = Vec::new();
                    let mut sources = Vec::new();
                    // Two candidates can settle on one row — a provider can answer with two ids for one
                    // title — and a grid keys its tiles by that row, so the repeat is dropped before it
                    // is drawn.
                    let mut seen = std::collections::HashSet::new();
                    for candidate in candidates.iter().take(limit.get() as usize) {
                        let Some((item, source)) = self.listed_item(candidate, at).await? else {
                            continue;
                        };
                        if !seen.insert(item.title_id) {
                            continue;
                        }
                        items.push(item);
                        sources.push(source);
                        if items.len() >= limit.get() as usize {
                            break;
                        }
                    }
                    self.fill_posters(provider, &mut items, &sources).await;
                    Ok(items)
                }
                .await;
            match provider_result {
                Ok(items) if !items.is_empty() => {
                    info!(tiles = items.len(), "search_tiles answered from a provider");
                    return Ok(items);
                }
                Ok(_) => {}
                Err(error) => {
                    warn!(%error, "the provider search failed; answering from the local index");
                    provider_error = Some(error);
                }
            }
        }

        let found = self
            .database
            .search_kind(installation_scope(), query, PageRequest::first(limit), kind)
            .await?;
        let mut items = Vec::new();
        for identity in found {
            let Some(detail) = self.library_title(identity.id).await? else {
                continue;
            };
            let playable = detail.entry.row.files > detail.entry.row.missing_files;
            items.push(crate::jobs::discover_rails::RailItem {
                title_id: identity.id,
                kind: identity.kind,
                name: identity.name,
                year: detail.released.map(|date| i32::from(date.year())),
                poster: detail.entry.poster,
                backdrop_source: None,
                backdrop: detail.backdrop,
                playable,
                rating: detail.rating,
                overview: detail.overview,
                votes: None,
            });
        }
        if items.is_empty()
            && let Some(error) = provider_error
        {
            return Err(error);
        }
        info!(
            tiles = items.len(),
            "search_tiles answered from the local index"
        );
        Ok(items)
    }

    /// One page of a filtered, ordered list, written into the catalogue and answered as it is drawn.
    ///
    /// **A command and not a job, and that is a deliberate reading of `ARCHITECTURE §7` rule 2
    /// rather than a shortcut around it.** The rule exists so a screen never waits on work it did
    /// not ask for; this read *is* the work the reader asked for. A job would answer with an id and
    /// no page, leaving the screen with nothing to draw until a job it cannot name stops — which is
    /// the full-page spinner `PG-E7-02` forbids. The precedent is [`Runtime::discover`], the search
    /// read: one provider request, written into the catalogue before it is answered, with the caller
    /// waiting for it by definition.
    ///
    /// **The page is written before it is answered**, for the reason `discover` gives: an item is a
    /// title the catalogue owns, with a row id a screen can open, rather than a row that exists only
    /// in this response. [`peregrine_sync::upsert_list_candidate`] is the write, shared with the
    /// rails job, so one film cannot be two titles on one screen and one on another.
    ///
    /// **The posters are fetched here too.** A grid of reserved boxes that fills in later is not the
    /// page anybody asked for, and every one of these images is keyed by the provider and the path
    /// the list answer already carried — so a page seen before costs a `stat` per tile and no
    /// request, and what paces the first one is `AGENTS R24`'s per-host token bucket rather than a
    /// constant here.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::NoProvider`] when no configured provider serves a queried list,
    /// [`RuntimeError::Provider`] when the provider fails, and [`RuntimeError::Sync`] when the
    /// answer cannot be written.
    pub async fn browse(
        &self,
        request: &peregrine_providers::BrowseRequest,
    ) -> Result<BrowsePage, RuntimeError> {
        let Some(provider) = self.browsing_provider() else {
            return Err(RuntimeError::NoProvider);
        };
        let at = self.clock.now().as_millisecond();
        let answer = provider.browse(request).await?;

        let mut items = Vec::with_capacity(answer.items().len());
        // Aligned with `items`: the poster path item `i` would fetch, or `None` when the list answer
        // named none. Kept beside the items rather than on them because a path is an input to a
        // fetch and an item carries only what a screen draws.
        let mut sources: Vec<Option<String>> = Vec::with_capacity(answer.items().len());
        for candidate in answer.items() {
            if let Some((item, source)) = self.listed_item(candidate, at).await? {
                items.push(item);
                sources.push(source);
            }
        }

        self.fill_posters(&provider, &mut items, &sources).await;
        Ok(BrowsePage {
            items,
            has_more: answer.has_more(),
        })
    }

    /// The titles a provider considers like one the catalogue holds, as tiles.
    ///
    /// **A read of the title, not of the screen.** The identifier asked for is the one *this*
    /// provider owns, which is the rule `peregrine_sync::refresh_title` states from the other
    /// direction: a title may carry several providers' ids, and asking one of them for another's
    /// number is how a film gets another film's recommendations. A title no configured provider
    /// holds an id for is answered empty rather than as a failure — a title the library inferred
    /// from a filename has no recommendations to fetch, and that is a fact rather than an error.
    ///
    /// **Empty is the answer for a title the catalogue does not hold, too**, which is the same
    /// answer `get_title` gives for one this account may not see: the two are indistinguishable by
    /// design (I37), and a rail is drawn or not drawn rather than explained.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::NoProvider`] when no configured provider serves a queried list,
    /// [`RuntimeError::Provider`] when the provider fails, and [`RuntimeError::Sync`] when an
    /// answer cannot be written.
    pub async fn similar(&self, title_id: i64) -> Result<Recommendations, RuntimeError> {
        let Some(provider) = self.browsing_provider() else {
            return Err(RuntimeError::NoProvider);
        };
        let Some(row) = self
            .database
            .library_title(installation_scope(), title_id)
            .await?
        else {
            return Ok(Recommendations::default());
        };
        let kind = row.library.title.kind;
        let Some(external) = self
            .database
            .external_ids_of(title_id)
            .await?
            .into_iter()
            .map(|held| held.id)
            .find(|external| external.provider() == provider.descriptor().provider())
        else {
            return Ok(Recommendations::default());
        };

        let at = self.clock.now().as_millisecond();
        let candidates = provider.similar(&external, kind).await?;
        // Read before the candidates are consumed, because the genres travel on them.
        let shared = shared_genres(&candidates);
        // The names, from the vocabulary this session already has for the kind — a genre whose name is
        // unknown is dropped rather than drawn as a number, which is what a shelf heading cannot be.
        let vocabulary = self.genres(kind).await.unwrap_or_default();
        let genres = shared
            .into_iter()
            .filter_map(|id| vocabulary.iter().find(|entry| entry.id() == id).cloned())
            .collect();
        let mut items = Vec::with_capacity(candidates.len());
        let mut sources: Vec<Option<String>> = Vec::with_capacity(candidates.len());
        for candidate in &candidates {
            if let Some((item, source)) = self.listed_item(candidate, at).await? {
                items.push(item);
                sources.push(source);
            }
        }

        self.fill_posters(&provider, &mut items, &sources).await;
        Ok(Recommendations { items, genres })
    }

    /// One list candidate as a tile, and the poster path that tile would fetch.
    ///
    /// **The shared half of every read that answers with a list** — browse and the similar-titles
    /// rail (`PG-E7-09`, `PG-E7-15`): write what the list carried, read the library's own answer
    /// about the title, and hand back both the tile and the path the artwork pass needs. `None` is a
    /// candidate the matching rule would not attribute, which is left out of the list rather than
    /// merged into a guess.
    async fn listed_item(
        &self,
        candidate: &peregrine_providers::provider::SearchCandidate,
        at: i64,
    ) -> Result<Option<(crate::jobs::discover_rails::RailItem, Option<String>)>, RuntimeError> {
        let Some(listed) =
            peregrine_sync::upsert_list_candidate(&self.database, candidate, at).await?
        else {
            return Ok(None);
        };
        let stored = self
            .database
            .library_title(installation_scope(), listed.title_id)
            .await
            .ok()
            .flatten();
        let source = stored
            .as_ref()
            .and_then(|row| row.library.poster.as_ref())
            .map(|artwork| artwork.path.clone());
        Ok(Some((
            crate::jobs::discover_rails::RailItem {
                title_id: listed.title_id,
                kind: listed.details.kind,
                name: listed.details.name.as_str().to_owned(),
                year: listed.details.released.map(|date| i32::from(date.year())),
                // Filled by the artwork pass, once the list's images have been asked for.
                poster: None,
                // A grid and a rail of recommendations draw no wide image, so the backdrop a hero
                // uses is not read here.
                backdrop_source: None,
                backdrop: None,
                playable: stored
                    .as_ref()
                    .is_some_and(|row| row.library.files > row.library.missing_files),
                rating: listed.details.rating,
                overview: listed.details.overview.clone(),
                votes: listed.details.votes,
            },
            source,
        )))
    }

    /// The genres the configured provider filters by for `kind`, read once per session.
    ///
    /// **Cache-first, and the cache is the session's** ([`Genres`]). A genre vocabulary is a handful
    /// of rows that changes on the provider's clock, so re-asking on every visit to a filter control
    /// would be a request per visit for an answer nothing had changed (`AGENTS R24`).
    ///
    /// # Errors
    ///
    /// [`RuntimeError::NoProvider`] when no configured provider serves a queried list,
    /// and [`RuntimeError::Provider`] when the provider fails.
    pub async fn genres(
        &self,
        kind: peregrine_domain::catalogue::TitleKind,
    ) -> Result<Vec<peregrine_providers::GenreRef>, RuntimeError> {
        if let Some(cached) = self.genres.get(kind) {
            return Ok(cached);
        }
        let Some(provider) = self.browsing_provider() else {
            return Err(RuntimeError::NoProvider);
        };
        let list = provider.genres(kind).await?;
        self.genres.put(kind, list.clone());
        Ok(list)
    }

    /// The first configured provider that can serve a queried list, if there is one.
    ///
    /// **One provider, because a page is one list.** Two providers' pages cannot be interleaved
    /// without an order that belongs to neither. Selecting on the capability rather than taking the
    /// first adapter is what keeps a search-only provider — an `OMDb` configured for ratings, say —
    /// from answering a browse with a refusal.
    fn browsing_provider(&self) -> Option<Arc<dyn peregrine_providers::MetadataProvider>> {
        self.providers.adapters().into_iter().find(|adapter| {
            adapter
                .descriptor()
                .can(peregrine_providers::Capability::Trending)
        })
    }

    /// Fetches a page's posters into the cache and puts the paths on the items.
    ///
    /// **Concurrently, because they are independent images on a CDN** and a sequential loop would be
    /// one round trip after another for a page the reader is looking at; what bounds the burst is
    /// the provider client's own token bucket (`AGENTS R24`) rather than a constant here. A title
    /// whose poster did not arrive keeps `None`, which is the reserved box a grid draws anyway — one
    /// failed image is not a failed page.
    ///
    /// `sources` is aligned with `items`: entry `i` is the poster path item `i` would fetch.
    async fn fill_posters(
        &self,
        provider: &Arc<dyn peregrine_providers::MetadataProvider>,
        items: &mut [crate::jobs::discover_rails::RailItem],
        sources: &[Option<String>],
    ) {
        let mut pending = tokio::task::JoinSet::new();
        for (index, source) in sources.iter().enumerate() {
            let Some(path) = source else {
                continue;
            };
            // A blob the cache already holds costs a `stat` and no request, which is what makes a
            // second visit to the same page cheap — and it is checked here rather than inside the
            // task so the common case spawns nothing at all.
            if let Some(found) = crate::artwork::cached_image(
                &self.artwork,
                provider.descriptor().provider(),
                path,
                peregrine_domain::catalogue::ImageKind::Poster,
            ) {
                if let Some(item) = items.get_mut(index) {
                    item.poster = Some(found);
                }
                continue;
            }
            let provider = Arc::clone(provider);
            let artwork = Arc::clone(&self.artwork);
            let path = path.clone();
            pending.spawn(async move {
                let found = crate::artwork::cached_or_fetched(
                    &artwork,
                    provider.as_ref(),
                    &path,
                    peregrine_domain::catalogue::ImageKind::Poster,
                )
                .await;
                (index, found)
            });
        }
        while let Some(joined) = pending.join_next().await {
            if let Ok((index, Some(found))) = joined
                && let Some(item) = items.get_mut(index)
            {
                item.poster = Some(found);
            }
        }
    }

    /// One title, with the counts a screen shows, the cached poster, and whether opening the screen
    /// should ask its provider for more, when the scope may see it.
    ///
    /// Three answers from one statement and one `stat`: the counts are the aggregate's, the poster
    /// path is decided by the cache (`crate::artwork::cached_image`), and whether to ask is the rule
    /// in `crate::artwork::refresh_due`. Resolving the artwork here rather than in the screen is
    /// `AGENTS R7`: the cache key is a hash, and whether a fill has happened is a column.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Library`] when the database refuses, which is the only failure this has.
    pub async fn library_title(&self, id: i64) -> Result<Option<LibraryTitleDetail>, RuntimeError> {
        let Some(row) = self
            .database
            .library_title(installation_scope(), id)
            .await?
        else {
            return Ok(None);
        };
        // Both posters are resolved before the row is taken apart, because the refresh rule below reads
        // the whole row and the entry below consumes half of it. **Two values, not one**: the rule asks
        // whether the *provider's* poster is on disk, while the entry carries whichever poster there is to
        // draw.
        let provider_poster = self.provider_poster(row.library.poster.as_ref());
        let local_poster = crate::local_posters::cached(&self.artwork, id);
        let needs_poster = provider_poster.is_none() && local_poster.is_none();
        let poster = provider_poster.clone().or(local_poster);
        let backdrop = row.backdrop.as_ref().and_then(|artwork| {
            crate::artwork::cached_image(
                &self.artwork,
                artwork.provider,
                &artwork.path,
                peregrine_domain::catalogue::ImageKind::Backdrop,
            )
        });
        let state = crate::artwork::ArtworkState::of(
            &row,
            provider_poster.as_deref(),
            self.provider_can_serve(id).await?,
        );
        let playback = self.title_playback(id).await?;
        Ok(Some(LibraryTitleDetail {
            refresh_due: crate::artwork::refresh_due(&state),
            playback,
            entry: LibraryTitleEntry {
                row: row.library,
                poster,
                needs_poster,
            },
            backdrop,
            overview: row.overview,
            rating: row.rating,
            released: row.released,
        }))
    }

    /// Whether this title has no poster for a capture to improve on.
    ///
    /// **The rule, in one place, because two commands ask it**: the player captures a frame while it
    /// plays, and the library backfills one for every title that has none. Both must answer the same way,
    /// or the backfill would fight the player for the same blob.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Storage`] when the catalogue row cannot be read.
    async fn needs_poster(&self, title_id: i64) -> Result<bool, RuntimeError> {
        let Some(row) = self
            .database
            .library_title(installation_scope(), title_id)
            .await?
        else {
            return Err(RuntimeError::UnknownTitle);
        };
        Ok(self.provider_poster(row.library.poster.as_ref()).is_none()
            && crate::local_posters::cached(&self.artwork, title_id).is_none())
    }

    /// One list row plus the cached poster the cache holds for it.
    ///
    /// The one place a title's primary poster reference becomes a path, so the table and the grid
    /// cannot resolve it two ways: the reference is the row's, and the `stat` that decides whether the
    /// blob is on disk is the cache's (AGENTS R7). A row whose reference is absent and a row whose
    /// blob was evicted both answer `None`, which is the reserved box either way.
    fn listed_title(&self, row: peregrine_storage::LibraryTitleRow) -> LibraryTitleEntry {
        let provider = self.provider_poster(row.poster.as_ref());
        let local = crate::local_posters::cached(&self.artwork, row.title.id);
        LibraryTitleEntry {
            needs_poster: provider.is_none() && local.is_none(),
            poster: provider.or(local),
            row,
        }
    }

    /// The cached file a provider artwork reference names, or `None` when the cache holds none.
    ///
    /// Split out of [`Runtime::listed_title`] because a title's own read needs the same answer without
    /// taking the row apart yet: its refresh rule reads the whole row, and the entry it returns owns
    /// half of it.
    /// **The provider's answer alone, and that distinction is load-bearing.** The refresh rule
    /// ([`crate::artwork::refresh_due`]) asks whether the cache holds *the provider's* poster, and a
    /// locally generated frame standing in for it here would make the rule answer "cached" for a title no
    /// provider has ever drawn — which would stop the one request that could replace a film still with a
    /// real poster. The fallback belongs to what a screen draws, not to what is due.
    fn provider_poster(
        &self,
        reference: Option<&peregrine_storage::ArtworkPath>,
    ) -> Option<PathBuf> {
        reference.and_then(|artwork| {
            crate::artwork::cached_image(
                &self.artwork,
                artwork.provider,
                &artwork.path,
                peregrine_domain::catalogue::ImageKind::Poster,
            )
        })
    }

    /// What one title's own page may offer and would do, from one read of the evidence the library
    /// holds of it.
    ///
    /// The wiring rather than the rule: the derivations are [`peregrine_domain::availability`] per
    /// item, [`peregrine_domain::TitleOffer`] over the title, and
    /// [`peregrine_domain::LocalPlayback`] over the file that would play — and what this owns is
    /// feeding them from the row's own sets rather than from a count (AGENTS R7).
    ///
    /// **`Asked::Nothing`, and it is exact rather than a placeholder.** [`peregrine_domain::Asked`]
    /// is the one input the item row cannot supply — SM2's three pre-acquisition states collapse into
    /// `wanted` there — and it is the watchlist that tells `watchlisted` from `discovered`. Nothing in
    /// this build writes `watchlist_entry` and no command requests an acquisition, so nothing can have
    /// asked for anything yet; the title-scoped read that will pass this in is `PG-E9-05`'s.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Storage`] when the evidence cannot be read.
    async fn title_playback(&self, title_id: i64) -> Result<TitlePlayback, RuntimeError> {
        let evidence = self
            .database
            .title_evidence(installation_scope(), title_id)
            .await?;
        let items: Vec<peregrine_domain::Availability> = evidence
            .items
            .iter()
            .map(|item| {
                peregrine_domain::availability(
                    &item.item,
                    peregrine_domain::Asked::Nothing,
                    &item.jobs,
                    &item.files,
                )
            })
            .collect();
        Ok(TitlePlayback {
            offer: peregrine_domain::TitleOffer::of(&items, &evidence.jobs),
            support: playable_file(&evidence, None)
                .map(|file| LocalPlayback::of_name(file.path.as_str())),
        })
    }

    /// Whether a configured provider owns an identifier `title_id` carries.
    ///
    /// The question a refresh's usefulness turns on, and the reason it is asked here rather than by a
    /// screen: it needs the credential store's answer (`PG-E5-12`'s registry, read once at startup and
    /// never on a command path) together with the ids the catalogue holds. Nothing leaves the machine —
    /// this compares what was already known.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Storage`] when the ids cannot be read.
    async fn provider_can_serve(&self, title_id: i64) -> Result<bool, RuntimeError> {
        let configured: Vec<peregrine_domain::Provider> = self
            .providers
            .adapters()
            .iter()
            .map(|adapter| adapter.descriptor().provider())
            .collect();
        let held: Vec<peregrine_domain::ExternalId> = self
            .database
            .external_ids_of(title_id)
            .await?
            .into_iter()
            .map(|one| one.id)
            .collect();
        Ok(crate::artwork::can_answer(&configured, &held))
    }

    /// A title's episodes, in the order a person watches them.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Library`] when the database refuses, which is the only failure this has.
    pub async fn title_episodes(
        &self,
        title_id: i64,
        season_number: Option<u32>,
        limit: u32,
    ) -> Result<Vec<TitleEpisodeDetail>, RuntimeError> {
        let rows = self
            .database
            .list_title_episodes(installation_scope(), title_id, season_number, limit)
            .await?;
        // One `stat` per episode, and no request: resolving a still is asking the cache whether it
        // holds a blob whose key this layer computes, which is why it cannot be the screen's job
        // (AGENTS R7) — the key is a hash and only the core knows whether the file is there.
        Ok(rows
            .into_iter()
            .map(|row| {
                let still = row.still.as_ref().and_then(|artwork| {
                    crate::artwork::cached_image(
                        &self.artwork,
                        artwork.provider,
                        &artwork.path,
                        peregrine_domain::catalogue::ImageKind::Still,
                    )
                });
                TitleEpisodeDetail { row, still }
            })
            .collect())
    }

    /// A title's seasons, in the order a switcher lists them, each with its cached poster.
    ///
    /// The same shape as the episode read above and for the same reason: the artwork's key is this
    /// layer's to compute, and a screen that resolved one would be hashing a provider's path itself.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Library`] when the database refuses, which is the only failure this has.
    pub async fn title_seasons(
        &self,
        title_id: i64,
        limit: u32,
    ) -> Result<Vec<TitleSeasonDetail>, RuntimeError> {
        let rows = self
            .database
            .list_title_seasons(installation_scope(), title_id, limit)
            .await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let poster = row.poster.as_ref().and_then(|artwork| {
                    crate::artwork::cached_image(
                        &self.artwork,
                        artwork.provider,
                        &artwork.path,
                        peregrine_domain::catalogue::ImageKind::Poster,
                    )
                });
                TitleSeasonDetail { row, poster }
            })
            .collect())
    }

    /// A page of the titles the library holds, newest first.
    ///
    /// Read under [`installation_scope`], which is the whole of this build's account story: the
    /// screen asks what *it* has, and on an installation with one account that is everything. Each row
    /// leaves with the cached poster the cache holds for it, because the screen that draws this read is
    /// a grid of tiles as often as it is a table of counts, and a tile cannot resolve a cache key.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Library`] when the database refuses; a poster the cache does not hold is `None`
    /// rather than a failure, because a reserved box is a state the tile already draws.
    pub async fn library_titles(
        &self,
        after: Option<i64>,
        limit: u32,
    ) -> Result<Vec<LibraryTitleEntry>, RuntimeError> {
        let request = match after {
            Some(cursor) => PageRequest::after(Cursor::new(cursor), limit),
            None => PageRequest::first(limit),
        };
        let rows = self
            .database
            .list_library_titles(installation_scope(), request)
            .await?;
        Ok(rows.into_iter().map(|row| self.listed_title(row)).collect())
    }

    /// The file a title would play, the guarded path it lives at, and what this build would do with it.
    ///
    /// **One function, because two commands need the same four answers** and the two must agree about
    /// which file they are describing: a capture session that chose a different copy from the playback
    /// session would put a poster from one release on a tile whose Play button opens another. The four
    /// steps are the title's existence under the scope (I37), the file the library actually has, the
    /// containment check that produces a [`SafePath`], and the container's name.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::UnknownTitle`], [`RuntimeError::NoPlayableFile`], or
    /// [`RuntimeError::FileOutsideRoots`].
    async fn playable(
        &self,
        title_id: i64,
        episode_id: Option<i64>,
    ) -> Result<(peregrine_domain::MediaFile, SafePath), RuntimeError> {
        if self
            .database
            .library_title(installation_scope(), title_id)
            .await?
            .is_none()
        {
            return Err(RuntimeError::UnknownTitle);
        }
        let evidence = self
            .database
            .title_evidence(installation_scope(), title_id)
            .await?;
        let Some(file) = playable_file(&evidence, episode_id) else {
            return Err(RuntimeError::NoPlayableFile);
        };
        let safe = self
            .guarded_media_path(Path::new(file.path.as_str()))
            .await?;
        Ok((file.clone(), safe))
    }

    /// Opens a session for one of a title's files so the renderer can capture a poster frame.
    ///
    /// **The same machinery as [`Runtime::start_playback`] and a separate command on purpose.** One file,
    /// one listener, one token — and a different *intent*, which is the whole reason the name is
    /// different: a reader who found `start_playback` called by a poster backfill could not tell whether
    /// the film was meant to be playing. Nothing plays a capture session: the renderer loads the URL in an
    /// element it never shows, draws one frame, posts it, and this is closed.
    ///
    /// It holds its own slot, so a backfill cannot stop a film and a film cannot stop a backfill.
    ///
    /// # Errors
    ///
    /// The same four refusals [`Runtime::start_playback`] documents.
    pub async fn start_poster_capture(
        &self,
        title_id: i64,
    ) -> Result<StartedCapture, RuntimeError> {
        // Whatever capture was open stops first, and by its own id rather than by "current": the two are
        // the same thing here, and naming it is the habit that keeps a stale stop from being able to close
        // a session it does not own.
        let opening = *self
            .capture_id
            .lock()
            .expect("the capture id lock is not poisoned");
        if let Some(held) = opening {
            self.stop_poster_capture(held).await;
        }

        let (file, safe) = self.playable(title_id, None).await?;
        let media_file_id = file.id;
        let session = peregrine_playback::PlaybackSession::open(
            media_file_id,
            title_id,
            &safe,
            self.tokens.next_token(),
            Some(Arc::clone(&self.posters) as Arc<dyn peregrine_stream::ThumbnailSink>),
        )
        .await
        .map_err(playback_error)?;
        let url = session.url().to_owned();
        let thumbnail_url = session.thumbnail_url().map(str::to_owned);
        let session_id = self.mint_session();
        *self
            .capture
            .lock()
            .expect("the capture lock is not poisoned") = Some(session);
        *self
            .capture_id
            .lock()
            .expect("the capture id lock is not poisoned") = Some(session_id);

        Ok(StartedCapture {
            session_id,
            title_id,
            media_file_id,
            url,
            thumbnail_url,
        })
    }

    /// Stops the capture session `session_id` names, answering whether that one was open.
    ///
    /// **A stop that names the wrong session does nothing, and that is the point.** A renderer's cleanup
    /// runs while the next mount's open may already have landed; a stop that closed "whatever is current"
    /// would kill the session the screen is reading. `false` is the honest answer: nothing of *yours* was
    /// open.
    pub async fn stop_poster_capture(&self, session_id: SessionId) -> bool {
        if *self
            .capture_id
            .lock()
            .expect("the capture id lock is not poisoned")
            != Some(session_id)
        {
            return false;
        }
        let session = self
            .capture
            .lock()
            .expect("the capture lock is not poisoned")
            .take();
        *self
            .capture_id
            .lock()
            .expect("the capture id lock is not poisoned") = None;
        match session {
            Some(session) => session.close().await,
            None => false,
        }
    }

    /// The engine-backed source for a title whose download is still running, when there is one.
    ///
    /// `None` covers every reason there is nothing to stream — no engine, no live handle for the job
    /// (a job another process started has none here), no selected media file, or no job for the title
    /// at all. The caller turns `None` into the same refusal a title with nothing on disk gets.
    async fn torrent_media(
        &self,
        title_id: i64,
    ) -> Result<Option<Arc<dyn peregrine_stream::TorrentMedia>>, RuntimeError> {
        let (Ok(engine), Ok(acquisition)) = (self.torrent_engine(), self.acquisition()) else {
            return Ok(None);
        };
        let scope = installation_scope();
        let jobs = self
            .database
            .list_download_jobs(
                scope,
                peregrine_domain::DownloadState::ALL,
                peregrine_storage::PageRequest::first(50),
            )
            .await?;
        let Some(job) = jobs.into_iter().find(|job| job.title_id == title_id) else {
            return Ok(None);
        };
        let Some(handle) = acquisition.handle_for(job.id) else {
            return Ok(None);
        };
        let files = self.database.download_files(scope, job.id).await?;
        let Some(file) = files
            .into_iter()
            .filter(|file| file.selected && file.is_media)
            .max_by_key(|file| file.size_bytes)
        else {
            return Ok(None);
        };
        let index = usize::try_from(file.torrent_file_index).unwrap_or_default();
        Ok(Some(Arc::new(crate::media_source::EngineMedia::new(
            engine,
            handle,
            index,
            file.size_bytes,
            file.path.as_str(),
        ))))
    }

    /// Starts playing one of a title's files, replacing whatever was playing.
    ///
    /// **Four decisions, in the order they matter.** The title is resolved under the installation scope,
    /// so a title this account may not see is the same refusal as one that does not exist (I37). The
    /// file is chosen by [`playable_file`] — what the library actually has, not what a count says. The
    /// path is put through a [`PathGuard`] built from the configured roots, so the loopback server can
    /// only ever be handed a [`SafePath`]. And the session replaces the previous one rather than
    /// competing with it, which is what makes the listener's lifetime the session's (SECURITY.md T10).
    ///
    /// A title with nothing on disk but an active download is served from the engine instead, which is
    /// what lets a film be watched before its last pieces arrive.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::UnknownTitle`] when no title the account may see has that id,
    /// [`RuntimeError::NoPlayableFile`] when it holds none that is on disk and none is downloading,
    /// [`RuntimeError::FileOutsideRoots`] when what it holds is outside every configured folder, and
    /// [`RuntimeError::Playback`] when the file cannot be served.
    pub async fn start_playback(
        &self,
        title_id: i64,
        episode: Option<crate::jobs::acquire::EpisodePick>,
    ) -> Result<StartedPlayback, RuntimeError> {
        let episode_id = match episode {
            Some(pick) => {
                self.database
                    .episode_id_for(title_id, pick.season, pick.episode)
                    .await?
            }
            None => None,
        };
        // A webview play replaces an mpv window for the same reason an mpv play replaces the
        // webview session: one press is one player.
        self.external_player.stop();
        // Whatever was playing stops first, and it stops before the new one is resolved: two listeners for
        // one screen would be two sockets the runtime could not both own. Named by its id rather than by
        // "current" — see `SessionId`, and the black rectangle a stale stop produced.
        let opening = *self
            .session_id
            .lock()
            .expect("the session id lock is not poisoned");
        if let Some(held) = opening {
            self.stop_playback(held).await;
        }

        let (media_file_id, support, needs_poster, session) =
            match self.playable(title_id, episode_id).await {
                Ok((file, safe)) => {
                    let support = LocalPlayback::of_name(file.path.as_str());
                    // Whether a poster is worth capturing on the way, asked from the same read the play
                    // needs: only when there is nothing to draw, which is the rule that stops a film still
                    // replacing real artwork.
                    let needs_poster = self.needs_poster(title_id).await?;
                    let session = peregrine_playback::PlaybackSession::open(
                        file.id,
                        title_id,
                        &safe,
                        self.tokens.next_token(),
                        Some(Arc::clone(&self.posters) as Arc<dyn peregrine_stream::ThumbnailSink>),
                    )
                    .await
                    .map_err(playback_error)?;
                    (file.id, support, needs_poster, session)
                }
                // A title with nothing on disk may still be downloading: serve it from the engine so a film
                // can be watched while its last pieces arrive.
                Err(RuntimeError::NoPlayableFile) if episode_id.is_none() => {
                    match self.torrent_media(title_id).await? {
                        Some(media) => {
                            let session = peregrine_playback::PlaybackSession::open_torrent(
                                title_id,
                                media,
                                self.tokens.next_token(),
                                Some(Arc::clone(&self.posters)
                                    as Arc<dyn peregrine_stream::ThumbnailSink>),
                            )
                            .await
                            .map_err(playback_error)?;
                            (0, LocalPlayback::Unknown, false, session)
                        }
                        None => return Err(RuntimeError::NoPlayableFile),
                    }
                }
                // A torrent holds one media stream for the title and no episode identity, so serving it
                // for a chosen episode would play whatever the torrent happens to contain first.
                Err(RuntimeError::NoPlayableFile) => return Err(RuntimeError::NoPlayableFile),
                Err(other) => return Err(other),
            };
        let url = session.url().to_owned();
        let thumbnail_url = session.thumbnail_url().map(str::to_owned);
        // The guard is dropped before the answer is built: nothing below this line awaits while holding
        // it, which is the rule a `std::sync::Mutex` in an async runtime exists under.
        let session_id = self.mint_session();
        *self
            .session
            .lock()
            .expect("the session lock is not poisoned") = Some(session);
        *self
            .session_id
            .lock()
            .expect("the session id lock is not poisoned") = Some(session_id);

        // Read after the session opens rather than before: a title that cannot play at all should
        // not pay for a progress read, and the answer is the same either way.
        let resume_position_ms = self
            .database
            .watch_progress(installation_scope(), title_id, None)
            .await?
            .map(|progress| progress.position_ms());

        Ok(StartedPlayback {
            session_id,
            title_id,
            episode_id,
            media_file_id,
            support,
            url,
            thumbnail_url,
            needs_poster,
            resume_position_ms,
        })
    }

    /// Plays one of a title's files in mpv, for a container the webview cannot demux.
    ///
    /// The title and path resolution is [`Runtime::start_playback`]'s, but the bytes never cross
    /// this process: mpv opens the file itself. Whatever the webview was serving is stopped first,
    /// because two players for one press is one too many.
    ///
    /// # Errors
    ///
    /// The same refusals as [`Runtime::start_playback`], plus [`RuntimeError::Player`] when mpv is
    /// not installed or the operating system would not start it.
    pub async fn start_external_playback(&self, title_id: i64) -> Result<(), RuntimeError> {
        let opening = *self
            .session_id
            .lock()
            .expect("the session id lock is not poisoned");
        if let Some(held) = opening {
            self.stop_playback(held).await;
        }

        let (_file, safe) = self.playable(title_id, None).await?;
        // The app's own watch progress is the resume rule; mpv's watch_later is disabled.
        let resume_ms = self
            .database
            .watch_progress(installation_scope(), title_id, None)
            .await?
            .map(|progress| progress.position_ms());

        // The child reports over its IPC socket; this sink hands each position to a task that writes
        // it through the same progress path the in-page player uses.
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let database = Arc::clone(&self.database);
        let clock = Arc::clone(&self.clock);
        tokio::spawn(async move {
            while let Some((position_ms, duration_ms)) = receiver.recv().await {
                let Ok(position) = PlaybackPosition::new(position_ms, duration_ms) else {
                    continue;
                };
                let progress = WatchProgress::record(
                    installation_scope().account(),
                    title_id,
                    None,
                    position,
                    ProgressSource::Local,
                    clock.now(),
                    None,
                );
                if let Err(error) = database.record_watch_progress(progress).await {
                    warn!(%error, title = title_id, "an external player position could not be recorded");
                }
            }
        });

        let player = Arc::clone(&self.external_player);
        let positions: Arc<dyn peregrine_domain::PositionSink> = Arc::new(ChannelSink(sender));
        tokio::task::spawn_blocking(move || player.play(&safe, resume_ms, positions))
            .await
            .map_err(|_| peregrine_domain::PlayerError::Spawn)??;
        Ok(())
    }
    /// A page of what the installation account is part-way through, most recently updated first.
    ///
    /// The account is the installation scope for the same reason every other read uses it (R34); the
    /// page is small because a rail shows a handful.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Storage`] when the read fails.
    /// The quality downloads prefer, as the default profile currently spells it.
    ///
    /// None means the stored list is one no choice names — a hand-edited profile — which the
    /// screen reports rather than silently relabelling.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the read fails.
    pub async fn preferred_quality(&self) -> Result<Option<PreferredQuality>, RuntimeError> {
        let profile = self.database.default_quality_profile().await?;
        Ok(profile
            .and_then(|profile| PreferredQuality::of_resolutions(profile.rules().resolutions())))
    }

    /// Records the quality downloads should prefer.
    ///
    /// The choice is written as the default profile's resolution list, because that list is the
    /// ranker's accept list: a resolution it omits is not a candidate, so naming 2160p is what
    /// makes a 4K release available again and naming 1080p is what keeps one out.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the write fails, and `RuntimeError::InvalidRequest` when there
    /// is no default profile to write.
    pub async fn set_preferred_quality(
        &self,
        quality: PreferredQuality,
    ) -> Result<(), RuntimeError> {
        let resolutions = serde_json::to_string(quality.resolutions()).map_err(|error| {
            RuntimeError::InvalidRequest {
                detail: error.to_string(),
            }
        })?;
        let wrote = self
            .database
            .set_default_quality_resolutions(&resolutions, self.clock.now().as_millisecond())
            .await?;
        if !wrote {
            return Err(RuntimeError::InvalidRequest {
                detail: "no default quality profile to write".to_owned(),
            });
        }
        Ok(())
    }

    /// The titles this account has started and not finished, most recent first.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the read fails.
    pub async fn continue_watching(&self) -> Result<Vec<WatchProgressRow>, RuntimeError> {
        Ok(self
            .database
            .list_continue_watching(installation_scope(), PageRequest::first(20))
            .await?)
    }

    /// The account's viewing history, newest first, for the settings list.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the read fails.
    pub async fn watch_history(&self, limit: u32) -> Result<Vec<WatchProgressRow>, RuntimeError> {
        Ok(self
            .database
            .list_watch_progress(installation_scope(), PageRequest::first(limit))
            .await?)
    }

    /// Queues a metadata refresh for the newest scanned titles that are still stubs.
    ///
    /// The matcher runs without a person asking: a scan writes titles it cannot fill, and a library
    /// that stayed grey until each title was opened would be a library nobody browses. The bound is
    /// deliberate — twenty a pass, and the provider's own rate limits do the rest.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the read fails. A title the scheduler refuses is skipped rather
    /// than failing the pass, so one bad row cannot stop the rest.
    pub async fn match_stub_titles(&self) -> Result<usize, RuntimeError> {
        if self.providers.adapters().is_empty() {
            return Ok(0);
        }
        let stubs = self.database.stub_titles(installation_scope(), 20).await?;
        // One attempt per stub per session: a title TMDB cannot name stays a stub, and retrying it
        // every minute would be a search nobody asked for.
        let pending: Vec<_> = {
            let mut attempted = self
                .attempted_matches
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            stubs
                .into_iter()
                .filter(|stub| attempted.insert(stub.id))
                .collect()
        };
        let mut queued = 0;
        for stub in pending {
            match self.start_provider_refresh(stub.id, stub.kind).await {
                Ok(_) => queued += 1,
                Err(error) => {
                    tracing::debug!(%error, title = stub.id, "a stub title was not queued");
                }
            }
        }
        Ok(queued)
    }

    /// Adds a title to the account's watchlist, or refreshes the row that is already there.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the write fails.
    pub async fn add_to_watchlist(&self, title_id: i64) -> Result<(), RuntimeError> {
        let entry = peregrine_domain::WatchlistEntry::new(
            installation_scope().account(),
            title_id,
            self.clock.now(),
        );
        self.database.add_to_watchlist(entry).await?;
        Ok(())
    }

    /// Removes a title from the watchlist, answering whether it was on it.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the delete fails.
    pub async fn remove_from_watchlist(&self, title_id: i64) -> Result<bool, RuntimeError> {
        Ok(self
            .database
            .remove_from_watchlist(installation_scope().account(), title_id)
            .await?)
    }

    /// The account's watchlist, newest first.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the read fails.
    pub async fn watchlist(&self) -> Result<Vec<peregrine_storage::WatchlistRow>, RuntimeError> {
        Ok(self
            .database
            .list_watchlist(installation_scope(), PageRequest::first(200))
            .await?)
    }

    /// One title's per-episode watch state, which the season list draws.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the read fails.
    pub async fn title_watch_progress(
        &self,
        title_id: i64,
    ) -> Result<Vec<peregrine_storage::EpisodeProgressRow>, RuntimeError> {
        Ok(self
            .database
            .list_progress_for_title(installation_scope(), title_id)
            .await?)
    }

    /// Schedules a download for the first date it may be fetched.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the write fails.
    pub async fn schedule_download(
        &self,
        title_id: i64,
        episode: Option<crate::jobs::acquire::EpisodePick>,
        release_on: String,
    ) -> Result<(), RuntimeError> {
        let episode_id = match episode {
            Some(pick) => {
                self.database
                    .episode_id_for(title_id, pick.season, pick.episode)
                    .await?
            }
            None => None,
        };
        let schedule = peregrine_storage::NewSchedule {
            account_id: installation_scope().account().get(),
            title_id,
            episode_id,
            release_on,
            created_at: self.clock.now(),
        };
        self.database.upsert_schedule(schedule).await?;
        Ok(())
    }

    /// Every schedule the account holds, soonest first.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the read fails.
    pub async fn download_schedules(
        &self,
    ) -> Result<Vec<peregrine_storage::DownloadScheduleRow>, RuntimeError> {
        Ok(self.database.list_schedules(installation_scope()).await?)
    }

    /// Cancels one schedule.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the delete fails.
    pub async fn cancel_schedule(&self, id: i64) -> Result<(), RuntimeError> {
        self.database.delete_schedule(id).await?;
        Ok(())
    }

    /// Starts every schedule whose date has arrived and marks the outcome on its row.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when a read or a state write fails. A failed acquisition is
    /// recorded on its schedule rather than returned, because one broken release must not stop
    /// the rest.
    pub async fn run_due_schedules(&self) -> Result<(), RuntimeError> {
        let now = self.clock.now();
        let stamp = now.to_string();
        let today = stamp.get(..10).unwrap_or(stamp.as_str()).to_owned();
        let due = self
            .database
            .due_schedules(installation_scope(), &today)
            .await?;
        for schedule in due {
            self.database
                .set_schedule_state(schedule.id, "started", None, now)
                .await?;
            let episode = match schedule.episode_id {
                Some(id) => self
                    .database
                    .episode_numbers(id)
                    .await?
                    .map(|(season, episode)| crate::jobs::acquire::EpisodePick { season, episode }),
                None => None,
            };
            match self
                .start_acquire(installation_scope(), schedule.title_id, episode, 1)
                .await
            {
                Ok(_) => {}
                Err(error) => {
                    let message = error.to_string();
                    self.database
                        .set_schedule_state(schedule.id, "failed", Some(&message), now)
                        .await?;
                }
            }
        }
        Ok(())
    }

    /// One episode's season and number, which a resume row needs to address the player.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the read fails.
    pub async fn episode_numbers(
        &self,
        episode_id: i64,
    ) -> Result<Option<(u32, u32)>, RuntimeError> {
        Ok(self.database.episode_numbers(episode_id).await?)
    }

    /// Records where the player is in a title or episode.
    ///
    /// The value is the domain's, so the percentage is one derivation (I6) and a position reported
    /// twice updates a single row. It is deliberately stored unwatched: whether a position finishes a
    /// title is the watch policy's decision at the end of a session, not the player's at every tick.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Storage`] when the write fails, and [`RuntimeError::InvalidRequest`] for a
    /// negative position or duration.
    pub async fn record_progress(
        &self,
        title_id: i64,
        episode_id: Option<i64>,
        position_ms: i64,
        duration_ms: Option<i64>,
    ) -> Result<(), RuntimeError> {
        let position = PlaybackPosition::new(position_ms, duration_ms).map_err(|error| {
            RuntimeError::InvalidRequest {
                detail: error.to_string(),
            }
        })?;
        let progress = WatchProgress::record(
            installation_scope().account(),
            title_id,
            episode_id,
            position,
            ProgressSource::Local,
            self.clock.now(),
            None,
        );
        self.database.record_watch_progress(progress).await?;
        Ok(())
    }

    /// Stops the playback session `session_id` names, answering whether that one was open.
    ///
    /// The same identity rule [`Runtime::stop_poster_capture`] documents, and it matters more here: a stale
    /// stop closing the live session is a film that dies the moment it starts.
    ///
    /// The session is taken out of the lock and closed *after* the guards are dropped, so closing — which
    /// awaits the listener's task — never happens under a lock another command could want.
    pub async fn stop_playback(&self, session_id: SessionId) -> bool {
        if *self
            .session_id
            .lock()
            .expect("the session id lock is not poisoned")
            != Some(session_id)
        {
            return false;
        }
        let session = self
            .session
            .lock()
            .expect("the session lock is not poisoned")
            .take();
        *self
            .session_id
            .lock()
            .expect("the session id lock is not poisoned") = None;
        match session {
            Some(session) => session.close().await,
            None => false,
        }
    }

    /// The next session id, in issue order.
    fn mint_session(&self) -> SessionId {
        SessionId(
            self.next_session
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        )
    }

    /// Whether something is playing.
    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.session
            .lock()
            .expect("the session lock is not poisoned")
            .is_some()
    }

    /// The media path as the filesystem layer will accept it, or a refusal naming why not.
    ///
    /// A `PathGuard` per configured root and the first that contains the path wins. **Every root is
    /// tried rather than the file's own `root_id`**, because that column is nullable — a file recorded
    /// before its root was known has none — and a check that answered `None` for such a file would
    /// refuse a file the user can see in their own library. The guard is what re-establishes
    /// containment: the scanner writes paths into a database, and the database is not the filesystem
    /// (SECURITY.md's path-safety row).
    ///
    /// # Errors
    ///
    /// [`RuntimeError::FileOutsideRoots`] when no configured root contains the path,
    /// [`RuntimeError::Storage`] when the roots cannot be read.
    async fn guarded_media_path(&self, path: &Path) -> Result<SafePath, RuntimeError> {
        let roots = self
            .library()
            .roots(PageRequest::first(MAX_MEDIA_ROOTS))
            .await?;
        for root in roots {
            let Ok(guard) = peregrine_domain::PathGuard::new(Path::new(root.root.path.as_str()))
            else {
                continue;
            };
            if let Ok(safe) = guard.resolve(path) {
                return Ok(safe);
            }
        }
        // The app's own download root, which is not a configured library root: a completed download
        // lives there, and a playback that only tried the roots would refuse the file Peregrine
        // itself wrote. The guard is built the same way, so containment is still proven.
        if let Ok(guard) =
            peregrine_domain::PathGuard::new(self.config.data_directory().join(DOWNLOAD_DIRECTORY))
            && let Ok(safe) = guard.resolve(path)
        {
            return Ok(safe);
        }
        Err(RuntimeError::FileOutsideRoots)
    }

    /// The library roots, newest first.
    ///
    /// A root is a directory the user configured, so there is no `AccountScope` here and none is
    /// accepted: what is scoped is the content inside the roots, not the list of them.
    pub async fn library_roots(
        &self,
        after: Option<i64>,
        limit: u32,
    ) -> Result<Vec<LibraryRootRow>, RuntimeError> {
        let request = match after {
            Some(cursor) => PageRequest::after(Cursor::new(cursor), limit),
            None => PageRequest::first(limit),
        };
        Ok(self.library().roots(request).await?)
    }

    /// Adds a directory as a library root.
    ///
    /// The path arrives from the shell's own folder picker, which hands it over as text it has not
    /// validated (`UntrustedPath`, PG-E1-12); `PathGuard` in the service is what decides whether it
    /// can be a root, and this is where that refusal becomes a structured error the UI can show.
    pub async fn add_library_root(
        &self,
        path: &str,
        kind: LibraryRootKind,
        scan_mode: ScanMode,
    ) -> Result<LibraryRoot, RuntimeError> {
        let now = self.clock.now();
        Ok(self
            .library()
            .add_root(Path::new(path), kind, scan_mode, now)
            .await?)
    }

    /// Removes a library root, reporting whether it was there.
    ///
    /// Nothing else is removed with it: the files, the items, and the watch progress all stay
    /// (PG-E14-01), which is why this returns a `bool` rather than a plan the caller has to
    /// confirm. There is no destructive step here to confirm.
    pub async fn remove_library_root(&self, id: i64) -> Result<bool, RuntimeError> {
        Ok(self.library().remove_root(id).await?)
    }

    /// The library service over this runtime's database.
    ///
    /// Built on demand rather than held: it is two fields of an `Arc`, and a service that outlives
    /// its runtime is the kind of thing that keeps a database handle alive after shutdown.
    fn library(&self) -> LibraryService {
        LibraryService::new(Arc::clone(&self.database))
    }

    /// Starts a scan of every library root, or says that one is already running.
    ///
    /// # Why this is not deduped by the scheduler
    ///
    /// `PG-E6-01`'s scheduler can coalesce a submission onto a live job that holds the same
    /// `dedupe_key`, and that is the obvious way to say "only one scan at a time". It is not what
    /// this does, because a coalesced handle's state never reaches a terminal one — the coalesce
    /// path resolves the caller's completion but sends no state change — so a runtime that had
    /// inserted a journal row for it would hold a row that stays `queued` forever and a live entry
    /// that is pruned only when the shutdown sweep runs. The pre-check below is the honest version
    /// of the same intention: a second request while a scan is live is refused by name.
    ///
    /// Two scans that did start together would not corrupt anything: each reconciles only on a
    /// complete walk of a root, so their conclusions agree and the cost is duplicated IO. The check
    /// is here to avoid paying it, not to keep the library consistent.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::ScanAlreadyRunning`] when a scan is live, and [`RuntimeError::Storage`] or
    /// [`RuntimeError::Scheduler`] when the job cannot be journaled or accepted.
    pub async fn start_library_scan(&self) -> Result<StartedJob, RuntimeError> {
        if let Some(running) = self.live_scan() {
            return Err(RuntimeError::ScanAlreadyRunning { job: running });
        }

        let now = self.clock.now().as_millisecond();
        let journal_id = self
            .database
            .insert_job(NewJob {
                kind: library_scan::KIND.to_owned(),
                priority: library_scan::PRIORITY,
                // No dedupe key, for the reason in this method's own doc.
                dedupe_key: None,
                payload_json: json!({ "roots": true }).to_string(),
                max_attempts: 1,
                created_at: now,
            })
            .await?;

        let spec = JobSpec {
            kind: library_scan::KIND.to_owned(),
            priority: library_scan::PRIORITY,
            dedupe_key: None,
            max_attempts: 1,
        };
        let job = LibraryScanJob::new(
            Arc::clone(&self.database),
            Arc::clone(&self.bus),
            Arc::clone(&self.clock),
            journal_id,
        );

        let handle = match self.scheduler.submit(spec, job).await {
            Ok(handle) => handle,
            Err(error) => {
                // The row exists and will never run, so leaving it `queued` would show a scan
                // waiting forever in the UI. Closed here for the same reason the hello job closes
                // its own row on a refused submission.
                let _ = self
                    .database
                    .update_job(
                        journal_id,
                        JobChange::Finished {
                            at: now,
                            phase: JobPhase::Failed,
                            error_code: Some(INTERNAL_CODE.to_owned()),
                            error: Some(error.to_string()),
                        },
                    )
                    .await;
                return Err(RuntimeError::Scheduler(error));
            }
        };

        let started = StartedJob {
            job: handle.id(),
            journal_id,
            coalesced: false,
        };
        info!(job = %started.job, journal_id, "accepted the library scan");
        self.remember(handle, journal_id);
        Ok(started)
    }

    /// The live scan job's id, when one is running.
    fn live_scan(&self) -> Option<Ulid> {
        self.live()
            .values()
            .find(|entry| entry.handle.state().kind() == library_scan::KIND)
            .map(|entry| entry.handle.id())
    }

    /// One page of journal rows, newest first.
    ///
    /// `after` is the id of the previous page's last row; `None` reads the newest page. The
    /// cursor is a bare `i64` here rather than in the command surface because the journal
    /// screen shows one page, and `specta` cannot export a large integer as a command
    /// argument without a DTO to carry it — the repository's capability is what matters.
    pub async fn jobs(
        &self,
        after: Option<i64>,
        limit: u32,
    ) -> Result<Vec<JobRecord>, RuntimeError> {
        let request = match after {
            Some(cursor) => PageRequest::after(Cursor::new(cursor), limit),
            None => PageRequest::first(limit),
        };
        Ok(self.database.list_jobs(request).await?)
    }

    /// The in-app notification centre, newest first.
    ///
    /// R31 makes the centre row the record and the OS toast the bonus, so this read is how a
    /// notification reaches the user at all while `PG-E1-14`'s notifier is unwritten.
    ///
    /// # Errors
    ///
    /// `CentreError` when the centre could not be read; the command maps it to a typed refusal.
    pub async fn list_notifications(&self, limit: u32) -> Result<Vec<CentreEntry>, CentreError> {
        NotificationRepository::new(Arc::clone(&self.database))
            .list(installation_scope(), None, limit)
            .await
    }

    /// Marks one centre row read, under the installation scope.
    ///
    /// # Errors
    ///
    /// `CentreError` when the centre could not be written.
    pub async fn mark_notification_read(&self, id: i64) -> Result<bool, CentreError> {
        let at = self.clock.now().as_millisecond();
        NotificationRepository::new(Arc::clone(&self.database))
            .mark_read(installation_scope(), NotificationId(id), at)
            .await
    }

    /// Marks every unread centre row read, answering how many changed.
    ///
    /// # Errors
    ///
    /// `CentreError` when the centre could not be written.
    pub async fn mark_all_notifications_read(&self) -> Result<u64, CentreError> {
        let at = self.clock.now().as_millisecond();
        NotificationRepository::new(Arc::clone(&self.database))
            .mark_all_read(installation_scope(), at)
            .await
    }

    /// Dismisses one centre row, under the installation scope.
    ///
    /// # Errors
    ///
    /// `CentreError` when the centre could not be written.
    pub async fn dismiss_notification(&self, id: i64) -> Result<bool, CentreError> {
        let at = self.clock.now().as_millisecond();
        NotificationRepository::new(Arc::clone(&self.database))
            .dismiss(installation_scope(), NotificationId(id), at)
            .await
    }

    /// The counters a diagnostics screen shows.
    ///
    /// `rescan_required` is read from the database handle the runtime actually opened, so it
    /// cannot disagree with the file in use (ARCHITECTURE §15).
    pub async fn health(&self) -> Health {
        Health {
            startup: self.startup,
            scheduler: self.scheduler.metrics().await,
            events: self.bus.stats(),
            rescan_required: self.database.recovery().requires_rescan(),
            // From the cache the refresh task fills, never from the keyring: reading one is blocking
            // and can raise an OS prompt, which is what the command budget test exists to catch.
            providers: self.providers.status(),
        }
    }

    /// Which providers hold a credential as of the last refresh.
    ///
    /// The cached answer, so a settings screen or a diagnostics poll never waits on a keyring: a
    /// read is blocking and can raise an OS prompt, and the refresh that fills this runs off the
    /// command path. Use [`Runtime::start_provider_credential`] to change it.
    #[must_use]
    pub fn provider_status(&self) -> crate::providers::ProviderStatus {
        self.providers.status()
    }

    /// The secret store this runtime reads and writes.
    ///
    /// Exposed rather than copied so a feature that owns its own credential shape — an indexer's base
    /// URL and key — reads the same store a provider key does, without this module learning the shape.
    #[must_use]
    pub fn secret_store(&self) -> Arc<dyn SecretStore> {
        Arc::clone(&self.secrets)
    }

    /// The state the tray's menu presents (PG-E1-09).
    pub async fn shell_status(&self) -> ShellStatus {
        let scheduler = self.scheduler.metrics().await;
        ShellStatus {
            active_downloads: u32::try_from(scheduler.live).unwrap_or(u32::MAX),
            // The port reads stats per handle and the runtime keeps no handle-to-job index yet, so
            // "how many are seeding" has no cheap answer; a non-zero value is PG-E10-09's, whose
            // seeding policy is what decides when a finished torrent stops.
            seeding: 0,
            tray_enabled: self.config.tray_enabled,
        }
    }

    /// The last `lines` lines of the newest log file.
    ///
    /// Reading is bounded twice — a 64 KiB window from the end of the file and at most
    /// [`MAX_LOG_LINES`] lines — because this answer travels over IPC, where a payload
    /// over 64 KB is a design error (ARCHITECTURE §7 rule 1). When the window starts
    /// mid-line the partial first line is dropped rather than returned as a truncated
    /// JSON object.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Logs`] when the directory or the newest file cannot be read. No
    /// log file at all is not an error: a process that has not logged yet returns an
    /// empty tail.
    pub async fn log_tail(&self, lines: u32) -> Result<Vec<String>, RuntimeError> {
        let directory = self.config.logs_directory();
        let Some(path) = newest_log_file(&directory).await else {
            return Ok(Vec::new());
        };

        let mut file = tokio::fs::File::open(&path)
            .await
            .map_err(|source| RuntimeError::Logs {
                path: path.clone(),
                source,
            })?;
        let length = file
            .metadata()
            .await
            .map_err(|source| RuntimeError::Logs {
                path: path.clone(),
                source,
            })?
            .len();
        let window_start = length.saturating_sub(LOG_TAIL_BYTES);
        file.seek(SeekFrom::Start(window_start))
            .await
            .map_err(|source| RuntimeError::Logs {
                path: path.clone(),
                source,
            })?;

        let mut buffer = Vec::new();
        file.take(LOG_TAIL_BYTES)
            .read_to_end(&mut buffer)
            .await
            .map_err(|source| RuntimeError::Logs {
                path: path.clone(),
                source,
            })?;

        let text = String::from_utf8_lossy(&buffer);
        let mut tail: Vec<String> = text.lines().map(str::to_owned).collect();
        if window_start > 0 && !tail.is_empty() {
            tail.remove(0);
        }
        let wanted = lines.clamp(1, MAX_LOG_LINES) as usize;
        if tail.len() > wanted {
            tail.drain(..tail.len() - wanted);
        }
        Ok(tail)
    }

    /// Stops the scheduler and the bus, and closes the journal rows of live jobs.
    ///
    /// The scheduler deliberately does not wait for a job that ignores its token, so the
    /// runtime closes their rows itself: a process that exits with a row still `running`
    /// would show a job that can never finish. The job's own report is then refused by
    /// the same terminal-state guard that refuses a late progress report.
    ///
    /// Idempotent.
    pub async fn shutdown(&self) {
        info!("the runtime is stopping");
        self.external_player.stop();
        self.prowlarr.stop();
        self.scheduler.shutdown().await;
        // Before the bus: a last tick published into a stopped bus is a delivery nobody hears.
        if let Some(service) = &self.acquisition {
            service.stop_pump().await;
        }
        self.bus.shutdown().await;

        let live: Vec<(Ulid, i64)> = {
            let mut guard = self.live();
            guard
                .drain()
                .map(|(job, entry)| (job, entry.journal_id))
                .collect()
        };
        let now = self.clock.now().as_millisecond();
        for (job, journal_id) in &live {
            match self
                .database
                .update_job(
                    *journal_id,
                    JobChange::Finished {
                        at: now,
                        phase: JobPhase::Canceled,
                        error_code: Some(CANCELLED_CODE.to_owned()),
                        error: None,
                    },
                )
                .await
            {
                Ok(true) => info!(job = %job, journal_id, "closed a live job's journal row"),
                Ok(false) => {}
                Err(error) => {
                    warn!(job = %job, journal_id, %error, "a live job's row could not be closed");
                }
            }
        }
        info!(closed = live.len(), "the runtime stopped");
    }

    /// The live jobs, with terminal entries pruned.
    ///
    /// Returns the guard rather than a copy because a caller needs `&mut` to cancel; the
    /// lock is held only for synchronous work, never across an `.await`.
    fn live(&self) -> MutexGuard<'_, HashMap<Ulid, LiveJob>> {
        let mut live = match self.live.lock() {
            Ok(live) => live,
            Err(poisoned) => poisoned.into_inner(),
        };
        // A job handle that reached a terminal state is dead weight: dropping it does not
        // cancel anything (ARCHITECTURE §7 rule 6), and keeping it would grow the map for
        // the life of the process.
        live.retain(|_, entry| !entry.handle.state().is_terminal());
        live
    }

    /// Tracks an accepted job so it can be cancelled later.
    fn remember(&self, handle: JobHandle, journal_id: i64) {
        let mut live = self.live();
        live.insert(handle.id(), LiveJob { handle, journal_id });
    }
}

/// The newest rotated log file, or `None` when none exists.
///
/// The file name carries a `YYYY-MM-DD` date, so the greatest name is the newest file and
/// no metadata read is needed to choose it.
async fn newest_log_file(directory: &Path) -> Option<PathBuf> {
    // A missing directory means nothing has been logged yet, which is not a failure to
    // report to a user who is only asking what the log says.
    let Ok(mut entries) = tokio::fs::read_dir(directory).await else {
        return None;
    };

    let mut newest: Option<PathBuf> = None;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(LOG_FILE_PREFIX) || !name.ends_with(LOG_FILE_SUFFIX) {
            continue;
        }
        let candidate = entry.path();
        if newest.as_ref().is_none_or(|current| {
            current
                .file_name()
                .and_then(|name| name.to_str())
                .is_none_or(|current| name > current)
        }) {
            newest = Some(candidate);
        }
    }
    newest
}

/// How many configured roots are checked when deciding whether a media path may be served.
///
/// A bound rather than an iteration to exhaustion because the read pages: a caller that asked for "all
/// of them" would be a caller that could be made to hold an unbounded list. Two hundred roots is far
/// past any plausible library, and a path that is inside a root past this one is *refused* rather than
/// served — which is the direction that cannot leak (SECURITY.md's path-safety row).
const MAX_MEDIA_ROOTS: u32 = 200;

/// Why the runtime could not start, or could not do what it was asked.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// The torrent session did not start, so acquisition and streaming are unavailable.
    #[error("the torrent engine is not running")]
    TorrentEngineUnavailable,

    /// The data directory could not be created.
    #[error("cannot create the data directory {}: {source}", path.display())]
    DataDirectory {
        /// The directory that failed.
        path: PathBuf,
        /// The filesystem's error.
        source: std::io::Error,
    },

    /// The database could not be opened, migrated, or written.
    #[error("the database could not be used: {0}")]
    Storage(#[from] StorageError),

    /// The search text was empty, which the command refuses before it reaches here.
    #[error("a search needs something to look for")]
    EmptyQuery,

    /// A metadata provider failed.
    #[error("the provider could not answer: {0}")]
    Provider(#[from] peregrine_providers::error::ProviderError),

    /// The sync between a provider and the catalogue failed.
    #[error("the provider's answer could not be written: {0}")]
    Sync(#[from] peregrine_sync::SyncError),

    /// The scheduler is not accepting work.
    #[error("the scheduler could not accept the job: {0}")]
    Scheduler(#[from] SchedulerError),

    /// The caller asked for something outside the documented range.
    #[error("{detail}")]
    InvalidRequest {
        /// What the caller got wrong, phrased for a user.
        detail: String,
    },

    /// A scan was asked for while one was already running.
    #[error("a library scan is already running")]
    ScanAlreadyRunning {
        /// The job that is already scanning, so a caller can watch that one instead.
        job: Ulid,
    },

    /// Indexing refused the operation, with the reason a caller can map.
    #[error(transparent)]
    Index(#[from] crate::library::IndexError),

    /// The library refused the operation, with the reason a user can read.
    #[error(transparent)]
    Library(#[from] crate::library::LibraryError),

    /// The title a command named is not here, or this account may not see it.
    ///
    /// One variant for both, because they are one answer to a caller (I37): a title outside the scope
    /// must be indistinguishable from one that does not exist.
    #[error("that title is not in your library")]
    UnknownTitle,

    /// The library holds no copy of this title that is on disk.
    #[error("the library holds no copy of that title that is on disk")]
    NoPlayableFile,

    /// A media file is outside every folder Peregrine is allowed to scan.
    #[error("that file is outside every folder Peregrine is allowed to scan")]
    FileOutsideRoots,

    /// The file a playback session would serve is no longer where the last scan saw it.
    ///
    /// Its own variant rather than a wrapped stream error, because it is the one refusal a person can
    /// act on differently from the others — the drive is unplugged, the file was moved — and a command
    /// surface that had to reach into `peregrine-stream`'s error type to say so would be an edge taken
    /// for a sentence.
    #[error("that file is no longer where the last scan saw it")]
    MediaMissing,

    /// The path a playback session would serve is not a file.
    #[error("that is not a file Peregrine can play")]
    MediaNotAFile,

    /// The file could not be served to the player, with the reason a user can read.
    #[error(transparent)]
    Playback(#[from] peregrine_playback::PlaybackError),

    /// The external player could not be started.
    #[error(transparent)]
    Player(#[from] peregrine_domain::PlayerError),

    /// No live job has that id.
    #[error("no job {job} is running; it may have finished already")]
    UnknownJob {
        /// The id the caller asked about.
        job: Ulid,
    },

    /// The log files could not be read.
    #[error("cannot read the logs at {}: {source}", path.display())]
    Logs {
        /// The file or directory that failed.
        path: PathBuf,
        /// The filesystem's error.
        source: std::io::Error,
    },

    /// No configured provider can serve the request this read makes.
    ///
    /// A setting rather than a failure, and its own variant so the screen can say so: the answer to
    /// it is "add a key in Settings", which is a different sentence from "the provider did not
    /// answer". The rails read states the same fact as [`RailsState::NoProvider`], because a rail
    /// list can be drawn empty while a browse page cannot.
    #[error("no metadata provider that can serve this read is configured")]
    NoProvider,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{CountingSecretStore, RecordingSink};
    use peregrine_secrets::{SecretKey, SecretStore};
    use peregrine_testkit::MemorySecretStore;
    use std::fmt::Write as _;
    use tempfile::TempDir;

    /// Starts a runtime against a temporary directory, with a recording sink.
    ///
    /// The subscriber is built but never installed process-wide: `set_global_default` can
    /// only be called once per process, so a test that installed it would make every other
    /// test in this binary order-dependent. Dropping it here is deliberate — the runtime
    /// holds the handle it needs, and nothing in these tests changes the log level.
    ///
    /// **The artwork directory is deliberately not `<data directory>/artwork`**, which is where the
    /// runtime used to derive it from: a test that supplied the old derivation's own answer could not
    /// tell the fix from the defect it fixed.
    ///
    /// The credential store is in memory for the same class of reason: `RuntimeConfig::new` defaults
    /// to the OS keyring, and a test that let it would read the developer's own login keychain and,
    /// on macOS, raise an OS prompt (AGENTS R14).
    async fn runtime(directory: &TempDir) -> (Runtime, Arc<RecordingSink>) {
        runtime_with(directory, Arc::new(MemorySecretStore::new())).await
    }

    /// Starts a runtime whose credential store is the one the caller brought.
    async fn runtime_with(
        directory: &TempDir,
        secrets: Arc<dyn SecretStore>,
    ) -> (Runtime, Arc<RecordingSink>) {
        let sink = Arc::new(RecordingSink::default());
        let config = RuntimeConfig::new(
            directory.path(),
            directory.path().join("cache/images"),
            Arc::clone(&sink) as Arc<dyn EventSink>,
        )
        .with_secrets(secrets);
        let (subscriber, logging) = Observability::build(&config.log_config()).unwrap();
        drop(subscriber);
        let runtime = Runtime::start(config, logging).await.unwrap();
        (runtime, sink)
    }

    /// An external player that records what it was asked to open, so the hand-off can be asserted
    /// without a window. It reports a fixed position, which is what makes the progress write
    /// observable.
    #[derive(Default)]
    struct RecordingPlayer {
        opened: Mutex<Vec<PathBuf>>,
        start: Mutex<Option<i64>>,
    }

    impl RecordingPlayer {
        fn opened(&self) -> Vec<PathBuf> {
            self.opened
                .lock()
                .expect("the player log is not poisoned")
                .clone()
        }

        fn start(&self) -> Option<i64> {
            *self.start.lock().expect("the player start is not poisoned")
        }
    }

    impl peregrine_domain::PlayerBackend for RecordingPlayer {
        fn play(
            &self,
            file: &SafePath,
            start_ms: Option<i64>,
            positions: Arc<dyn peregrine_domain::PositionSink>,
        ) -> Result<(), peregrine_domain::PlayerError> {
            self.opened
                .lock()
                .expect("the player log is not poisoned")
                .push(file.as_path().to_path_buf());
            *self.start.lock().expect("the player start is not poisoned") = start_ms;
            // The position a real mpv would report over its socket, so the write path runs.
            positions.position(84_000, Some(7_200_000));
            Ok(())
        }

        fn stop(&self) {}
    }

    /// Starts a runtime whose external player is the one the caller brought.
    async fn runtime_with_player(
        directory: &TempDir,
        player: Arc<RecordingPlayer>,
    ) -> (Runtime, Arc<RecordingSink>) {
        let sink = Arc::new(RecordingSink::default());
        let config = RuntimeConfig::new(
            directory.path(),
            directory.path().join("cache/images"),
            Arc::clone(&sink) as Arc<dyn EventSink>,
        )
        .with_secrets(Arc::new(MemorySecretStore::new()))
        .with_player(Arc::clone(&player) as Arc<dyn peregrine_domain::PlayerBackend>);
        let (subscriber, logging) = Observability::build(&config.log_config()).unwrap();
        drop(subscriber);
        let runtime = Runtime::start(config, logging).await.unwrap();
        (runtime, sink)
    }

    /// Waits until the startup refresh has finished reading the store.
    ///
    /// The refresh runs off the start-up path, so a test that asserted "nothing read the store"
    /// without this would be racing it rather than measuring the poll. What is waited on is the read
    /// finishing rather than the known list — which is the build's manifest and in place from
    /// construction — and the read may end in a refusal, which is an answer and not a reason to wait.
    async fn await_cached_status(runtime: &Runtime) -> crate::providers::ProviderStatus {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let status = runtime.provider_status();
                if status.store != crate::providers::CredentialStore::Unread {
                    return status;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the startup refresh must finish, whatever it answers")
    }

    /// Waits for the journal row to reach a terminal phase.
    async fn await_terminal(runtime: &Runtime, journal_id: i64) -> JobRecord {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let row = runtime.jobs(None, 20).await.unwrap();
                let row = row
                    .into_iter()
                    .find(|row| row.id == journal_id)
                    .expect("the row exists");
                if row.state.is_terminal() {
                    return row;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the job must reach a terminal state")
    }

    /// The journal rows the Discover fetch has written, whatever phase they are in.
    ///
    /// What "queues one fetch" is asserted against: the row is the thing a duplicate would add, and
    /// its phase is not what says so.
    async fn rails_rows(runtime: &Runtime) -> Vec<i64> {
        runtime
            .jobs(None, peregrine_storage::MAX_PAGE_ROWS)
            .await
            .unwrap()
            .into_iter()
            .filter(|row| row.kind == crate::jobs::discover_rails::KIND)
            .map(|row| row.id)
            .collect()
    }

    /// The state the read answered, which is one fact about it and therefore the same on every rail.
    ///
    /// Asserted rather than picked, because the repetition is only honest while the values agree:
    /// two rails disagreeing would be a screen whose copy depended on which one it read first.
    fn rails_state(rails: &[HomeRail]) -> RailsState {
        let first = rails
            .first()
            .expect("the core answers every rail, including the ones nobody has fetched")
            .state;
        assert!(
            rails.iter().all(|rail| rail.state == first),
            "one read answers one state"
        );
        first
    }

    /// Starts a runtime whose injected store holds a provider key and whose adapter is `adapter`.
    ///
    /// **Waited for, and that is the safety of it.** The registry is refreshed off the start-up path,
    /// so a swap that landed first would be overwritten by the real adapter the store builds — and
    /// the adapter the store builds is TMDB, whose rail answer is a request (`AGENTS R14`). Once the
    /// refresh has published its status, the swap is the last write to the adapters and nothing this
    /// test does leaves the machine.
    async fn runtime_with_a_provider(
        directory: &TempDir,
        adapter: Arc<dyn peregrine_providers::provider::MetadataProvider>,
    ) -> (Runtime, Arc<RecordingSink>) {
        let secrets = Arc::new(MemorySecretStore::with_provider("tmdb", "a-key"));
        let (runtime, sink) =
            runtime_with(directory, Arc::clone(&secrets) as Arc<dyn SecretStore>).await;
        let status = await_cached_status(&runtime).await;
        assert_eq!(
            status.configured,
            vec!["tmdb"],
            "the injected store is what says a provider is configured"
        );
        // `Runtime::start` refreshes the registry on a blocking thread. Replacing the adapters
        // before that lands lets the refresh overwrite them with real ones, which turns a test
        // into a live provider call; wait for the flag rather than racing it.
        let mut spins = 0_u32;
        while !runtime.providers.refreshed() {
            std::thread::yield_now();
            spins += 1;
            assert!(spins < 10_000_000, "the provider registry never refreshed");
        }
        runtime.providers.use_adapters(vec![adapter]);
        (runtime, sink)
    }

    /// One rail candidate, which is what a provider lists in a rail.
    fn candidate(id: &str, name: &str) -> peregrine_providers::provider::SearchCandidate {
        peregrine_providers::provider::SearchCandidate::new(
            peregrine_domain::id::ExternalId::new(peregrine_domain::id::Provider::Tmdb, id)
                .expect("a fixture id"),
            peregrine_domain::text::NonEmptyText::new(name).expect("the fixture passes real text"),
            peregrine_domain::catalogue::TitleKind::Movie,
            None,
        )
    }

    /// A provider whose rail answer waits for the test, so a fetch a read started is still in the air
    /// when the next read asks.
    ///
    /// Idempotence is a statement about a fetch *in flight*, and a scripted provider that answered
    /// instantly would have finished before the second read — leaving the test to race the scheduler
    /// instead of measuring it. The gate is a `watch` the test closes when it is done, so nothing
    /// here waits on a clock or a sleep (`AGENTS R14`); everything but `rail` is the testkit fake,
    /// which is the same adapter the rails job's own tests drive.
    /// The gate a [`GatedSecretStore`]'s read waits on: the flag that releases it, and the condvar
    /// that wakes the waiting read.
    type ReadGate = Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>;

    /// A credential store whose read blocks until the test releases it.
    ///
    /// What a launch looks like on a machine whose keychain raises a prompt: the read runs on a
    /// blocking thread, and the screen's first read arrives before it is answered. The gate is a
    /// `Condvar` rather than a poll, so the fixture decides when the read lands instead of a sleep
    /// deciding it (`AGENTS R14`).
    struct GatedSecretStore {
        /// Told once, when the read starts, so the test knows it is in the air.
        entered: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        /// Set to true, and notified, when the read may finish.
        gate: ReadGate,
    }

    impl GatedSecretStore {
        fn holding_the_read() -> (Arc<Self>, tokio::sync::oneshot::Receiver<()>, ReadGate) {
            let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
            let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
            (
                Arc::new(Self {
                    entered: std::sync::Mutex::new(Some(entered_tx)),
                    gate: Arc::clone(&gate),
                }),
                entered_rx,
                gate,
            )
        }
    }

    impl SecretStore for GatedSecretStore {
        fn get(
            &self,
            _key: &peregrine_secrets::SecretKey,
        ) -> Result<Option<peregrine_secrets::Secret>, peregrine_secrets::SecretStoreError>
        {
            if let Some(entered) = self
                .entered
                .lock()
                .expect("the lock is not poisoned")
                .take()
            {
                let _ = entered.send(());
            }
            // Blocking, which is what this port is and why the runtime runs it on a blocking thread.
            let (lock, condvar) = &*self.gate;
            let mut released = lock.lock().expect("the lock is not poisoned");
            while !*released {
                released = condvar.wait(released).expect("the lock is not poisoned");
            }
            // Nothing is configured, so the registry this read fills holds no adapter: the test
            // asserts what the *read path* decides without a request leaving the machine (`R14`).
            Ok(None)
        }

        fn set(
            &self,
            _key: &peregrine_secrets::SecretKey,
            _secret: &peregrine_secrets::Secret,
        ) -> Result<(), peregrine_secrets::SecretStoreError> {
            Ok(())
        }

        fn delete(
            &self,
            _key: &peregrine_secrets::SecretKey,
        ) -> Result<(), peregrine_secrets::SecretStoreError> {
            Ok(())
        }
    }

    struct GatedRailProvider {
        inner: peregrine_testkit::FakeMetadataProvider,
        released: tokio::sync::watch::Receiver<bool>,
    }

    impl GatedRailProvider {
        /// A provider that publishes the rails and holds the first answer until the sender says so.
        fn holding_the_rails() -> (Self, tokio::sync::watch::Sender<bool>) {
            let (sender, released) = tokio::sync::watch::channel(false);
            (
                Self {
                    inner: peregrine_testkit::FakeMetadataProvider::publishing_rails(
                        peregrine_domain::id::Provider::Tmdb,
                        Vec::new(),
                    ),
                    released,
                },
                sender,
            )
        }
    }

    impl peregrine_providers::provider::MetadataProvider for GatedRailProvider {
        fn descriptor(&self) -> &peregrine_providers::ProviderDescriptor {
            self.inner.descriptor()
        }

        fn check(&self) -> peregrine_providers::ProviderFuture<'_, ()> {
            self.inner.check()
        }

        fn search(
            &self,
            query: &peregrine_providers::provider::SearchQuery,
        ) -> peregrine_providers::ProviderFuture<
            '_,
            Vec<peregrine_providers::provider::SearchCandidate>,
        > {
            self.inner.search(query)
        }

        fn title(
            &self,
            id: &peregrine_domain::id::ExternalId,
            kind: peregrine_domain::catalogue::TitleKind,
        ) -> peregrine_providers::ProviderFuture<'_, peregrine_domain::catalogue::TitleDetails>
        {
            self.inner.title(id, kind)
        }

        /// Delegated, so a season read in a test sees whatever the fake behind this wrapper answers.
        fn seasons(
            &self,
            id: &peregrine_domain::id::ExternalId,
            kind: peregrine_domain::catalogue::TitleKind,
        ) -> peregrine_providers::ProviderFuture<'_, Vec<peregrine_domain::catalogue::SeasonDetails>>
        {
            self.inner.seasons(id, kind)
        }

        /// Delegated, for the reason [`Self::seasons`] gives.
        fn season_episodes(
            &self,
            id: &peregrine_domain::id::ExternalId,
            kind: peregrine_domain::catalogue::TitleKind,
            season_number: peregrine_domain::catalogue::SeasonNumber,
        ) -> peregrine_providers::ProviderFuture<'_, Vec<peregrine_domain::catalogue::EpisodeDetails>>
        {
            self.inner.season_episodes(id, kind, season_number)
        }

        fn image(
            &self,
            path: &str,
            kind: peregrine_domain::catalogue::ImageKind,
        ) -> peregrine_providers::ProviderFuture<'_, Vec<u8>> {
            self.inner.image(path, kind)
        }

        fn rail(
            &self,
            _rail: peregrine_providers::provider::RailKind,
            _page: std::num::NonZeroU32,
        ) -> peregrine_providers::ProviderFuture<
            '_,
            Vec<peregrine_providers::provider::SearchCandidate>,
        > {
            let mut released = self.released.clone();
            Box::pin(async move {
                // A sender that is gone is a finished test, not a locked gate.
                while !*released.borrow_and_update() && released.changed().await.is_ok() {}
                Ok(Vec::new())
            })
        }

        /// Delegated, so a browse in a test sees whatever the fake behind this gate answers.
        fn browse(
            &self,
            request: &peregrine_providers::provider::BrowseRequest,
        ) -> peregrine_providers::ProviderFuture<'_, peregrine_providers::ProviderPage> {
            self.inner.browse(request)
        }

        /// Delegated, for the reason [`Self::browse`] gives.
        fn genres(
            &self,
            kind: peregrine_domain::catalogue::TitleKind,
        ) -> peregrine_providers::ProviderFuture<'_, Vec<peregrine_providers::GenreRef>> {
            self.inner.genres(kind)
        }

        /// Delegated, for the reason [`Self::browse`] gives.
        fn similar(
            &self,
            id: &peregrine_domain::id::ExternalId,
            kind: peregrine_domain::catalogue::TitleKind,
        ) -> peregrine_providers::ProviderFuture<
            '_,
            Vec<peregrine_providers::provider::SearchCandidate>,
        > {
            self.inner.similar(id, kind)
        }
    }

    /// What a provider answered for one title, with only the two fields a test reads set.
    fn details(name: &str, poster: Option<&str>) -> peregrine_domain::catalogue::TitleDetails {
        peregrine_domain::catalogue::TitleDetails {
            kind: peregrine_domain::catalogue::TitleKind::Movie,
            name: peregrine_domain::NonEmptyText::new(name).expect("the fixture is not empty"),
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
            poster_path: poster.map(str::to_owned),
            backdrop_path: None,
        }
    }

    /// The read a title screen makes answers where the poster is, and whether to ask a provider.
    ///
    /// **The wiring, not the rule.** `crate::artwork` tests the rule and the cache lookup on their
    /// own; what this asserts is that the read feeds them the columns the row actually holds. Three
    /// arguments taken from the wrong column is a bug no unit test of the rule can see, and it is
    /// the mistake a screen would show as a poster that never arrives.
    #[tokio::test]
    async fn a_title_read_reports_the_cached_poster_from_the_configured_cache_directory() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;
        let id = runtime
            .database
            .insert_stub_title(peregrine_storage::StubTitle {
                kind: peregrine_domain::catalogue::TitleKind::Movie,
                subtype: peregrine_domain::catalogue::TitleSubtype::Standard,
                name: "Alien".to_owned(),
                original_name: None,
                sort_name: "alien".to_owned(),
                at: 0,
            })
            .await
            .unwrap();

        // Nothing has filled it in. What `refresh_due` answers is asserted where it can be asserted
        // deterministically — `crate::artwork`'s tests for the rule, and `ArtworkState::of` for the
        // wiring — because its first input is whether the OS credential store holds a provider key,
        // which is a fact about the machine running the tests (AGENTS R14). The poster path is what
        // this test owns.
        let fresh = runtime.library_title(id).await.unwrap().expect("the stub");
        assert_eq!(fresh.entry.poster, None);

        // A provider that answered with no artwork at all: the title *has* been filled, so asking
        // again could only be told the same thing.
        runtime
            .database
            .update_title_metadata(
                id,
                details("Alien", None),
                "alien".to_owned(),
                peregrine_storage::MetadataState::Partial,
                "tmdb".to_owned(),
                1,
            )
            .await
            .unwrap();
        let no_artwork = runtime.library_title(id).await.unwrap().expect("the title");
        assert_eq!(no_artwork.entry.poster, None);

        // And a fill that recorded a poster the cache does not hold: one request puts it there.
        runtime
            .database
            .update_title_metadata(
                id,
                details("Alien", Some("/poster.jpg")),
                "alien".to_owned(),
                peregrine_storage::MetadataState::Partial,
                "tmdb".to_owned(),
                1,
            )
            .await
            .unwrap();
        let known = runtime.library_title(id).await.unwrap().expect("the title");
        assert_eq!(known.entry.poster, None, "the bytes have not been fetched");

        // Once the blob is in the cache the screen has nothing to ask for — and the path it answers
        // with is inside the directory the runtime was told to write into, which is the one the
        // asset protocol was scoped to. That is the whole point of the directory being a parameter.
        let key = peregrine_sync::ImageKey::for_image(
            peregrine_domain::Provider::Tmdb,
            "/poster.jpg",
            peregrine_sync::ImageVariant::PosterW342,
        );
        let stored = peregrine_sync::ImageCache::new(runtime.config().artwork_directory())
            .store(&key, b"pretend this is a jpeg")
            .expect("the artwork directory is writable");
        let cached = runtime.library_title(id).await.unwrap().expect("the title");
        assert_eq!(cached.entry.poster.as_deref(), Some(stored.disk_path()));
        assert!(
            cached
                .entry
                .poster
                .as_deref()
                .expect("a path")
                .starts_with(runtime.config().artwork_directory()),
            "the path must be one the asset protocol can serve"
        );
        assert_ne!(
            runtime.config().artwork_directory(),
            runtime.config().database_path().with_file_name("artwork"),
            "the fixture must not supply the answer the old derivation would have produced, or this \
             test cannot tell the fix from the defect"
        );

        // A title that is not there is still `None` rather than an error (I37).
        assert_eq!(runtime.library_title(9_999).await.unwrap(), None);
    }

    /// The acceptance for PG-E0-19 in one test: the runtime opens the database, accepts a
    /// job, the job's state is readable from the journal, progress arrived over the bus,
    /// and shutdown leaves nothing running.
    #[tokio::test]
    async fn a_hello_job_runs_end_to_end_and_is_journaled() {
        let directory = TempDir::new().unwrap();
        let (runtime, sink) = runtime(&directory).await;

        let started = runtime.start_hello(4).await.unwrap();
        assert!(runtime.config().database_path().exists());

        let row = await_terminal(&runtime, started.journal_id).await;
        assert_eq!(row.state, JobPhase::Succeeded);
        assert_eq!(row.kind, hello::KIND);
        assert_eq!(row.progress_permille, 1000);
        assert_eq!(row.attempts, 1);
        assert!(
            row.started_at.is_some() && row.finished_at.is_some(),
            "both timestamps are recorded: {row:?}"
        );

        let health = runtime.health().await;
        assert!(health.startup < Duration::from_secs(30));
        assert_eq!(
            health.scheduler.live, 0,
            "a finished job must not stay in the scheduler's bookkeeping"
        );
        assert_eq!(health.events.dropped, 0, "one job cannot exhaust the bus");
        assert!(!health.events.is_degraded());

        runtime.shutdown().await;
        let delivered = sink.delivered();
        assert!(
            delivered
                .iter()
                .any(|event| event.topic.name() == "job://progress"),
            "progress must reach the sink: {delivered:#?}"
        );
        assert!(
            delivered
                .iter()
                .any(|event| event.payload["phase"] == "succeeded"),
            "the terminal phase must reach the sink: {delivered:#?}"
        );

        // Idempotent: a second shutdown must not hang or double-close anything.
        runtime.shutdown().await;
    }

    /// A row that has reached a terminal phase means the job is gone, whatever the handle says.
    ///
    /// A job writes its own terminal row and the scheduler resolves the handle afterwards, so
    /// there is a window in which the journal is honest and the handle is not yet: a cancel
    /// arriving there used to answer `ok` about a job that had already stopped, which is the one
    /// answer a caller cannot tell apart from a real cancellation. This test stages that window by
    /// writing the row by hand, where racing a real job staged it about one run in ten.
    #[tokio::test]
    async fn cancelling_a_job_whose_row_is_already_terminal_reports_it_unknown() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;
        let started = runtime.start_hello(hello::MAX_STEPS).await.unwrap();

        // What the job itself writes as its last act, before the scheduler sees its outcome.
        let closed = runtime
            .database
            .update_job(
                started.journal_id,
                JobChange::Finished {
                    at: runtime.clock.now().as_millisecond(),
                    phase: JobPhase::Canceled,
                    error_code: Some(CANCELLED_CODE.to_owned()),
                    error: None,
                },
            )
            .await
            .unwrap();
        assert!(closed, "the row was open");

        let error = runtime
            .cancel(started.job)
            .await
            .expect_err("a job whose row is terminal is not live");
        assert!(
            matches!(error, RuntimeError::UnknownJob { .. }),
            "{error:?}"
        );

        runtime.shutdown().await;
    }

    /// A scan of an empty library runs, journals, and reaches a terminal state.
    ///
    /// The walk's own behaviour is the job's to test; what this asserts is the runtime's half —
    /// a row exists before the job runs, the job is remembered, and the row closes.
    #[tokio::test]
    async fn a_scan_can_be_started_and_its_row_closes() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;

        let started = runtime.start_library_scan().await.unwrap();
        let row = await_terminal(&runtime, started.journal_id).await;

        assert_eq!(row.kind, library_scan::KIND);
        assert_eq!(row.state, JobPhase::Succeeded, "{row:?}");
        assert!(
            !runtime.live().contains_key(&started.job),
            "and it is no longer live"
        );

        runtime.shutdown().await;
    }

    /// A second scan while one is live is refused by name rather than queued or dropped.
    ///
    /// The slot is held the way the runtime holds it — a scheduler job of the scan's kind,
    /// remembered as live — because a scan of a temporary library finishes in microseconds and a
    /// test that raced two real ones would be a test of the machine's speed. What is under test is
    /// the guard, and the guard reads the live set.
    #[tokio::test]
    async fn a_second_scan_while_one_is_live_is_refused_by_name() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;

        let journal_id = runtime
            .database
            .insert_job(NewJob {
                kind: library_scan::KIND.to_owned(),
                priority: library_scan::PRIORITY,
                dedupe_key: None,
                payload_json: "{}".to_owned(),
                max_attempts: 1,
                created_at: runtime.clock.now().as_millisecond(),
            })
            .await
            .unwrap();
        let held = runtime
            .scheduler
            .submit(
                JobSpec {
                    kind: library_scan::KIND.to_owned(),
                    priority: library_scan::PRIORITY,
                    dedupe_key: None,
                    max_attempts: 1,
                },
                HelloJob::new(
                    Arc::clone(&runtime.database),
                    Arc::clone(&runtime.bus),
                    Arc::clone(&runtime.clock),
                    journal_id,
                    hello::MAX_STEPS,
                ),
            )
            .await
            .unwrap();
        let held_id = held.id();
        runtime.remember(held, journal_id);

        let refused = runtime
            .start_library_scan()
            .await
            .expect_err("one scan at a time");

        match refused {
            RuntimeError::ScanAlreadyRunning { job } => assert_eq!(job, held_id),
            other => panic!("expected a refusal naming the running scan, got {other:?}"),
        }

        runtime.shutdown().await;
    }

    /// A cancellation is honored and journaled, and the row ends terminal rather than
    /// running forever.
    #[tokio::test]
    async fn cancelling_a_hello_job_stops_it_and_closes_its_row() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;

        let started = runtime.start_hello(hello::MAX_STEPS).await.unwrap();
        runtime.cancel(started.job).await.unwrap();

        let row = await_terminal(&runtime, started.journal_id).await;
        assert_eq!(row.state, JobPhase::Canceled);
        assert_eq!(row.last_error_code.as_deref(), Some(CANCELLED_CODE));
        assert!(row.cancel_requested, "the request is part of the audit");

        // The handle is gone once the job is terminal, so asking again is honest rather
        // than a silent no-op.
        let error = runtime
            .cancel(started.job)
            .await
            .expect_err("a finished job cannot be cancelled again");
        assert!(
            matches!(error, RuntimeError::UnknownJob { .. }),
            "{error:?}"
        );

        runtime.shutdown().await;
    }

    /// Shutdown closes the row of a job that is still running, because the scheduler does
    /// not wait for a job that is mid-step.
    #[tokio::test]
    async fn shutdown_closes_a_live_jobs_row() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;

        let started = runtime.start_hello(hello::MAX_STEPS).await.unwrap();
        // One step is enough for the job to have written its `running` row.
        tokio::time::sleep(Duration::from_millis(25)).await;
        runtime.shutdown().await;

        let rows = runtime.jobs(None, 10).await.unwrap();
        let row = rows
            .into_iter()
            .find(|row| row.id == started.journal_id)
            .expect("the row exists");
        assert_eq!(
            row.state,
            JobPhase::Canceled,
            "a stopping runtime must not leave a job running: {row:?}"
        );
    }

    /// The boundary: a step count outside the documented range is refused with the range
    /// in the message, and nothing is journaled.
    #[tokio::test]
    async fn start_hello_refuses_a_step_count_outside_the_bounds() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;

        for steps in [0, hello::MAX_STEPS + 1, u32::MAX] {
            let error = runtime
                .start_hello(steps)
                .await
                .expect_err("the range must be enforced");
            let RuntimeError::InvalidRequest { detail } = &error else {
                panic!("expected an invalid-request failure, got {error:?}");
            };
            assert!(
                detail.contains(&hello::MIN_STEPS.to_string())
                    && detail.contains(&hello::MAX_STEPS.to_string()),
                "the message must name the range: {detail}"
            );
        }
        assert!(
            runtime.jobs(None, 10).await.unwrap().is_empty(),
            "a refused request must not leave a journal row behind"
        );

        // The bounds themselves are accepted.
        let lowest = runtime.start_hello(hello::MIN_STEPS).await.unwrap();
        let highest = runtime.start_hello(hello::MAX_STEPS).await.unwrap();
        assert_ne!(lowest.journal_id, highest.journal_id);
        runtime.cancel(highest.job).await.unwrap();
        runtime.shutdown().await;
    }

    /// The log tail reads the newest file, stays inside the line limit, and does not
    /// return the partial line a byte window starts in the middle of.
    #[tokio::test]
    async fn log_tail_reads_the_newest_file_within_its_window() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;
        let logs = runtime.config().logs_directory();
        tokio::fs::create_dir_all(&logs).await.unwrap();

        // The fixture's dates are far outside any real one, because the choice is made by comparing
        // names as text: a date written in the past is outranked by whatever the running process has
        // logged today, and this test would then read the live file instead of the fixture. It did
        // exactly that on 2026-10-13, which is why these are the extremes of the format's range.
        tokio::fs::write(
            logs.join("peregrine.0001-01-01.jsonl"),
            "{\"message\":\"from-yesterday\"}\n",
        )
        .await
        .unwrap();

        // A newest file larger than the window, so the read starts mid-line.
        let mut newest = String::from("{\"message\":\"partial-line-that-must-be-dropped\"}\n");
        for index in 0..4_000 {
            let _ = writeln!(newest, "{{\"message\":\"line-{index}\"}}");
        }
        assert!(
            newest.len() as u64 > LOG_TAIL_BYTES,
            "the fixture must be larger than the window"
        );
        tokio::fs::write(logs.join("peregrine.9999-12-31.jsonl"), &newest)
            .await
            .unwrap();

        let tail = runtime.log_tail(10).await.unwrap();
        assert_eq!(tail.len(), 10);
        assert_eq!(tail[9], "{\"message\":\"line-3999\"}");
        assert_eq!(tail[0], "{\"message\":\"line-3990\"}");
        assert!(
            !tail.iter().any(|line| line.contains("partial")),
            "a window that starts mid-line must drop the partial record: {tail:?}"
        );
        assert!(
            !tail.iter().any(|line| line.contains("yesterday")),
            "the newest file is the one to read"
        );

        // Asking for more than the cap returns at most the cap.
        assert_eq!(
            runtime.log_tail(u32::MAX).await.unwrap().len(),
            usize::try_from(MAX_LOG_LINES).unwrap()
        );

        runtime.shutdown().await;
    }

    /// A process that has logged nothing yet has an empty tail, not an error.
    #[tokio::test]
    async fn log_tail_of_a_process_that_has_not_logged_is_empty() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;

        assert!(
            runtime.log_tail(10).await.unwrap().is_empty(),
            "no line has been written by this runtime"
        );
        runtime.shutdown().await;
    }

    /// A data directory that cannot exist fails startup with the path in the message.
    #[tokio::test]
    async fn start_reports_a_data_directory_that_cannot_be_created() {
        let directory = TempDir::new().unwrap();
        let blocked = directory.path().join("a-file");
        std::fs::write(&blocked, b"not a directory").unwrap();

        let sink = Arc::new(RecordingSink::default());
        let config = RuntimeConfig::new(
            &blocked,
            directory.path().join("artwork"),
            sink as Arc<dyn EventSink>,
        );
        // The log directory has to be somewhere writable, so it is built from a second
        // configuration; startup must fail on the data directory regardless.
        let logs = RuntimeConfig::new(
            directory.path().join("logs-home"),
            directory.path().join("artwork"),
            Arc::new(RecordingSink::default()) as Arc<dyn EventSink>,
        );
        let (_subscriber, logging) = Observability::build(&logs.log_config()).unwrap();

        let error = Runtime::start(config, logging)
            .await
            .err()
            .expect("a file cannot be a data directory");
        assert!(
            matches!(error, RuntimeError::DataDirectory { .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains("a-file"), "{error}");
    }

    /// An empty journal lists nothing, and health reports a scheduler with no work.
    #[tokio::test]
    async fn a_fresh_runtime_has_no_jobs() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;

        assert!(
            runtime.jobs(None, 10).await.unwrap().is_empty(),
            "a fresh runtime has journaled nothing"
        );
        let health = runtime.health().await;
        assert_eq!(health.scheduler, SchedulerMetrics::default());
        assert_eq!(health.events, EventStats::default());
        assert!(
            !health.rescan_required,
            "a first install has no library to rebuild"
        );
        runtime.shutdown().await;
    }

    /// The Discover read is a lock and a clone, and a cold start answers empty rails rather than
    /// nothing: the screen has its geometry from the first frame (`DESIGN_SYSTEM §11` rule 2) and
    /// pays for it with rails that are empty until the fetch lands. With no provider configured
    /// there is nothing to fetch, and the answer says which of the three states this is.
    #[tokio::test]
    async fn a_fresh_runtime_answers_every_rail_as_empty() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;

        let rails = runtime.home_rails(None).await.unwrap();

        assert_eq!(rails.len(), peregrine_providers::RailKind::ALL.len());
        assert!(rails.iter().all(|rail| rail.items.is_empty()));
        assert_eq!(
            rails_state(&rails),
            RailsState::NoProvider,
            "a key nobody has set up is what keeps these rails empty"
        );
        runtime.shutdown().await;
    }

    /// A live job holding the key is found before a second row is written, which is what makes two
    /// visits to Discover share one fetch.
    ///
    /// **This is the coalescing, and it is a journal read rather than
    /// `Scheduler::submit_if_absent`** — that method's own doc says it answers `Ok(None)` when an
    /// existing job took the submission, and it does not: it maps `enqueue_one`'s `Ok(handle)` to
    /// `Ok(Some(handle))` unconditionally. The engine *does* drop a coalesced submission's work,
    /// but a caller holding the returned handle cannot tell that from a fresh job, and the journal
    /// row it would have written stays `queued` forever. The pre-check is the honest version of the
    /// same intention, and it is what this pins.
    #[tokio::test]
    async fn a_live_rails_job_is_found_by_its_key_rather_than_duplicated() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;
        let key = crate::jobs::discover_rails::dedupe_key();

        assert!(
            runtime.live_job_for(&key).await.unwrap().is_none(),
            "nothing is live before anything is started"
        );

        let journal_id = runtime
            .database
            .insert_job(NewJob {
                kind: crate::jobs::discover_rails::KIND.to_owned(),
                priority: crate::jobs::discover_rails::PRIORITY,
                dedupe_key: Some(key.clone()),
                payload_json: "{}".to_owned(),
                max_attempts: 2,
                created_at: runtime.clock.now().as_millisecond(),
            })
            .await
            .unwrap();

        let live = runtime
            .live_job_for(&key)
            .await
            .unwrap()
            .expect("a queued row holding the key is live");
        assert_eq!(live.journal_id, journal_id);
        assert!(
            live.coalesced,
            "a joined job says so rather than looking new"
        );

        // And the same answer through the method a screen actually calls: the second start joins the
        // first rather than writing a second row, which the journal's partial unique index would
        // refuse. No provider is configured in this runtime, so the row found is the one written
        // above.
        let joined = runtime.start_discover_rails().await.unwrap();
        assert_eq!(joined.journal_id, journal_id);
        assert!(joined.coalesced, "two visits to Discover share one fetch");
        runtime.shutdown().await;
    }

    /// `FLOWS F2`'s cold start: the read answers the shells *and* starts the fetch, so nobody has to
    /// press Refresh — and the second read, which is what a screen does every time it paints, joins
    /// the fetch in flight rather than writing a second row.
    #[tokio::test]
    async fn a_cold_read_with_a_provider_starts_one_fetch_and_the_second_read_joins_it() {
        let directory = TempDir::new().unwrap();
        let (provider, released) = GatedRailProvider::holding_the_rails();
        let (runtime, _sink) = runtime_with_a_provider(&directory, Arc::new(provider)).await;

        let first = runtime.home_rails(None).await.unwrap();
        assert_eq!(rails_state(&first), RailsState::Fetching);
        assert_eq!(first.len(), peregrine_providers::RailKind::ALL.len());
        assert!(
            first.iter().all(|rail| rail.items.is_empty()),
            "the shells are answered before anything has been fetched"
        );
        let queued = rails_rows(&runtime).await;
        assert_eq!(queued.len(), 1, "a cold read queues exactly one fetch");

        let second = runtime.home_rails(None).await.unwrap();
        assert_eq!(
            rails_state(&second),
            RailsState::Fetching,
            "the fetch is still in the air, which is what the screen says"
        );
        assert_eq!(
            rails_rows(&runtime).await,
            queued,
            "a second read joins the fetch in flight rather than starting another"
        );

        // Released before the scheduler is shut down: the job is waiting on a gate this test owns,
        // and a shutdown that joined it would wait for ever.
        let _ = released.send(true);
        runtime.shutdown().await;
    }

    /// The state that has to keep saying "configure a provider key in Settings": with no provider
    /// there is nothing to ask, so a cold read queues nothing and answers `NoProvider`.
    ///
    /// The wait is part of the fixture and not of the rule: the credential read runs off the startup
    /// path (a keychain can raise a prompt), so this test has to let it land before it can claim
    /// anything about what a settled registry answers. A read that arrives first is the case
    /// [`a_cold_read_before_the_credentials_are_read_starts_the_fetch`] pins.
    #[tokio::test]
    async fn a_cold_read_with_no_provider_queues_nothing() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;
        let _ = await_cached_status(&runtime).await;

        let rails = runtime.home_rails(None).await.unwrap();

        assert_eq!(rails_state(&rails), RailsState::NoProvider);
        assert!(
            rails_rows(&runtime).await.is_empty(),
            "there is no provider to fetch from"
        );
        runtime.shutdown().await;
    }

    /// The launch whose keychain is slow — a prompt, a locked login keyring, a first run from a new
    /// binary — where the screen's first read arrives before the credential read has answered.
    ///
    /// `NoProvider` there is two mistakes at once: it tells a user who *has* a key to go and set one
    /// up, and it starts nothing, so the rails stay empty until they press Refresh once the prompt has
    /// been dismissed. The read enqueues the fetch instead, and the job waits for the read that is
    /// still in the air.
    #[tokio::test]
    async fn a_cold_read_before_the_credentials_are_read_starts_the_fetch() {
        let directory = TempDir::new().unwrap();
        let (store, entered, gate) = GatedSecretStore::holding_the_read();
        let (runtime, _sink) = runtime_with(&directory, store as Arc<dyn SecretStore>).await;
        entered
            .await
            .expect("the runtime reads the store at startup");

        let rails = runtime.home_rails(None).await.unwrap();
        assert_eq!(
            rails_state(&rails),
            RailsState::Fetching,
            "a read that has not answered yet is not a provider that is absent"
        );
        let queued = rails_rows(&runtime).await;
        assert_eq!(
            queued.len(),
            1,
            "and the fetch that will use the credential is queued, not refused"
        );

        // The read lands, and answers that nothing is configured: that is the answer the screen was
        // waiting for, and it is the one it shows.
        *gate.0.lock().expect("the gate is not poisoned") = true;
        gate.1.notify_all();
        let _ = await_cached_status(&runtime).await;

        let settled = runtime.home_rails(None).await.unwrap();
        assert_eq!(rails_state(&settled), RailsState::NoProvider);
        assert_eq!(
            rails_rows(&runtime).await,
            queued,
            "a settled registry queues nothing more"
        );
        runtime.shutdown().await;
    }

    /// The relaunch PG-E7-08 exists for: the rails the last run published are drawn from the
    /// catalogue and the cache, and nothing is fetched, because there is nothing to fill.
    #[tokio::test]
    async fn a_launch_hydrates_the_manifest_the_last_run_published() {
        let directory = TempDir::new().unwrap();
        let id = {
            let database = peregrine_storage::Database::open(directory.path().join(DATABASE_FILE))
                .await
                .expect("the database opens");
            let id = database
                .insert_stub_title(peregrine_storage::StubTitle {
                    kind: peregrine_domain::catalogue::TitleKind::Movie,
                    subtype: peregrine_domain::catalogue::TitleSubtype::Standard,
                    name: "The Matrix".to_owned(),
                    original_name: None,
                    sort_name: "the matrix".to_owned(),
                    at: 1_700_000_000_000,
                })
                .await
                .expect("the title is written");
            database
                .replace_discover_rails(&[(
                    peregrine_providers::RailKind::PopularMovies
                        .as_str()
                        .to_owned(),
                    vec![id],
                )])
                .await
                .expect("the manifest is written");
            id
        };

        let (runtime, _sink) = runtime(&directory).await;

        let rails = runtime.home_rails(None).await.unwrap();
        assert_eq!(
            rails_state(&rails),
            RailsState::Ready,
            "the rails are here, so there is nothing to fetch"
        );
        let popular = rails
            .iter()
            .find(|rail| rail.rail == peregrine_providers::RailKind::PopularMovies)
            .expect("every rail is reported");
        assert_eq!(popular.items.len(), 1);
        assert_eq!(popular.items[0].title_id, id);
        assert_eq!(popular.items[0].name, "The Matrix");
        assert!(
            rails_rows(&runtime).await.is_empty(),
            "a launch that has the rails starts no fetch"
        );
        runtime.shutdown().await;
    }

    /// Rails already in hand are not fetched again: the read that finds them answers `Ready` and
    /// queues nothing, which is what makes a warm Discover a read and no work.
    #[tokio::test]
    async fn a_read_with_rails_already_present_queues_nothing() {
        let directory = TempDir::new().unwrap();
        let adapter = Arc::new(peregrine_testkit::FakeMetadataProvider::publishing_rails(
            peregrine_domain::id::Provider::Tmdb,
            vec![candidate("603", "The Matrix")],
        ));
        let (runtime, _sink) = runtime_with_a_provider(&directory, adapter).await;

        let cold = runtime.home_rails(None).await.unwrap();
        assert_eq!(rails_state(&cold), RailsState::Fetching);
        let queued = rails_rows(&runtime).await;
        assert_eq!(queued.len(), 1);
        let row = await_terminal(&runtime, queued[0]).await;
        assert_eq!(
            row.state,
            JobPhase::Succeeded,
            "the fetch lands and publishes: {row:?}"
        );

        let warm = runtime.home_rails(None).await.unwrap();
        assert_eq!(rails_state(&warm), RailsState::Ready);
        assert!(
            warm.iter().any(|rail| !rail.items.is_empty()),
            "the fetch published the rails it was answered"
        );
        assert_eq!(
            rails_rows(&runtime).await,
            queued,
            "a warm read queues nothing"
        );
        runtime.shutdown().await;
    }

    /// PG-E3-13 and PG-E3-14 on the real boot path: a damaged database is detected, kept
    /// aside, rebuilt, and the rescan it requires reaches the health surface the commands
    /// read — after which the runtime is a working one.
    #[tokio::test]
    async fn a_damaged_database_is_rebuilt_at_boot_and_reported_through_health() {
        let directory = TempDir::new().unwrap();
        tokio::fs::write(directory.path().join(DATABASE_FILE), b"not a database")
            .await
            .unwrap();

        let (runtime, _sink) = runtime(&directory).await;

        assert!(
            runtime.health().await.rescan_required,
            "the boot must signal that the library has to be rescanned"
        );
        let quarantine = directory.path().join(format!("{DATABASE_FILE}.corrupt-1"));
        assert!(
            quarantine.is_file(),
            "the damaged file must be kept, not deleted"
        );

        let started = runtime.start_hello(1).await.unwrap();
        let row = await_terminal(&runtime, started.journal_id).await;
        assert_eq!(
            row.state,
            JobPhase::Succeeded,
            "the rebuilt database must be a working one: {row:?}"
        );
        runtime.shutdown().await;
    }

    /// The tray is on by default and the off state is a value, not a branch nobody has
    /// run: PG-E1-09's "hidden when disabled" needs something to be disabled by.
    #[test]
    fn the_tray_is_enabled_unless_the_configuration_says_otherwise() {
        let directory = TempDir::new().unwrap();
        let sink = Arc::new(RecordingSink::default());
        let enabled = RuntimeConfig::new(
            directory.path(),
            directory.path().join("artwork"),
            Arc::clone(&sink) as Arc<dyn EventSink>,
        );
        assert!(enabled.tray_enabled, "a tray nobody asked to hide is shown");
        assert!(!enabled.with_tray_enabled(false).tray_enabled);
    }

    /// The counts the tray presents are the runtime's, read from the scheduler that owns
    /// them; the shell never assembles them from webview events.
    #[tokio::test]
    async fn shell_status_reports_the_schedulers_counters_and_no_seeding() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;

        let status = runtime.shell_status().await;
        assert_eq!(status.active_downloads, 0, "nothing has been queued yet");
        assert_eq!(
            status.seeding, 0,
            "no torrent engine is composed into the runtime yet (PG-E10)"
        );
        assert!(status.tray_enabled);
        runtime.shutdown().await;
    }

    /// A provider this build does not know is refused by name, and nothing is journaled.
    #[tokio::test]
    async fn an_unknown_provider_is_refused_by_name() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;

        for provider in ["nobody", ""] {
            let error = runtime
                .start_provider_credential(provider, CredentialRequest::Forget)
                .await
                .expect_err("an unknown provider cannot be asked about");
            match error {
                RuntimeError::InvalidRequest { detail } => assert!(
                    detail.contains(provider),
                    "the refusal must name what was asked for: {detail}"
                ),
                other => panic!("expected a refusal naming the provider, got {other:?}"),
            }
        }
        assert!(
            runtime.jobs(None, 10).await.unwrap().is_empty(),
            "a refused request must not leave a journal row behind"
        );
        runtime.shutdown().await;
    }

    /// The status a settings screen polls is the cached answer, so a locked or prompting keychain
    /// cannot stall a command.
    #[tokio::test]
    async fn provider_status_answers_from_the_cache_without_reading_the_store() {
        let directory = TempDir::new().unwrap();
        let secrets = Arc::new(CountingSecretStore::new());
        let (runtime, _sink) =
            runtime_with(&directory, Arc::clone(&secrets) as Arc<dyn SecretStore>).await;

        let cached = await_cached_status(&runtime).await;
        assert_eq!(cached.known, vec!["tmdb"]);
        assert!(
            cached.configured.is_empty(),
            "nothing is stored in the fixture"
        );
        let reads = secrets.reads();
        assert!(reads > 0, "the startup refresh is what filled the cache");

        assert_eq!(runtime.provider_status(), cached);
        assert_eq!(
            secrets.reads(),
            reads,
            "a polled status must answer from the cache, not from a keyring"
        );
        runtime.shutdown().await;
    }

    /// Forgetting a credential runs the whole flow and leaves the cache agreeing with the store.
    ///
    /// `Forget` is the one action that reaches no provider, which is what makes it an end-to-end
    /// test of the runtime's half without a request (AGENTS R14).
    #[tokio::test]
    async fn forgetting_a_credential_reaches_the_registrys_cache() {
        let directory = TempDir::new().unwrap();
        let secrets = Arc::new(MemorySecretStore::with_provider("tmdb", "a-key"));
        let (runtime, _sink) =
            runtime_with(&directory, Arc::clone(&secrets) as Arc<dyn SecretStore>).await;

        // Waited for, so the assertion below is about the job's own refresh rather than a race with
        // the one startup started.
        let cached = await_cached_status(&runtime).await;
        assert_eq!(cached.configured, vec!["tmdb"]);

        let started = runtime
            .start_provider_credential("tmdb", CredentialRequest::Forget)
            .await
            .unwrap();
        let row = await_terminal(&runtime, started.journal_id).await;

        assert_eq!(row.state, JobPhase::Succeeded, "{row:?}");
        assert_eq!(row.kind, crate::jobs::provider_credential::KIND);
        assert_eq!(row.dedupe_key.as_deref(), Some("provider.credential:tmdb"));
        assert_eq!(row.last_error_code, None);
        assert!(
            secrets
                .get(&SecretKey::provider("tmdb"))
                .expect("the store answers")
                .is_none()
        );
        assert!(
            runtime.provider_status().configured.is_empty(),
            "the cache must agree with the store the job wrote"
        );

        runtime.shutdown().await;
    }

    /// What the runtime writes for a `Replace` is the provider's name, even though the request
    /// carries a credential.
    ///
    /// The scheduler is stopped first, so the submitted job cannot run its probe and no request
    /// leaves the machine (AGENTS R14). That makes this an assertion about the row the runtime
    /// writes rather than about a provider being reachable — and since `JobRecord` does not carry
    /// `payload_json`, the row is read back from the bytes the database wrote.
    #[tokio::test]
    async fn the_row_the_runtime_writes_for_a_replace_carries_no_credential() {
        const KEY: &str = "super-secret-key";
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;
        runtime.shutdown().await;

        let error = runtime
            .start_provider_credential("tmdb", CredentialRequest::Replace(Secret::new(KEY)))
            .await
            .expect_err("a stopped scheduler refuses work");
        assert!(matches!(error, RuntimeError::Scheduler(_)), "{error:?}");

        // The row is written before the scheduler is asked, so it is there to read.
        let rows = runtime.jobs(None, 10).await.unwrap();
        let row = rows
            .first()
            .expect("the row was written before the submission was refused");
        assert_eq!(row.kind, crate::jobs::provider_credential::KIND);
        assert!(
            !row.last_error.as_deref().unwrap_or_default().contains(KEY),
            "the row's message carries the credential"
        );
        let carries = |bytes: &[u8]| {
            bytes
                .windows(KEY.len())
                .any(|window| window == KEY.as_bytes())
        };
        for entry in std::fs::read_dir(directory.path()).unwrap() {
            let path = entry.unwrap().path();
            if !path.is_file() {
                // The runtime makes the log and artwork directories under the same root.
                continue;
            }
            assert!(
                !carries(&std::fs::read(&path).unwrap()),
                "{} carries the credential",
                path.display()
            );
        }
    }

    /// One browse request, which is what the command surface builds out of a screen's filters.
    fn browse_request() -> peregrine_providers::BrowseRequest {
        peregrine_providers::BrowseRequest::new(
            peregrine_domain::catalogue::TitleKind::Movie,
            std::num::NonZeroU32::new(1).expect("one is not zero"),
            peregrine_providers::BrowseSort::Popularity,
            None,
            None,
        )
    }

    /// A provider that answers the genre list once and fails afterwards.
    ///
    /// The cache rule is otherwise unobservable: two calls that both ask a working provider return
    /// the same list whether the second one asked or not. A second call that *must* fail is what
    /// makes "the session remembered it" a fact rather than an assumption.
    struct GenreOnceProvider {
        inner: peregrine_testkit::FakeMetadataProvider,
        asked: std::sync::atomic::AtomicUsize,
    }

    impl GenreOnceProvider {
        fn answering(genres: Vec<peregrine_providers::GenreRef>) -> Self {
            Self {
                inner: peregrine_testkit::FakeMetadataProvider::publishing_rails(
                    peregrine_domain::id::Provider::Tmdb,
                    Vec::new(),
                )
                .with_genres_answer(genres),
                asked: std::sync::atomic::AtomicUsize::new(0),
            }
        }
    }

    impl peregrine_providers::provider::MetadataProvider for GenreOnceProvider {
        fn descriptor(&self) -> &peregrine_providers::ProviderDescriptor {
            self.inner.descriptor()
        }

        fn check(&self) -> peregrine_providers::ProviderFuture<'_, ()> {
            self.inner.check()
        }

        fn search(
            &self,
            query: &peregrine_providers::provider::SearchQuery,
        ) -> peregrine_providers::ProviderFuture<
            '_,
            Vec<peregrine_providers::provider::SearchCandidate>,
        > {
            self.inner.search(query)
        }

        fn title(
            &self,
            id: &peregrine_domain::id::ExternalId,
            kind: peregrine_domain::catalogue::TitleKind,
        ) -> peregrine_providers::ProviderFuture<'_, peregrine_domain::catalogue::TitleDetails>
        {
            self.inner.title(id, kind)
        }

        /// Delegated, so a season read in a test sees whatever the fake behind this wrapper answers.
        fn seasons(
            &self,
            id: &peregrine_domain::id::ExternalId,
            kind: peregrine_domain::catalogue::TitleKind,
        ) -> peregrine_providers::ProviderFuture<'_, Vec<peregrine_domain::catalogue::SeasonDetails>>
        {
            self.inner.seasons(id, kind)
        }

        /// Delegated, for the reason [`Self::seasons`] gives.
        fn season_episodes(
            &self,
            id: &peregrine_domain::id::ExternalId,
            kind: peregrine_domain::catalogue::TitleKind,
            season_number: peregrine_domain::catalogue::SeasonNumber,
        ) -> peregrine_providers::ProviderFuture<'_, Vec<peregrine_domain::catalogue::EpisodeDetails>>
        {
            self.inner.season_episodes(id, kind, season_number)
        }

        fn image(
            &self,
            path: &str,
            kind: peregrine_domain::catalogue::ImageKind,
        ) -> peregrine_providers::ProviderFuture<'_, Vec<u8>> {
            self.inner.image(path, kind)
        }

        fn rail(
            &self,
            rail: peregrine_providers::provider::RailKind,
            page: std::num::NonZeroU32,
        ) -> peregrine_providers::ProviderFuture<
            '_,
            Vec<peregrine_providers::provider::SearchCandidate>,
        > {
            self.inner.rail(rail, page)
        }

        fn browse(
            &self,
            request: &peregrine_providers::provider::BrowseRequest,
        ) -> peregrine_providers::ProviderFuture<'_, peregrine_providers::ProviderPage> {
            self.inner.browse(request)
        }

        fn genres(
            &self,
            kind: peregrine_domain::catalogue::TitleKind,
        ) -> peregrine_providers::ProviderFuture<'_, Vec<peregrine_providers::GenreRef>> {
            if self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                return self.inner.genres(kind);
            }
            Box::pin(async {
                Err(peregrine_providers::error::ProviderError::Transport {
                    provider: peregrine_domain::id::Provider::Tmdb,
                    kind: peregrine_providers::error::TransportFailure::Connect,
                })
            })
        }

        fn similar(
            &self,
            id: &peregrine_domain::id::ExternalId,
            kind: peregrine_domain::catalogue::TitleKind,
        ) -> peregrine_providers::ProviderFuture<
            '_,
            Vec<peregrine_providers::provider::SearchCandidate>,
        > {
            self.inner.similar(id, kind)
        }
    }

    /// A page is written into the catalogue before it is answered, so an item is a title the
    /// catalogue owns — with a row id a screen can open — rather than a row of this response only.
    ///
    /// The observable form of that is identity across two calls: the same provider id answers the
    /// same catalogue row the second time, which is only true if the first call wrote it.
    #[tokio::test]
    async fn a_browse_page_is_written_into_the_catalogue_and_answered_as_it_is_drawn() {
        let directory = TempDir::new().unwrap();
        let adapter = Arc::new(
            peregrine_testkit::FakeMetadataProvider::publishing_rails(
                peregrine_domain::id::Provider::Tmdb,
                Vec::new(),
            )
            .with_browse_answer(peregrine_providers::ProviderPage::new(
                vec![candidate("603", "The Matrix")],
                true,
            )),
        );
        let (runtime, _sink) = runtime_with_a_provider(&directory, adapter).await;

        let first = runtime.browse(&browse_request()).await.unwrap();
        assert_eq!(first.items.len(), 1);
        assert_eq!(first.items[0].name, "The Matrix");
        assert!(
            first.items[0].title_id > 0,
            "the item carries a catalogue row"
        );
        assert!(
            first.has_more,
            "the provider's own answer about another page is carried rather than guessed"
        );

        let second = runtime.browse(&browse_request()).await.unwrap();
        assert_eq!(
            second.items[0].title_id, first.items[0].title_id,
            "the identity the first call wrote is what the second call matched"
        );
        runtime.shutdown().await;
    }

    /// No configured provider can serve a queried list, so there is nothing to browse and the
    /// refusal says so rather than asking a provider that cannot answer.
    #[tokio::test]
    async fn browsing_without_a_provider_that_can_serve_a_list_is_a_typed_refusal() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;

        let refused = runtime
            .browse(&browse_request())
            .await
            .expect_err("no provider");
        assert!(
            matches!(refused, RuntimeError::NoProvider),
            "got {refused:?}"
        );
        let refused = runtime
            .genres(peregrine_domain::catalogue::TitleKind::Movie)
            .await
            .expect_err("no provider");
        assert!(
            matches!(refused, RuntimeError::NoProvider),
            "got {refused:?}"
        );
        runtime.shutdown().await;
    }

    /// A provider that can search but not list is not asked to list.
    ///
    /// The registry may hold several providers, and a ratings fallback that declares `search` alone
    /// is a normal configuration — so browse selects on the capability rather than taking the first
    /// adapter and reporting its refusal as the installation's.
    #[tokio::test]
    async fn a_search_only_provider_is_not_asked_for_a_queried_list() {
        let directory = TempDir::new().unwrap();
        let adapter = Arc::new(peregrine_testkit::FakeMetadataProvider::searching(
            peregrine_domain::id::Provider::Tmdb,
            Vec::new(),
        ));
        let (runtime, _sink) = runtime_with_a_provider(&directory, adapter).await;

        let refused = runtime
            .browse(&browse_request())
            .await
            .expect_err("no list");
        assert!(
            matches!(refused, RuntimeError::NoProvider),
            "a provider that cannot list is not the provider to ask: {refused:?}"
        );
        runtime.shutdown().await;
    }

    /// A provider's answer is the search result, even when the local index cannot spell the query.
    ///
    /// "Spiderman" has no hyphen, and the index tokenises "Spider-Man" into two terms — so
    /// re-searching the reader's text locally after the sync writes the title would answer zero for
    /// the title the provider just found. The read-back by the ids the sync attached is what makes
    /// the answer the provider's own result.
    #[tokio::test]
    async fn a_provider_search_answers_its_own_result_when_the_local_index_cannot_spell_it() {
        let directory = TempDir::new().unwrap();
        let adapter = Arc::new(peregrine_testkit::FakeMetadataProvider::searching(
            peregrine_domain::id::Provider::Tmdb,
            vec![candidate("1", "Spider-Man: Brand New Day")],
        ));
        let (runtime, _sink) = runtime_with_a_provider(&directory, adapter).await;

        let found = runtime
            .discover("spiderman", 20, None)
            .await
            .expect("the provider's answer is readable");
        assert_eq!(found.len(), 1, "the provider found the title");
        assert_eq!(found[0].name, "Spider-Man: Brand New Day");
        runtime.shutdown().await;
    }

    /// A tile search answers with the poster fields the list writer stored, not a bare identity.
    ///
    /// The provider's candidate carries the year, the score, and the synopsis a list answer does, and
    /// `search_tiles` writes them through `upsert_list_candidate` — the same writer a browse page
    /// uses — so a result row can be handed to a tile without a second read or a screen-side lookup.
    #[tokio::test]
    async fn a_provider_search_answers_tiles_with_the_metadata_the_list_carried() {
        let directory = TempDir::new().unwrap();
        let listed = peregrine_providers::provider::SearchCandidate::new(
            peregrine_domain::id::ExternalId::new(peregrine_domain::id::Provider::Tmdb, "1")
                .expect("a fixture id"),
            peregrine_domain::text::NonEmptyText::new("Spider-Man: Brand New Day")
                .expect("real text"),
            peregrine_domain::catalogue::TitleKind::Movie,
            Some("2026-07-31".parse().expect("a date")),
        )
        .with_rating(Some(8.5))
        .with_overview(Some("A spider in a world that forgot him.".to_owned()));
        let adapter = Arc::new(peregrine_testkit::FakeMetadataProvider::searching(
            peregrine_domain::id::Provider::Tmdb,
            vec![listed],
        ));
        let (runtime, _sink) = runtime_with_a_provider(&directory, adapter).await;

        let tiles = runtime
            .search_tiles("spiderman", 20, None)
            .await
            .expect("the search answers");
        assert_eq!(tiles.len(), 1, "the provider found the title");
        assert_eq!(tiles[0].name, "Spider-Man: Brand New Day");
        assert_eq!(tiles[0].year, Some(2026));
        assert_eq!(tiles[0].rating, Some(8.5));
        assert_eq!(
            tiles[0].overview.as_deref(),
            Some("A spider in a world that forgot him.")
        );
        runtime.shutdown().await;
    }

    /// A genre vocabulary is read once per kind per session: the second read answers from the
    /// runtime's own memory, which is what keeps a filter control from costing a request per visit.
    #[tokio::test]
    async fn the_genre_vocabulary_is_read_once_per_kind() {
        let directory = TempDir::new().unwrap();
        let genre = peregrine_providers::GenreRef::new(
            28,
            peregrine_domain::text::NonEmptyText::new("Action").expect("real text"),
        );
        let adapter = Arc::new(GenreOnceProvider::answering(vec![genre.clone()]));
        let (runtime, _sink) = runtime_with_a_provider(&directory, adapter).await;

        let first = runtime
            .genres(peregrine_domain::catalogue::TitleKind::Movie)
            .await
            .expect("the first read asks the provider");
        assert_eq!(first, vec![genre.clone()]);

        let second = runtime
            .genres(peregrine_domain::catalogue::TitleKind::Movie)
            .await
            .expect("the second read is answered from the session's memory");
        assert_eq!(second, first);

        // And the other kind is its own slot rather than the same one. The provider fails every call
        // after the first, so a *series* read that answered would mean the film vocabulary had been
        // handed to the series filter — which is the mistake a per-kind cache exists to prevent.
        let other = runtime
            .genres(peregrine_domain::catalogue::TitleKind::Show)
            .await;
        assert!(
            other.is_err(),
            "the film vocabulary is not the series vocabulary"
        );
        runtime.shutdown().await;
    }

    /// The genre a shelf is built from is the genre the provider actually listed the rail under.
    ///
    /// A rule rather than a formula: a genre has to appear on a quarter of the rail *and* at least
    /// twice, so one unusual neighbour cannot invent a shelf, and ties break by identifier so two runs
    /// over one answer produce the same shelves in the same order.
    #[test]
    fn shared_genres_keeps_what_a_rail_agrees_on_and_invents_nothing() {
        let listed = |genres: &[u32]| candidate("1", "A title").with_genre_ids(genres.to_vec());

        // Three of four share 28 and two share 12; one item alone carries 99.
        let rail = vec![
            listed(&[28, 12]),
            listed(&[28]),
            listed(&[28, 12]),
            listed(&[99]),
        ];
        assert_eq!(
            shared_genres(&rail),
            vec![28, 12],
            "most-shared first, and a tie would break by identifier"
        );
        assert!(
            !shared_genres(&rail).contains(&99),
            "a genre on one item of four is below the floor of two"
        );

        assert!(
            shared_genres(&[listed(&[1]), listed(&[2])]).is_empty(),
            "two items that agree on nothing offer no shelf"
        );
        assert!(
            shared_genres(&[]).is_empty(),
            "an empty rail cannot invent a shelf"
        );
    }

    /// A title's recommendations are read from the provider that owns its identifier.
    ///
    /// **The right provider, asked about the right title.** `peregrine_sync::refresh_title` states the
    /// same rule from the other direction: a title may carry several providers' identifiers, and
    /// asking one of them for another's number is how a film gets another film's recommendations.
    /// A title no configured provider holds an identifier for is answered empty rather than as a
    /// failure — a title the library inferred from a filename has none to fetch.
    #[tokio::test]
    async fn a_title_s_recommendations_come_from_the_provider_that_owns_its_identifier() {
        let directory = TempDir::new().unwrap();
        let adapter = Arc::new(
            peregrine_testkit::FakeMetadataProvider::publishing_rails(
                peregrine_domain::id::Provider::Tmdb,
                Vec::new(),
            )
            .with_similar_answer(vec![candidate("604", "The Matrix Reloaded")]),
        );
        let (runtime, _sink) = runtime_with_a_provider(&directory, adapter).await;

        let held = runtime
            .database
            .insert_stub_title(peregrine_storage::StubTitle {
                kind: peregrine_domain::catalogue::TitleKind::Movie,
                subtype: peregrine_domain::catalogue::TitleSubtype::Standard,
                name: "The Matrix".to_owned(),
                original_name: None,
                sort_name: "matrix".to_owned(),
                at: 0,
            })
            .await
            .unwrap();

        assert!(
            runtime.similar(held).await.unwrap().items.is_empty(),
            "a title no provider can be asked about has no recommendations to fetch"
        );

        runtime
            .database
            .attach_external_ids(
                held,
                &[peregrine_storage::TitleExternalId {
                    id: peregrine_domain::id::ExternalId::new(
                        peregrine_domain::id::Provider::Tmdb,
                        "603",
                    )
                    .expect("a fixture id"),
                    is_primary: true,
                    fetched_at: 0,
                }],
            )
            .await
            .unwrap();

        let found = runtime.similar(held).await.unwrap();
        assert_eq!(found.items.len(), 1);
        assert_eq!(found.items[0].name, "The Matrix Reloaded");
        assert!(
            found.items[0].title_id > 0,
            "a recommendation is a title the catalogue owns"
        );

        // A title the catalogue does not hold is empty rather than an error, which is the same answer
        // `library_title` gives for one this account may not see (I37).
        let absent = runtime.similar(9_999).await.unwrap();
        assert!(
            absent.items.is_empty(),
            "a title the catalogue does not hold has no recommendations, and that is not an error: {absent:?}"
        );
        runtime.shutdown().await;
    }

    /// A scanned library holding one file, and the title the scan made of it.
    ///
    /// **A real scan of a real file rather than hand-written rows**, and that is the point: what
    /// `start_playback` needs is a path the *scanner* wrote, a root that path is really inside, and a
    /// container the file's name really states. A fixture that inserted `media_file` rows directly
    /// would be asserting a shape no scan produces, and the containment check is exactly the kind of
    /// thing that passes such a fixture and fails a real library.
    async fn scanned(directory: &TempDir, name: &str) -> (Runtime, i64) {
        let movies = directory.path().join("Movies");
        std::fs::create_dir_all(&movies).expect("the library folder is creatable");
        std::fs::write(movies.join(name), vec![9_u8; 4096]).expect("the fixture file writes");
        let (runtime, _sink) = runtime(directory).await;
        runtime
            .add_library_root(
                &movies.to_string_lossy(),
                LibraryRootKind::Movies,
                ScanMode::Manual,
            )
            .await
            .expect("the folder becomes a root");
        let started = runtime.start_library_scan().await.expect("a scan starts");
        let row = await_terminal(&runtime, started.journal_id).await;
        assert_eq!(
            row.state,
            JobPhase::Succeeded,
            "the scan must succeed: {row:?}"
        );
        let listed = runtime.library_titles(None, 10).await.unwrap();
        let title = listed
            .into_iter()
            .next()
            .expect("the scan indexed the file as a title");
        (runtime, title.row.title.id)
    }

    /// The whole path, end to end: a file on disk, a scan, a Play press, and a URL that serves the
    /// bytes.
    ///
    /// The HTTP behaviour is `peregrine-stream`'s to assert; what this owns is the wiring — that the
    /// title resolves, that the file is the one the library holds, that the path passes containment, and
    /// that a session is open afterwards.
    #[tokio::test]
    async fn a_scanned_file_can_be_played_over_a_loopback_url() {
        let directory = TempDir::new().unwrap();
        let (runtime, id) = scanned(&directory, "Alien (1979).mp4").await;

        let started = runtime
            .start_playback(id, None)
            .await
            .expect("the film plays");

        assert_eq!(started.title_id, id);
        assert!(started.media_file_id > 0);
        assert_eq!(
            started.support,
            LocalPlayback::Webview,
            "an MP4 is what the webview's own media stack reads, which is the case this whole slice \
             exists for"
        );
        assert!(
            started.url.starts_with("http://127.0.0.1:"),
            "loopback only, and the port is ephemeral: {}",
            started.url
        );
        assert!(
            !started
                .url
                .contains(&directory.path().to_string_lossy().into_owned()),
            "the URL names the session, never the file: {}",
            started.url
        );
        assert!(runtime.is_playing());
        // The socket is really listening, which the URL alone cannot show.
        let port: u16 = started
            .url
            .rsplit(':')
            .next()
            .and_then(|tail| tail.split('/').next())
            .and_then(|text| text.parse().ok())
            .expect("the url carries a port");
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("the session is listening");

        assert!(
            runtime.stop_playback(started.session_id).await,
            "a stop that names the session it opened closes it"
        );
        assert!(!runtime.is_playing(), "and stopping ends it");

        runtime.shutdown().await;
    }

    /// A completed download lives in the app own download root rather than a configured library
    /// root, and the external hand-off must still reach it. This is the path that makes a
    /// downloaded MKV playable at all.
    #[tokio::test]
    async fn a_downloaded_mkv_is_handed_to_the_external_player() {
        use peregrine_domain::{MediaFile, MediaItem, MediaItemState, MediaOrigin, NonEmptyText};

        let directory = TempDir::new().unwrap();
        let player = Arc::new(RecordingPlayer::default());
        let (runtime, _sink) = runtime_with_player(&directory, Arc::clone(&player)).await;

        let folder = directory.path().join("downloads").join("Interstellar");
        std::fs::create_dir_all(&folder).expect("the download folder");
        let film = folder.join("Interstellar (2014).mkv");
        std::fs::write(&film, vec![1_u8; 16]).expect("the fixture file");

        let at = jiff::Timestamp::from_millisecond(0).expect("in range");
        let title_id = runtime
            .database
            .insert_stub_title(peregrine_storage::StubTitle {
                kind: peregrine_domain::catalogue::TitleKind::Movie,
                subtype: peregrine_domain::catalogue::TitleSubtype::Standard,
                name: "Interstellar".to_owned(),
                original_name: None,
                sort_name: "interstellar".to_owned(),
                at: 0,
            })
            .await
            .expect("the title");
        let item_id = runtime
            .database
            .insert_media_item(MediaItem {
                id: 0,
                title_id,
                episode_id: None,
                state: MediaItemState::Available,
                origin: MediaOrigin::Torrent,
                preferred_file_id: None,
                quality_label: None,
                created_at: at,
                updated_at: at,
            })
            .await
            .expect("the item");
        runtime
            .database
            .insert_media_file(MediaFile {
                id: 0,
                media_item_id: item_id,
                path: NonEmptyText::new(film.to_string_lossy().as_ref()).expect("fixture text"),
                // No root owns a download: it lives under the app own directory, which is exactly
                // the path the guard has to allow.
                root_id: None,
                size_bytes: 16,
                mtime_ms: at,
                added_at: at,
                missing_since: None,
                container: None,
                duration_ms: None,
                video_codec: None,
                video_profile: None,
                width: None,
                height: None,
                bit_depth: None,
                hdr_format: None,
                frame_rate: None,
                bitrate_kbps: None,
                audio_codec: None,
                audio_channels: None,
                audio_layout: None,
                audio_languages: None,
                subtitle_format: None,
                probe_version: None,
                probe_json: None,
                quality_label: None,
                updated_at: at,
            })
            .await
            .expect("the file");

        runtime
            .start_external_playback(title_id)
            .await
            .expect("the player is handed the file");
        assert_eq!(player.opened(), vec![film.clone()]);
        runtime.shutdown().await;
    }

    /// A downloaded title: a file under the app's own download root and the library rows the
    /// import would have written, which is the shape `start_external_playback` must reach.
    async fn downloaded_title(runtime: &Runtime, directory: &TempDir) -> (i64, PathBuf) {
        use peregrine_domain::{MediaFile, MediaItem, MediaItemState, MediaOrigin, NonEmptyText};

        let folder = directory.path().join("downloads").join("Interstellar");
        std::fs::create_dir_all(&folder).expect("the download folder");
        let film = folder.join("Interstellar (2014).mkv");
        std::fs::write(&film, vec![1_u8; 16]).expect("the fixture file");

        let at = jiff::Timestamp::from_millisecond(0).expect("in range");
        let title_id = runtime
            .database
            .insert_stub_title(peregrine_storage::StubTitle {
                kind: peregrine_domain::catalogue::TitleKind::Movie,
                subtype: peregrine_domain::catalogue::TitleSubtype::Standard,
                name: "Interstellar".to_owned(),
                original_name: None,
                sort_name: "interstellar".to_owned(),
                at: 0,
            })
            .await
            .expect("the title");
        let item_id = runtime
            .database
            .insert_media_item(MediaItem {
                id: 0,
                title_id,
                episode_id: None,
                state: MediaItemState::Available,
                origin: MediaOrigin::Torrent,
                preferred_file_id: None,
                quality_label: None,
                created_at: at,
                updated_at: at,
            })
            .await
            .expect("the item");
        runtime
            .database
            .insert_media_file(MediaFile {
                id: 0,
                media_item_id: item_id,
                path: NonEmptyText::new(film.to_string_lossy().as_ref()).expect("fixture text"),
                root_id: None,
                size_bytes: 16,
                mtime_ms: at,
                added_at: at,
                missing_since: None,
                container: None,
                duration_ms: None,
                video_codec: None,
                video_profile: None,
                width: None,
                height: None,
                bit_depth: None,
                hdr_format: None,
                frame_rate: None,
                bitrate_kbps: None,
                audio_codec: None,
                audio_channels: None,
                audio_layout: None,
                audio_languages: None,
                subtitle_format: None,
                probe_version: None,
                probe_json: None,
                quality_label: None,
                updated_at: at,
            })
            .await
            .expect("the file");
        (title_id, film)
    }

    /// The external player resumes from the app's own watch progress, and the position it reports
    /// is written through the same row the in-page player writes.
    #[tokio::test]
    async fn an_external_playback_resumes_and_reports_progress() {
        let directory = TempDir::new().unwrap();
        let player = Arc::new(RecordingPlayer::default());
        let (runtime, _sink) = runtime_with_player(&directory, Arc::clone(&player)).await;
        let (title_id, _film) = downloaded_title(&runtime, &directory).await;

        runtime
            .record_progress(title_id, None, 42_000, Some(7_200_000))
            .await
            .expect("the seed position is written");
        runtime
            .start_external_playback(title_id)
            .await
            .expect("the player starts");
        assert_eq!(
            player.start(),
            Some(42_000),
            "the stored position is where mpv starts"
        );

        // The fake reports 84 s over the sink; the consumer task writes it. Wait for the row rather
        // than assuming the task has run by the time `start` returns.
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                // The raw row rather than the rail: Continue Watching hides a position under 2 %,
                // and this fixture is a two-hour film at 84 s.
                let progress = runtime
                    .database
                    .watch_progress(installation_scope(), title_id, None)
                    .await
                    .expect("the read");
                if progress.is_some_and(|progress| progress.position_ms() == 84_000) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the external position reaches the progress row");
        runtime.shutdown().await;
    }
    /// A second press replaces the first session rather than opening a second listener.
    ///
    /// SECURITY.md T10 asks for a socket that dies with its session, and "one at a time" is what makes
    /// that reachable: a runtime holding two sessions could not close both when the window went away.
    #[tokio::test]
    async fn playing_again_replaces_the_session_and_frees_the_first_port() {
        let directory = TempDir::new().unwrap();
        let (runtime, id) = scanned(&directory, "Alien (1979).mp4").await;

        let first = runtime
            .start_playback(id, None)
            .await
            .expect("the first press plays");
        let first_port: u16 = first
            .url
            .rsplit(':')
            .next()
            .unwrap()
            .split('/')
            .next()
            .unwrap()
            .parse()
            .unwrap();

        let second = runtime
            .start_playback(id, None)
            .await
            .expect("the second press plays");

        assert_ne!(
            first.url, second.url,
            "a new session, not the old URL re-answered"
        );
        assert!(
            tokio::net::TcpStream::connect(("127.0.0.1", first_port))
                .await
                .is_err(),
            "the replaced session's socket is closed"
        );
        assert!(runtime.is_playing());

        runtime.shutdown().await;
    }

    /// A container the webview does not demux is answered as such, so the page can say why.
    ///
    /// The tenth file in this project's own library is an HEVC MKV, and a Play control that offered it
    /// without a word would produce a black rectangle and a failed decode (`ADR 0023 §5`).
    #[tokio::test]
    async fn a_container_the_webview_cannot_read_is_named_before_it_is_pressed() {
        let directory = TempDir::new().unwrap();
        let (runtime, id) = scanned(&directory, "Nuremberg (2025).mkv").await;

        let detail = runtime
            .library_title(id)
            .await
            .unwrap()
            .expect("the fixture's title");

        assert_eq!(
            detail.playback.support,
            Some(LocalPlayback::ExternalPlayer),
            "an MKV needs mpv, and the page is told so before the press"
        );
        assert_eq!(
            detail.playback.offer,
            peregrine_domain::TitleOffer::Play,
            "the library does hold it, so the offer is not the thing that changes"
        );

        runtime.shutdown().await;
    }

    /// A poster is captured only for a title that has none, and a stored frame stops the asking.
    ///
    /// **The rule that stops a film still overwriting real artwork.** A capture costs a decode of the film
    /// and a blob in the cache, and it exists for the title a provider has drawn nothing for — so the
    /// answer is `true` only when neither a provider's poster nor a generated one is on disk, and it turns
    /// `false` the moment either arrives.
    #[tokio::test]
    async fn a_poster_is_wanted_only_while_a_title_has_none() {
        let directory = TempDir::new().unwrap();
        let (runtime, id) = scanned(&directory, "Alien (1979).mp4").await;

        assert!(
            runtime.needs_poster(id).await.unwrap(),
            "a freshly scanned stub is exactly the title a capture exists for"
        );

        // A provider that has filled the title in with a poster path, and the blob the fetch left.
        runtime
            .database
            .update_title_metadata(
                id,
                details("Alien", Some("/poster.jpg")),
                "alien".to_owned(),
                peregrine_storage::MetadataState::Partial,
                "tmdb".to_owned(),
                1,
            )
            .await
            .unwrap();
        assert!(
            runtime.needs_poster(id).await.unwrap(),
            "the catalogue records a poster the cache does not hold, so there is still nothing to draw"
        );

        let key = peregrine_sync::ImageKey::for_image(
            peregrine_domain::Provider::Tmdb,
            "/poster.jpg",
            peregrine_sync::ImageVariant::PosterW342,
        );
        peregrine_sync::ImageCache::new(runtime.config().artwork_directory())
            .store(&key, b"pretend this is a jpeg")
            .expect("the artwork directory is writable");
        assert!(
            !runtime.needs_poster(id).await.unwrap(),
            "the provider's poster is on disk, so a film still would be a downgrade"
        );

        runtime.shutdown().await;
    }

    /// A frame this machine generated is a poster, and it stops the asking exactly as a provider's does.
    #[tokio::test]
    async fn a_generated_poster_answers_the_read_even_though_no_provider_drew_one() {
        let directory = TempDir::new().unwrap();
        let (runtime, id) = scanned(&directory, "Alien (1979).mp4").await;
        assert!(runtime.needs_poster(id).await.unwrap());

        // The sink a capture route stores through, driven the way the route drives it — on a blocking
        // thread, which is what `ThumbnailSink::store`'s contract requires and what makes its `block_on`
        // legal. The route itself is `peregrine-stream`'s to test; what this owns is that a stored frame
        // reaches the reads.
        let posters = Arc::clone(&runtime.posters);
        tokio::task::spawn_blocking(move || {
            peregrine_stream::ThumbnailSink::store(
                posters.as_ref(),
                id,
                &[0xff, 0xd8, 0xff, 0xe0, 0x00, 0x01],
            )
        })
        .await
        .expect("the blocking task completes")
        .expect("the artwork directory is writable");

        assert!(!runtime.needs_poster(id).await.unwrap());

        let listed = runtime.library_titles(None, 10).await.unwrap();
        let entry = listed.first().expect("the fixture's title");
        assert!(
            entry.poster.is_some(),
            "the tile has a picture: a generated poster is the fallback the screen draws"
        );
        assert!(!entry.needs_poster);

        let detail = runtime.library_title(id).await.unwrap().expect("the title");
        assert!(
            detail.playback.support.is_some(),
            "and the page still knows what would play"
        );

        runtime.shutdown().await;
    }

    /// A title the library holds nothing of cannot be played, and a title that is not here is a
    /// different refusal.
    #[tokio::test]
    async fn playing_something_the_library_does_not_hold_is_refused_by_name() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;

        let refused = runtime
            .start_playback(4_242, None)
            .await
            .expect_err("nothing is here to play");

        assert!(
            matches!(refused, RuntimeError::UnknownTitle),
            "a title that does not exist reads the same as one this account may not see (I37): {refused:?}"
        );
        assert!(!runtime.is_playing(), "and no session was left open");

        runtime.shutdown().await;
    }

    /// Stopping when nothing is playing is an answer rather than a failure.
    #[tokio::test]
    async fn stopping_nothing_reports_that_it_stopped_nothing() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;

        // A stop naming a session this runtime never opened is `false` rather than an error: there is no
        // state in which asking for silence should be refused, and nothing of the caller's was open.
        assert!(!runtime.stop_playback(SessionId::default()).await);

        runtime.shutdown().await;
    }

    /// The file a press would use: an item's own preference wins, and a copy the scan could not find
    /// never does.
    #[test]
    fn the_playable_file_follows_the_library_s_own_preference() {
        use peregrine_domain::{MediaFile, MediaItem, MediaItemState, MediaOrigin, NonEmptyText};
        use peregrine_storage::{ItemEvidence, TitleEvidence};

        fn file(id: i64, name: &str, missing: bool) -> MediaFile {
            let at = jiff::Timestamp::from_millisecond(1_700_000_000_000).expect("in range");
            let mut file = MediaFile {
                id,
                media_item_id: 1,
                path: NonEmptyText::new(name).expect("fixture text"),
                root_id: Some(1),
                size_bytes: 4_096,
                mtime_ms: at,
                added_at: at,
                missing_since: None,
                container: None,
                duration_ms: None,
                video_codec: None,
                video_profile: None,
                width: None,
                height: None,
                bit_depth: None,
                hdr_format: None,
                frame_rate: None,
                bitrate_kbps: None,
                audio_codec: None,
                audio_channels: None,
                audio_layout: None,
                audio_languages: None,
                subtitle_format: None,
                probe_version: None,
                probe_json: None,
                quality_label: None,
                updated_at: at,
            };
            if missing {
                file.mark_missing(at);
            }
            file
        }

        fn item(preferred: Option<i64>, files: Vec<MediaFile>) -> ItemEvidence {
            let at = jiff::Timestamp::from_millisecond(1_700_000_000_000).expect("in range");
            ItemEvidence {
                item: MediaItem {
                    id: 1,
                    title_id: 7,
                    episode_id: None,
                    state: MediaItemState::Available,
                    origin: MediaOrigin::Scan,
                    preferred_file_id: preferred,
                    quality_label: None,
                    created_at: at,
                    updated_at: at,
                },
                files,
                jobs: Vec::new(),
            }
        }

        // Read order says the first file; the item's preference says the second, and the preference is
        // what a person chose.
        let evidence = TitleEvidence {
            items: vec![item(
                Some(2),
                vec![file(1, "/a.mp4", false), file(2, "/b.mkv", false)],
            )],
            jobs: Vec::new(),
        };
        assert_eq!(
            playable_file(&evidence, None).map(|file| file.id),
            Some(2),
            "the item's preferred file wins over read order"
        );

        // A preference for a copy the scan could not find falls through rather than answering a file
        // that cannot be opened.
        let evidence = TitleEvidence {
            items: vec![item(
                Some(1),
                vec![file(1, "/a.mp4", true), file(2, "/b.mkv", false)],
            )],
            jobs: Vec::new(),
        };
        assert_eq!(playable_file(&evidence, None).map(|file| file.id), Some(2));

        // Nothing present is nothing to play, which is the state `NoPlayableFile` names.
        let evidence = TitleEvidence {
            items: vec![item(None, vec![file(1, "/a.mp4", true)])],
            jobs: Vec::new(),
        };
        assert!(playable_file(&evidence, None).is_none());
        assert!(
            playable_file(
                &TitleEvidence {
                    items: Vec::new(),
                    jobs: Vec::new(),
                },
                None,
            )
            .is_none(),
            "a title the library holds nothing of has nothing to play"
        );
    }
}
