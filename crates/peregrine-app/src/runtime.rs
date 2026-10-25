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
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::Duration;

use std::num::NonZeroU32;

use crate::notify::{
    Consent, InstalledOsNotifier, NotificationService, NotifyAcquisition, Preferences,
    StoredPreferences,
};
use peregrine_domain::catalogue::{ImageKind, Region, WatchCategory, watch_brand};
use peregrine_domain::centre::{CentreEntry, CentreError, NotificationCentre as _, NotificationId};
use peregrine_domain::notify::{Focus, NotificationEvent, Platform};
use peregrine_domain::text::NonEmptyText;
use peregrine_domain::watch::finalize as finalize_watch;
use peregrine_domain::{
    AccountScope, Clock, IdGenerator, LibraryRoot, LibraryRootKind, LocalPlayback, LoopbackUrl,
    PlaybackPosition, PlaybackPreference, PlayerInput, PreferredQuality, ProgressSource, SafePath,
    ScanMode, SubtitleFormat, SubtitleTrack, TokenSource, WatchOrigin, WatchProgress,
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

/// The account the installation is watching as.
///
/// The picker changes this once, on the process, and every scoped read and write resolves it
/// through [`installation_scope`]. The value starts at the seeded default and is replaced at
/// startup from the persisted choice. A web request deliberately does not override it: one
/// household profile is active at a time across the desktop and every paired browser.
static ACTIVE_ACCOUNT: AtomicI64 = AtomicI64::new(1);
static ACTIVE_RESTRICTED: AtomicBool = AtomicBool::new(false);
static ACTIVE_CAN_ACQUIRE: AtomicBool = AtomicBool::new(true);

/// The scope every user-facing read in this build runs under.
///
/// The id and the restricted flag are the active profile's; a child profile, a rating ceiling,
/// or a curated-only account sets the flag, which is what makes the storage predicates hide
/// adult and unrated content. The full rating map and the capability gates are E35's.
#[must_use]
pub fn installation_scope() -> peregrine_domain::AccountScope {
    let account = peregrine_domain::AccountId::new(ACTIVE_ACCOUNT.load(Ordering::Relaxed))
        .unwrap_or_else(|_| peregrine_domain::AccountId::new(1).expect("account one is positive"));
    if ACTIVE_RESTRICTED.load(Ordering::Relaxed) {
        peregrine_domain::AccountScope::restricted(account)
    } else {
        peregrine_domain::AccountScope::unrestricted(account)
    }
}

/// Makes `account` the scope every later read and write uses.
fn activate(account: &peregrine_storage::AccountRow) {
    ACTIVE_ACCOUNT.store(account.id, Ordering::Relaxed);
    ACTIVE_RESTRICTED.store(account.is_restricted(), Ordering::Relaxed);
    ACTIVE_CAN_ACQUIRE.store(account.can_acquire, Ordering::Relaxed);
}

/// Writes a position a player reported, preserving a flag the user set by hand.
///
/// A free function because two writers need the same rule and only one of them holds the runtime:
/// the in-page player's command and the external player's socket reader, which runs on its own task.
/// A manual row moves its position and nothing else; a measured row is written exactly as
/// [`WatchProgress::record`] describes, because a tick is not evidence that the session ended and
/// the 0.92 storage floor is not the 0.95 playback threshold.
async fn write_reported_position(
    database: &Database,
    clock: &(dyn Clock + Send + Sync),
    title_id: i64,
    episode_id: Option<i64>,
    position: PlaybackPosition,
) -> Result<(), StorageError> {
    let now = clock.now();
    let progress = match database
        .watch_progress(installation_scope(), title_id, episode_id)
        .await?
    {
        Some(mut existing) if existing.source() == ProgressSource::Manual => {
            existing.set_position(position, now);
            existing
        }
        _ => WatchProgress::record(
            installation_scope().account(),
            title_id,
            episode_id,
            position,
            ProgressSource::Local,
            now,
            None,
        ),
    };
    database.record_watch_progress(progress).await
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

/// How many episode stills one startup pass may attempt.
///
/// A library downloaded before the capture fallback existed can hold thousands of NULL stills, and
/// a pass that worked through all of them would run ffmpeg for minutes after every launch. Eight is
/// enough to answer a season in one launch and leaves later launches to continue where this one
/// stopped, because an episode that gained a still is no longer a candidate.
const STILL_BACKFILL_ATTEMPTS: usize = 8;

/// The pause between two still capture attempts in that pass.
///
/// Long enough that a launch with eight episodes to fill does not run ffmpeg back to back, short
/// enough that the pass finishes while the reader is still looking at the library.
const STILL_BACKFILL_INTERVAL: Duration = Duration::from_secs(3);

/// How long a title's watch availability is trusted before a visit asks again.
///
/// Services change what they carry on their own clock, and seven days is short enough that a film
/// that just left a service stops being advertised without turning every visit into a request. The
/// rule lives here rather than in a screen because it is the same kind of question `refresh_due`
/// answers for metadata, and the screen already follows that answer.
const WATCH_AVAILABILITY_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1_000;

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
    /// The system-sleep assertion (PG-E1-17). Injected so a test can record what was held and
    /// released, and so the shell supplies the per-OS adapter while every other host takes the
    /// no-op default.
    power: Arc<dyn peregrine_platform::PowerSave>,
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
            power: Arc::new(peregrine_platform::NoPowerSave),
        }
    }

    /// Replaces the system-sleep assertion.
    ///
    /// The default holds nothing, which keeps a test from touching the machine's power state; the
    /// shell is the one caller that supplies the platform's own adapter.
    #[must_use]
    pub fn with_power(mut self, power: Arc<dyn peregrine_platform::PowerSave>) -> Self {
        self.power = power;
        self
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

/// One sidecar subtitle track a playback session can serve.
///
/// The track's own facts come from the `subtitle_track` row a scan wrote; the URL is the session's,
/// because only the core knows the listener's port and token. A renderer hands the URL to a track
/// element and never assembles one (AGENTS R11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackSubtitle {
    /// The `subtitle_track` row id, which is the route segment.
    pub id: i64,
    /// The language, as a BCP-47 tag.
    pub language: String,
    /// The label to show instead of the language.
    pub label: Option<String>,
    /// The container, as `subtitle_track.format` spells it.
    pub format: String,
    /// Whether the track only covers forced narrative.
    pub forced: bool,
    /// Whether the track carries captions for the hard of hearing.
    pub sdh: bool,
    /// Whether the track is preferred when nothing else is chosen.
    pub default: bool,
    /// The loopback URL a track element follows.
    pub url: String,
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
    /// The sidecar subtitle tracks this session can serve, from `subtitle_track`.
    ///
    /// Empty when the file has no sidecar the library indexed, when the caller opened a torrent session,
    /// and when every row's path was refused by the path guard. A session with none answers no track id,
    /// which is a fact about the film rather than about a request.
    pub subtitles: Vec<PlaybackSubtitle>,
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

/// A live job's media, ready to be served from the engine.
struct TorrentSource {
    /// The engine reader the server opens per range request.
    media: Arc<dyn peregrine_stream::TorrentMedia>,
    /// The entry's own name, which is what classifies the container for the page and the player.
    support: LocalPlayback,
}

/// Why a job's payload did not name exactly one playable file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileChoice {
    /// The metadata is not here yet, or no selected entry is a media file.
    NotReady,
    /// Several media entries, and none or several match the episode asked for.
    Ambiguous,
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

/// One person and the library titles they are credited on.
#[derive(Debug, Clone, PartialEq)]
pub struct PersonPage {
    /// Their name.
    pub name: String,
    /// The cached portrait's absolute path, when one is cached.
    pub profile: Option<String>,
    /// The titles in the library that credit them, newest first.
    pub titles: Vec<LibraryTitleEntry>,
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

/// Where one title can be watched in one region, as a title page draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchAvailabilityView {
    /// The region these offers are for.
    pub region: Region,
    /// The provider's own page for every option, when it sent one.
    pub link: Option<String>,
    /// When the answer was fetched, or `None` when this title has never been asked about.
    ///
    /// The distinction the screen needs: `None` hides the section, and `Some` with no offers says
    /// that the provider answered and nothing is listed where the reader is.
    pub fetched_at: Option<i64>,
    /// The offers, grouped by the screen from their category.
    pub offers: Vec<WatchOfferView>,
}

/// One service on a where-to-watch row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchOfferView {
    /// Subscription, rental, purchase, free, or ad-supported.
    pub category: WatchCategory,
    /// The provider's own id for the service.
    pub provider_id: i64,
    /// The service's name, which is also the logo's accessible name.
    pub provider_name: String,
    /// Where the cached logo is, or `None` when the cache holds none.
    pub logo: Option<PathBuf>,
    /// The provider's ordering hint within its category.
    pub display_priority: Option<u32>,
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

/// The one selected media entry a job's payload supplies for the play asked for.
///
/// **A single media entry needs no matching at all**, which is the common episode release and every
/// film: the job is scoped to the title or the episode by its own row, so the only file is the answer.
///
/// **Several need a name.** A pack, or a film with extras, cannot be resolved from the job row: the
/// requested season and number are matched against each entry's own file name with the release parser,
/// and exactly one match wins. None or several is a refusal rather than a pick by size — playing the
/// wrong episode is a worse failure than admitting the file could not be identified.
fn choose_download_file(
    files: &[peregrine_storage::DownloadFileRow],
    episode: Option<(u32, u32)>,
) -> Result<usize, FileChoice> {
    let media: Vec<usize> = files
        .iter()
        .enumerate()
        .filter(|(_, file)| file.selected && file.is_media)
        .map(|(index, _)| index)
        .collect();
    match media.as_slice() {
        [] => Err(FileChoice::NotReady),
        [only] => Ok(*only),
        many => match episode {
            // A film's payload with several media entries is feature-plus-extras; the largest is the
            // feature and is what the read before this change already chose.
            None => many
                .iter()
                .copied()
                .max_by_key(|index| files[*index].size_bytes)
                .ok_or(FileChoice::NotReady),
            Some((season, number)) => {
                let matching: Vec<usize> = many
                    .iter()
                    .copied()
                    .filter(|index| {
                        let path = files[*index].path.as_str();
                        // The entry's own name, not the release folder's: a pack folder states the
                        // season but only the file states which episode inside it.
                        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
                        let parsed = peregrine_domain::ReleaseName::parse(name);
                        parsed.season() == Some(season)
                            && parsed
                                .episodes()
                                .is_some_and(|span| span.first() <= number && number <= span.last())
                    })
                    .collect();
                match matching.as_slice() {
                    [only] => Ok(*only),
                    _ => Err(FileChoice::Ambiguous),
                }
            }
        },
    }
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
    /// The notification service, behind the capability the acquisition path uses. The centre
    /// row is the record; the OS toast goes through the shell's `TauriOsNotifier`, installed
    /// before startup and gated on the consent this service stores.
    notifications: Arc<dyn NotifyAcquisition>,
    /// The same service, for the consent the Settings screen reads and writes.
    ///
    /// Kept beside the narrowed [`NotifyAcquisition`] handle because the two audiences differ: a
    /// job may raise only its own events, while the consent question belongs to the whole service.
    notification_service: Arc<Notifications>,
}

/// The notification service this runtime composes.
///
/// The storage repository appears twice because it implements both ports the service needs: the
/// centre that stores the row, and the catalogue question asked before a title may be named.
type Notifications = NotificationService<
    NotificationRepository,
    StoredPreferences,
    InstalledOsNotifier,
    NotificationRepository,
>;

/// The acquire path's view of the notification service.
///
/// A job holds this rather than the service so it does not have to name four generic ports, and
/// so what it can do is the two events it owns rather than the whole service.
struct AcquisitionNotifier {
    service: Arc<Notifications>,
    bus: Arc<EventBus>,
}

impl AcquisitionNotifier {
    /// Records one event and tells the centre to re-read it.
    ///
    /// The centre row is the record and the toast is a bonus (R31), so a failure here is
    /// logged and never changes the acquire job's outcome. The refresh is published only on
    /// success, because a row that was not written is not news.
    async fn raise(
        &self,
        scope: AccountScope,
        event: NotificationEvent,
        title_id: i64,
        label: Option<&str>,
    ) {
        match self
            .service
            .notify(scope, event, title_id, label, Focus::default())
            .await
        {
            Ok(_) => {
                // A trigger to re-read rather than the row itself: the bell asks the centre,
                // so a subscriber can never render copy the repository has not validated.
                self.bus.publish(Event::new(
                    NOTIFICATION_RAISED,
                    title_id.to_string(),
                    serde_json::json!({ "event": event.as_str() }),
                ));
            }
            Err(error) => warn!(%error, "the acquisition notification could not be recorded"),
        }
    }
}

impl NotifyAcquisition for AcquisitionNotifier {
    fn offers_available<'a>(
        &'a self,
        scope: AccountScope,
        title_id: i64,
        label: Option<String>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.raise(
                scope,
                NotificationEvent::ReleaseAlternatives,
                title_id,
                label.as_deref(),
            )
            .await;
        })
    }

    fn failed<'a>(
        &'a self,
        scope: AccountScope,
        title_id: i64,
        label: Option<String>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.raise(
                scope,
                NotificationEvent::AcquisitionFailed,
                title_id,
                label.as_deref(),
            )
            .await;
        })
    }
}

/// The acquisition path's view of the notification service.
///
/// A finished transfer is the one event the job cannot raise for itself: its handle resolves
/// elsewhere and nothing on the completion path awaits it (PG-E13-08).
struct DownloadNotifier {
    service: Arc<Notifications>,
    bus: Arc<EventBus>,
}

impl crate::acquisition::DownloadNotices for DownloadNotifier {
    fn finished<'a>(
        &'a self,
        download_id: i64,
        failed: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let event = if failed {
                NotificationEvent::DownloadFailed
            } else {
                NotificationEvent::DownloadCompleted
            };
            // The centre row is the record and the toast is a bonus (R31); a failure here is logged
            // and never changes the job's outcome.
            match self
                .service
                .notify(
                    installation_scope(),
                    event,
                    download_id,
                    None,
                    Focus::default(),
                )
                .await
            {
                Ok(_) => {
                    self.bus.publish(Event::new(
                        NOTIFICATION_RAISED,
                        download_id.to_string(),
                        serde_json::json!({ "event": event.as_str() }),
                    ));
                }
                Err(error) => warn!(%error, "the download notification could not be recorded"),
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

        // A restore is staged while the app runs and applied before anything opens the file:
        // the live database has a writer task and a reader pool, and SQLite offers no swap
        // beneath them. Doing it here means the next launch starts on the restored file, and a
        // file that cannot be opened puts the previous database back instead of taking the
        // installation with it (ADR 0033).
        let restored =
            crate::backup::apply_pending_restore(config.data_directory(), &config.database_path())
                .await?;
        // A recovery is logged where it happens, in `Database::open`; the runtime carries
        // the signal on to health.
        let database = match Database::open(config.database_path()).await {
            Ok(database) => Arc::new(database),
            Err(error) => {
                if !restored {
                    return Err(error.into());
                }
                warn!(
                    %error,
                    "the restored database could not be opened; putting the previous one back"
                );
                crate::backup::roll_back_restore(config.data_directory(), &config.database_path())
                    .await?;
                Arc::new(Database::open(config.database_path()).await?)
            }
        };
        if restored {
            info!(
                database = %config.database_path().display(),
                "the restored database is open"
            );
        }
        // The last profile the household chose applies before the first read of this process.
        match database.active_profile().await {
            Ok(Some(account)) => activate(&account),
            Ok(None) => {}
            Err(error) => warn!(%error, "the active profile could not be read"),
        }
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
        // A file removed in Finder leaves an item claiming the library holds it; the scan cannot
        // see what is not there, so startup is where that state is repaired. A failure is not
        // fatal: the library lists a phantom tile, which is recoverable, rather than no library.
        match database.remove_empty_items().await {
            Ok(true) => info!("removed downloaded items whose files are gone"),
            Ok(false) => {}
            Err(error) => warn!(%error, "empty media items could not be removed"),
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

        // The notification service, composed here because this is the only place that may pick the
        // adapters. The shell installs the OS notifier before startup (PG-E1-14); the store below
        // carries the user's consent (PG-E1-20) and leaves the per-event choices to the catalogue
        // until PG-E20-01. The centre row is written either way, which is R31's order and the reason
        // a refused toast is not an error.
        let notification_service: Arc<Notifications> = Arc::new(NotificationService::new(
            NotificationRepository::new(Arc::clone(&database)),
            StoredPreferences::new(Arc::clone(&database), Arc::clone(&clock)),
            InstalledOsNotifier::installed(),
            NotificationRepository::new(Arc::clone(&database)),
            Box::new(SharedClock(Arc::clone(&clock))),
            Platform::current(),
        ));
        let notifications: Arc<dyn NotifyAcquisition> = Arc::new(AcquisitionNotifier {
            service: Arc::clone(&notification_service),
            bus: Arc::clone(&bus),
        });
        let download_notices: Arc<dyn crate::acquisition::DownloadNotices> =
            Arc::new(DownloadNotifier {
                service: Arc::clone(&notification_service),
                bus: Arc::clone(&bus),
            });

        // The download root. `RuntimeConfig` carries no configured download directory — library
        // roots live in the database and are read per scan rather than cached here — so the session
        // writes under the data directory, the one directory the app already owns and creates. A
        // downloads root the user configured can replace it once the settings registry lands
        // (PG-E20-01); this is deliberately not a second reading of that table.
        let download_root = config.data_directory().join(DOWNLOAD_DIRECTORY);
        // A downloaded episode with no provider still gets one from its own file, and the same
        // service is the library backfill below. Built before the torrent engine because the
        // episodes that pass repairs were downloaded in an earlier session, so a launch whose
        // engine cannot start still fills them.
        let stills = crate::episode_stills::FfmpegCapture::resolve().map(|capture| {
            Arc::new(crate::episode_stills::EpisodeStills::new(
                Arc::clone(&artwork),
                Arc::clone(&database),
                Arc::clone(&clock),
                Arc::new(capture),
            ))
        });

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
                            let mut service = AcquisitionService::new(
                                Arc::clone(&database),
                                Arc::clone(&torrent),
                                Arc::clone(&clock),
                                destination,
                                Arc::new(SystemVolume),
                                FREE_SPACE_FLOOR,
                                Arc::clone(&bus),
                                Arc::new(SystemTrash),
                            )
                            .with_notices(Arc::clone(&download_notices))
                            .with_usenet(Arc::new(peregrine_usenet::NntpEngine::new()))
                            .with_metadata_fetch(Arc::clone(&fetch));
                            if let Some(tools) = crate::usenet_post::PostTools::resolve() {
                                service = service.with_postprocess(tools);
                            }
                            // Without an ffmpeg on this host the import is unchanged and the
                            // backfill below is not started.
                            if let Some(stills) = &stills {
                                service = service.with_stills(Arc::clone(stills));
                            }
                            let service = Arc::new(service);
                            // The engine is in-process, so a restart loses every live handle. The
                            // rows and their selected files are enough to re-add each torrent with
                            // reattach semantics; the engine verifies the bytes already on disk.
                            match service.reattach_torrents().await {
                                Ok(0) => {}
                                Ok(count) => info!(count, "torrent jobs reattached"),
                                Err(error) => {
                                    warn!(%error, "torrent jobs could not be reattached");
                                }
                            }
                            if let Ok(servers) =
                                crate::usenet::server_configs(&database, Arc::clone(&secrets)).await
                            {
                                match service.reattach_usenet(servers).await {
                                    Ok(0) => {}
                                    Ok(count) => info!(count, "Usenet jobs reattached"),
                                    Err(error) => {
                                        warn!(%error, "Usenet jobs could not be reattached");
                                    }
                                }
                            }
                            match service.sweep_orphan_nzbs().await {
                                Ok(0) => {}
                                Ok(count) => info!(count, "orphan NZBs swept"),
                                Err(error) => warn!(%error, "orphan NZBs could not be swept"),
                            }
                            // **A job interrupted between its report and the engine's acceptance
                            // is left `metadata`, and nothing else moves it.** Its recovery may have
                            // to wait for a magnet's metadata, so it runs after startup rather than
                            // in front of it; the queue polls the row and draws the state change.
                            let recovery = Arc::clone(&service);
                            tokio::spawn(async move {
                                match recovery.recover_metadata_jobs().await {
                                    Ok(0) => {}
                                    Ok(count) => info!(count, "metadata downloads resumed"),
                                    Err(error) => {
                                        warn!(%error, "metadata downloads could not be resumed");
                                    }
                                }
                            });
                            torrent_engine = Some(torrent);
                            metadata_fetch = Some(fetch);
                            acquisition = Some(service);
                        }
                    }
                }
            },
        }

        let prowlarr = Arc::new(crate::jobs::prowlarr::Managed::discover(
            config.data_directory(),
        ));

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
            notification_service,
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

        // After the runtime is ready and after the downloads a previous process left are
        // reconciled, a library that predates the capture fallback fills a few stills per launch.
        // The pass is spawned rather than awaited so the launch does not wait for ffmpeg, and it
        // spaces its attempts so it cannot run the machine flat out.
        if let Some(stills) = stills {
            tokio::spawn(async move {
                match stills
                    .backfill(STILL_BACKFILL_ATTEMPTS, STILL_BACKFILL_INTERVAL)
                    .await
                {
                    Ok(0) => {}
                    Ok(count) => info!(count, "episode stills backfilled from their files"),
                    Err(error) => {
                        warn!(%error, "the episode still backfill stopped");
                    }
                }
            });
        }

        Ok(runtime)
    }

    /// The configuration this runtime was built with.
    #[must_use]
    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }

    /// Writes a backup of this installation's data into the chosen folder.
    ///
    /// The destination is a folder the caller already chose in an OS dialog. It is canonicalized
    /// and checked for being a directory here rather than trusted from the webview (SECURITY.md),
    /// and the file name is generated so no part of the user's text becomes a path component.
    ///
    /// # Errors
    ///
    /// `RuntimeError::InvalidRequest` when the destination is not a directory, and
    /// `RuntimeError::Storage` when the copy cannot be written.
    pub async fn create_backup(
        &self,
        directory: &Path,
    ) -> Result<peregrine_storage::BackupReport, RuntimeError> {
        self.create_backup_with_progress(directory, &|_| {}).await
    }

    /// Writes a backup, reporting each step to progress.
    ///
    /// The callback is called on the thread doing the copy, a handful of times per backup:
    /// the work is phase-based because SQLite does not report pages written, and a sender
    /// that emitted per frame would spend more time on the UI than the copy does.
    ///
    /// # Errors
    ///
    /// The same failures as `Self::create_backup`.
    pub async fn create_backup_with_progress(
        &self,
        directory: &Path,
        progress: &(dyn Fn(peregrine_storage::BackupPhase) + Send + Sync),
    ) -> Result<peregrine_storage::BackupReport, RuntimeError> {
        let directory =
            tokio::fs::canonicalize(directory)
                .await
                .map_err(|_| RuntimeError::InvalidRequest {
                    detail: "That folder could not be found.".to_owned(),
                })?;
        if !tokio::fs::metadata(&directory)
            .await
            .map_err(|_| RuntimeError::InvalidRequest {
                detail: "That folder could not be read.".to_owned(),
            })?
            .is_dir()
        {
            return Err(RuntimeError::InvalidRequest {
                detail: "Choose a folder, not a file.".to_owned(),
            });
        }
        let name = crate::backup::backup_file_name(self.clock.now().as_millisecond());
        let destination = crate::backup::free_destination(&directory, &name).await?;
        self.database
            .write_backup(&destination, progress)
            .await
            .map_err(|error| {
                // The IPC payload deliberately says only that the copy failed. The typed error
                // names the file and the driver's reason, which is what makes a support report
                // actionable without putting a path in the webview.
                warn!(
                    %error,
                    backup = %destination.display(),
                    "the backup could not be written"
                );
                RuntimeError::Storage(error)
            })
    }

    /// Validates a chosen backup and stages it for the next launch.
    ///
    /// The running database is untouched: the command that calls this restarts the app, and the
    /// swap happens in `Runtime::start` before the file is opened. A file that is not a backup,
    /// or one written by a newer build, is refused here while the installation still works.
    ///
    /// # Errors
    ///
    /// `RuntimeError::InvalidRequest` when the file is not a Peregrine backup, and
    /// `RuntimeError::Storage` when the copy cannot be staged.
    pub async fn restore_backup(
        &self,
        source: &Path,
    ) -> Result<peregrine_storage::BackupInfo, RuntimeError> {
        self.restore_backup_with_progress(source, &|_| {}).await
    }

    /// Validates and stages a restore, reporting each step to progress.
    ///
    /// # Errors
    ///
    /// The same failures as `Self::restore_backup`.
    pub async fn restore_backup_with_progress(
        &self,
        source: &Path,
        progress: &(dyn Fn(crate::backup::RestorePhase) + Send + Sync),
    ) -> Result<peregrine_storage::BackupInfo, RuntimeError> {
        crate::backup::stage_restore(
            &self.database,
            self.config.data_directory(),
            &self.config.database_path(),
            source,
            progress,
        )
        .await
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
        // A live row owns the key, and `job_dedupe_uq` would refuse a second one with a bare
        // constraint error; joining it is what pressing Install twice means (see
        // `start_discover_rails`).
        if let Some(live) = self.live_job_for(kind).await? {
            return Ok(live);
        }
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
            Arc::clone(&self.clock),
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
        // The same join as the install: a live row holds the key, and the index would refuse the
        // second one with an error that names the database rather than the double press.
        if let Some(live) = self.live_job_for(kind).await? {
            return Ok(live);
        }
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
            Arc::clone(&self.clock),
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
                season: None,
                episode,
                chosen: None,
            },
            min_seeders,
        )
        .await
    }

    /// Finds one season's releases and queues a download for it, as a job.
    ///
    /// The job searches `Name Sxx` once and takes the side — a season pack or a single episode of
    /// that season — whose best candidate has the most seeders; a pack's selection keeps only the
    /// entries that name the season.
    ///
    /// A second request while the season is already being acquired joins the journalled job rather
    /// than starting a second one, so pressing the control twice is one season's work.
    ///
    /// # Errors
    ///
    /// The same storage, scheduler and engine errors that `start_acquire` reports.
    pub async fn start_acquire_season(
        &self,
        account: peregrine_domain::AccountScope,
        title_id: i64,
        season: u32,
        min_seeders: u32,
    ) -> Result<StartedJob, RuntimeError> {
        self.submit_acquire(
            account,
            crate::jobs::acquire::Target {
                title_id,
                season: Some(season),
                episode: None,
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
        // New rows carry the tagged source. Rows written before the protocol existed carry a bare
        // `TorrentSource`, which is still a usable answer.
        let source: peregrine_indexers::ReleaseSource = serde_json::from_str(&chosen.source_json)
            .or_else(|_| {
                serde_json::from_str::<peregrine_indexers::TorrentSource>(&chosen.source_json)
                    .map(peregrine_indexers::ReleaseSource::Torrent)
            })
            .map_err(|_| RuntimeError::InvalidRequest {
                detail: "that offer's release is not usable".to_owned(),
            })?;
        self.submit_acquire(
            account,
            crate::jobs::acquire::Target {
                title_id: chosen.title_id,
                season: None,
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
    ///
    /// **A request for work already in flight joins that work.** The journal's `job_dedupe_uq` is
    /// a partial unique index over the non-terminal phases, and `jobs::insert` reports every driver
    /// failure as a statement error — so a blind insert while a season's fan-out was running came
    /// back as "The library database rejected a statement." for what is only a second press. The
    /// live row is the job the person asked for, so it is the answer; `start_prowlarr` and
    /// `start_discover_rails` join the same way.
    async fn submit_acquire(
        &self,
        account: peregrine_domain::AccountScope,
        target: crate::jobs::acquire::Target,
        min_seeders: u32,
    ) -> Result<StartedJob, RuntimeError> {
        let title_id = target.title_id;
        let key = crate::jobs::acquire::dedupe_key(title_id, target.season, target.episode);
        // A chosen release is not a search: a live title-wide search does not do the work a person
        // picked from the offers, so only a search request joins. Its key is the same because the
        // offer names no season or episode of its own.
        if target.chosen.is_none()
            && let Some(live) = self.live_job_for(&key).await?
        {
            return Ok(live);
        }
        // The account's preference is read once per submit, so the job orders the candidates it
        // finds by the language the person asked the host to prefer.
        let preferred_audio_language = self
            .playback_preference()
            .await?
            .audio_language()
            .to_owned();
        let now = self.clock.now().as_millisecond();
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
            Arc::new(crate::jobs::acquire::ConfiguredReleaseIndexer::new(
                self.secret_store(),
            )),
            self.metadata_fetch()?,
            self.acquisition()?,
            Arc::clone(&self.clock),
            account,
            journal_id,
            target,
            min_seeders,
            preferred_audio_language,
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

    /// Whether a job's payload has named a selected media file the player could stream.
    ///
    /// The question a Play control asks before it is drawn: metadata that has not arrived names no
    /// entries, and a payload whose every entry is refused or unselected names no media. It is the
    /// same predicate `choose_download_file` reads — a file the payload has, not one on disk — which
    /// is why a downloading title and a downloaded one can offer the same action.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the file rows cannot be read.
    pub async fn download_has_media(&self, id: i64) -> Result<bool, RuntimeError> {
        let files = self
            .database
            .download_files(installation_scope(), id)
            .await?;
        Ok(files.iter().any(|file| file.selected && file.is_media))
    }

    /// The account's playback language preference.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Storage`] when the row cannot be read.
    pub async fn playback_preference(&self) -> Result<PlaybackPreference, RuntimeError> {
        Ok(self.database.playback_preference().await?)
    }

    /// Stores the account's playback language preference.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Storage`] when the row cannot be written.
    pub async fn set_playback_preference(
        &self,
        preference: &PlaybackPreference,
    ) -> Result<(), RuntimeError> {
        self.database
            .set_playback_preference(preference, self.clock.now())
            .await?;
        Ok(())
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
                // **The opened title is not its own recommendation.** The adapter drops the id it was
                // asked about, but an answer can reach the same local title through another id — a
                // duplicate provider entry, or a name the matcher merged — and the rail is drawn by
                // local id. Filtering here is the one place both routes meet.
                if item.title_id == title_id {
                    continue;
                }
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
        // Availability shares the one refresh the screen already follows: a title nobody can be
        // asked about has no answer to fetch, and a title fetched inside the window is not asked
        // again just because its details are complete.
        let watch_due = state.answerable
            && match self.database.watch_synced_at(id).await? {
                None => true,
                Some(at) => {
                    self.clock.now().as_millisecond().saturating_sub(at)
                        >= WATCH_AVAILABILITY_TTL_MS
                }
            };
        let playback = self.title_playback(id).await?;
        Ok(Some(LibraryTitleDetail {
            refresh_due: crate::artwork::refresh_due(&state) || watch_due,
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

    /// The LAN surface over this runtime's own reads, for the shell to start and stop.
    ///
    /// The service holds the runtime, not the other way round: it is built by the composition root
    /// (the shell) once the runtime exists, and nothing in the core starts a listener on its own.
    #[must_use]
    pub fn web_service(self: &Arc<Self>) -> crate::web::WebService {
        crate::web::WebService::new(
            Arc::clone(self),
            Arc::clone(&self.database),
            Arc::clone(&self.bus),
            Arc::clone(&self.artwork),
            Arc::clone(&self.clock),
            Arc::clone(&self.tokens),
            self.config.data_directory().join("hls"),
        )
    }

    /// The system-sleep assertion this runtime was built with (PG-E1-17).
    pub(crate) fn power(&self) -> Arc<dyn peregrine_platform::PowerSave> {
        Arc::clone(&self.config.power)
    }

    /// One catalogue title's identity, for a page reached from a discovery rail.
    ///
    /// The read a non-library title needs: a rail names a catalogue row the library does not hold,
    /// and opening it should say what it is and that no copy is here rather than answering 404.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Storage`] when the read fails.
    pub async fn title_identity(
        &self,
        id: i64,
    ) -> Result<Option<peregrine_storage::TitleIdentity>, RuntimeError> {
        Ok(self
            .database
            .title_identity(installation_scope(), id)
            .await?)
    }

    /// The episode row id for one season and episode number, when the library holds it.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Storage`] when the read fails.
    pub async fn episode_id(
        &self,
        title_id: i64,
        season_number: u32,
        episode_number: u32,
    ) -> Result<Option<i64>, RuntimeError> {
        Ok(self
            .database
            .episode_id_for(title_id, season_number, episode_number)
            .await?)
    }

    /// The file a LAN client may play directly, when the library holds one.
    ///
    /// `None` is "the title has no copy on disk", which is an answer rather than a failure; a
    /// container a browser cannot demux is returned so the caller can name the refusal instead of
    /// handing the element bytes it will reject with an error code and no reason.
    ///
    /// The path is a [`SafePath`], so the check that it lives under a configured root happened
    /// once, in [`Runtime::playable`], and the server never assembles a path of its own.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::UnknownTitle`], [`RuntimeError::FileOutsideRoots`], or
    /// [`RuntimeError::Storage`].
    pub async fn web_playback(
        &self,
        title_id: i64,
        episode_id: Option<i64>,
    ) -> Result<Option<crate::web::WebPlaybackTarget>, RuntimeError> {
        match self.playable(title_id, episode_id).await {
            Ok((file, path)) => {
                // The episode the chosen file belongs to is what progress is keyed by: a series'
                // resume row is per episode, and a title-level row would resume the wrong one.
                let evidence = self
                    .database
                    .title_evidence(installation_scope(), title_id)
                    .await?;
                let episode_id = evidence
                    .items
                    .iter()
                    .find(|item| item.files.iter().any(|held| held.id == file.id))
                    .and_then(|item| item.item.episode_id);
                Ok(Some(crate::web::WebPlaybackTarget {
                    source: crate::web::WebSource::File(path.as_path().to_path_buf()),
                    size_bytes: file.size_bytes,
                    playback: peregrine_domain::LocalPlayback::of_name(file.path.as_str()),
                    episode_id,
                    duration_ms: file
                        .duration_ms
                        .and_then(|duration| i64::try_from(duration).ok()),
                }))
            }
            // A title with nothing on disk but a job in flight is served from the engine, exactly as
            // the desktop is. An episode is selected by its job's identity, so a show's season pack
            // does not answer for the wrong episode.
            Err(RuntimeError::NoPlayableFile) => {
                match self.torrent_media(title_id, episode_id).await? {
                    Some(source) => Ok(Some(crate::web::WebPlaybackTarget {
                        size_bytes: source.media.len(),
                        source: crate::web::WebSource::Torrent(source.media),
                        playback: source.support,
                        episode_id,
                        // The metadata names no duration; a probe over a live stream would have to
                        // read enough of it to demux, and the client tolerates a growing playlist.
                        duration_ms: None,
                    })),
                    None => Ok(None),
                }
            }
            Err(error) => Err(error),
        }
    }

    /// Where this title's playable file was left, in the same store the desktop player writes.
    ///
    /// `None` when the title has no file or nobody has watched it yet; the two are different on
    /// screen only in that one has a Resume control and the other starts from the beginning.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Storage`] when the read fails.
    pub async fn web_progress(
        &self,
        title_id: i64,
    ) -> Result<Option<crate::web::WebProgress>, RuntimeError> {
        let Some(target) = self.web_playback(title_id, None).await? else {
            return Ok(None);
        };
        let Some(progress) = self
            .database
            .watch_progress(installation_scope(), title_id, target.episode_id)
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(crate::web::WebProgress {
            episode_id: target.episode_id,
            position_ms: progress.position_ms(),
            duration_ms: progress.duration_ms(),
            percent: progress.percent(),
            watched: progress.watched(),
        }))
    }

    /// The sidecar subtitle tracks a playback session may offer for one title or episode.
    ///
    /// Metadata only: the path a sidecar lives at never leaves the core (AGENTS R11), and the
    /// route that serves it resolves the same track through [`Runtime::web_subtitle_path`]. A
    /// title with no playable file has no tracks to offer, which is an empty list rather than a
    /// failure: the player draws no subtitle menu for it.
    pub async fn web_subtitles(
        &self,
        title_id: i64,
        episode_id: Option<i64>,
    ) -> Vec<crate::web::WebSubtitleTrack> {
        let Ok((file, _)) = self.playable(title_id, episode_id).await else {
            return Vec::new();
        };
        let Ok(tracks) = self.playable_subtitles(file.id).await else {
            return Vec::new();
        };
        tracks
            .into_iter()
            .map(|(track, _)| crate::web::WebSubtitleTrack {
                id: track.id,
                language: track.language.as_str().to_owned(),
                label: track.label,
                format: track.format.as_str().to_owned(),
                forced: track.is_forced,
                sdh: track.is_sdh,
                is_default: track.is_default,
            })
            .collect()
    }

    /// The guarded sidecar and its container for one track of one title or episode.
    ///
    /// The title and episode are resolved to the file the core would play, and the track is looked
    /// up among that file's own sidecars. A track id from another file therefore resolves to
    /// `None` rather than to a path, which is what keeps a client from naming a file at all.
    pub async fn web_subtitle_path(
        &self,
        title_id: i64,
        episode_id: Option<i64>,
        track_id: i64,
    ) -> Option<(SafePath, SubtitleFormat)> {
        let (file, _) = self.playable(title_id, episode_id).await.ok()?;
        let tracks = self.playable_subtitles(file.id).await.ok()?;
        tracks
            .into_iter()
            .find(|(track, _)| track.id == track_id)
            .map(|(track, path)| (path, track.format))
    }

    /// The playable episode that follows one on the same title.
    ///
    /// "Next" is the next row in the same season with a file on disk, else the first such row of
    /// a later season. A film, the last episode, and a later season nobody has acquired all answer
    /// `None`, which the player draws as no autoplay.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when a read fails.
    pub async fn web_next_episode(
        &self,
        title_id: i64,
        episode_id: Option<i64>,
    ) -> Result<Option<crate::web::NextEpisodeDto>, RuntimeError> {
        let Some(episode_id) = episode_id else {
            return Ok(None);
        };
        let Some(identity) = self.title_identity(title_id).await? else {
            return Ok(None);
        };
        if identity.kind != peregrine_domain::catalogue::TitleKind::Show {
            return Ok(None);
        }
        let Some((season, number)) = self.episode_numbers(episode_id).await? else {
            return Ok(None);
        };
        let same_season = self
            .database
            .list_title_episodes(installation_scope(), title_id, Some(season), 500)
            .await?;
        let row = if let Some(row) = same_season
            .into_iter()
            .find(|row| row.episode_number.get() > number && row.files > 0)
        {
            row
        } else {
            let later = self
                .database
                .list_title_episodes(installation_scope(), title_id, None, 1000)
                .await?;
            let Some(row) = later
                .into_iter()
                .find(|row| row.season_number.get() > season && row.files > 0)
            else {
                return Ok(None);
            };
            row
        };
        let season_number = row.season_number.get();
        let episode_number = row.episode_number.get();
        let Some(episode_id) = self
            .episode_id(title_id, season_number, episode_number)
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(crate::web::NextEpisodeDto {
            season_number,
            episode_number,
            episode_id,
            name: row.name,
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
        let offer = peregrine_domain::TitleOffer::of(&items, &evidence.jobs);
        let support = match playable_file(&evidence, None) {
            Some(file) => Some(LocalPlayback::of_name(file.path.as_str())),
            // A film whose transfer is running has no file on disk yet, and the page still has to
            // know which reader the Play will use before it is pressed. The job's own media entry
            // answers it. An episode of a show has no title-level entry, so this stays `None` and the
            // hero keeps its disabled Play: the episode card is where that episode is started.
            None if offer == peregrine_domain::TitleOffer::Arriving => self
                .torrent_media(title_id, None)
                .await?
                .map(|source| source.support),
            None => None,
        };
        Ok(TitlePlayback { offer, support })
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
        //
        // **The provider's still wins, and a captured one stands behind it.** The captured frame is
        // the fallback for an episode no provider has drawn, so a late TMDB still replaces it the
        // moment its blob is cached; until then the card still has a picture. The episode id is the
        // second key because a capture is not a provider's path, and the episode row does not carry
        // one for it.
        Ok(rows
            .into_iter()
            .map(|row| {
                let still = row
                    .still
                    .as_ref()
                    .and_then(|artwork| {
                        crate::artwork::cached_image(
                            &self.artwork,
                            artwork.provider,
                            &artwork.path,
                            peregrine_domain::catalogue::ImageKind::Still,
                        )
                    })
                    .or_else(|| crate::episode_stills::cached(&self.artwork, row.id));
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

    /// Whether the active profile may start a download.
    ///
    /// A child profile's row says no, and the command and the web route refuse before the queue
    /// is touched. The UI hides the button too, but this is the gate that makes the refusal true.
    #[must_use]
    pub fn can_acquire(&self) -> bool {
        ACTIVE_CAN_ACQUIRE.load(Ordering::Relaxed)
    }

    /// Every profile, with the default first.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Storage`] when the database refuses.
    pub async fn profiles(&self) -> Result<Vec<peregrine_storage::AccountRow>, RuntimeError> {
        Ok(self.database.profiles().await?)
    }

    /// The profile the installation is watching as.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Storage`] when the database refuses.
    pub async fn active_profile(
        &self,
    ) -> Result<Option<peregrine_storage::AccountRow>, RuntimeError> {
        Ok(self.database.active_profile().await?)
    }

    /// Creates a profile and makes it the active one.
    ///
    /// The child kind carries the two fields the schema requires of it: a ceiling and no
    /// acquisition. The full rating map and the OS gate are E35's; the restricted scope already
    /// hides adult and unrated content.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::InvalidRequest`] for a blank or overlong name and an unknown kind, and
    /// [`RuntimeError::Storage`] when the row cannot be written.
    pub async fn create_profile(
        &self,
        name: &str,
        kind: &str,
        avatar_ref: Option<&str>,
    ) -> Result<peregrine_storage::AccountRow, RuntimeError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 40 {
            return Err(RuntimeError::InvalidRequest {
                detail: "a profile name is between 1 and 40 characters".to_owned(),
            });
        }
        let child = match kind {
            "child" => true,
            "adult" | "guest" => false,
            _ => {
                return Err(RuntimeError::InvalidRequest {
                    detail: "a profile is adult, child, or guest".to_owned(),
                });
            }
        };
        let id = self
            .database
            .create_profile(peregrine_storage::NewAccount {
                name: name.to_owned(),
                kind: kind.to_owned(),
                avatar_kind: if avatar_ref.is_some() {
                    "custom"
                } else {
                    "initials"
                }
                .to_owned(),
                avatar_ref: avatar_ref.map(ToOwned::to_owned),
                rating_ceiling: child.then(|| "PG-13".to_owned()),
                curated_only: false,
                can_acquire: !child && kind != "guest",
                created_at: self.clock.now().as_millisecond(),
            })
            .await?;
        self.select_profile(id).await
    }

    /// Writes one profile's name and avatar.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::InvalidRequest`] for a blank or overlong name, and
    /// [`RuntimeError::Storage`] when the row cannot be written.
    pub async fn update_profile(
        &self,
        id: i64,
        name: &str,
        avatar_ref: Option<&str>,
    ) -> Result<peregrine_storage::AccountRow, RuntimeError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 40 {
            return Err(RuntimeError::InvalidRequest {
                detail: "a profile name is between 1 and 40 characters".to_owned(),
            });
        }
        if !self
            .database
            .update_profile(
                id,
                peregrine_storage::AccountEdit {
                    name: name.to_owned(),
                    avatar_kind: if avatar_ref.is_some() {
                        "custom"
                    } else {
                        "initials"
                    }
                    .to_owned(),
                    avatar_ref: avatar_ref.map(ToOwned::to_owned),
                },
            )
            .await?
        {
            return Err(RuntimeError::InvalidRequest {
                detail: "that profile does not exist".to_owned(),
            });
        }
        let Some(account) = self.database.profile(id).await? else {
            return Err(RuntimeError::InvalidRequest {
                detail: "that profile does not exist".to_owned(),
            });
        };
        if self
            .active_profile()
            .await?
            .is_some_and(|active| active.id == id)
        {
            activate(&account);
        }
        Ok(account)
    }

    /// Makes one profile the active scope and persists the choice.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::InvalidRequest`] when no such profile exists, and
    /// [`RuntimeError::Storage`] when the setting cannot be written.
    pub async fn select_profile(
        &self,
        id: i64,
    ) -> Result<peregrine_storage::AccountRow, RuntimeError> {
        let Some(account) = self.database.profile(id).await? else {
            return Err(RuntimeError::InvalidRequest {
                detail: "that profile does not exist".to_owned(),
            });
        };
        self.database
            .set_active_profile(account.id, self.clock.now().as_millisecond())
            .await?;
        activate(&account);
        Ok(account)
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

    /// The sidecar subtitles a playback session may serve for one file, with their metadata.
    ///
    /// Only a row with a path is offered: an embedded or generated track has no sidecar file to serve,
    /// and extraction is not this slice's. A path outside every configured root is refused track by
    /// track rather than failing the play — a stale or tampered row must not take a film down with it —
    /// and the refusal is logged without the path (SECURITY.md T13).
    async fn playable_subtitles(
        &self,
        media_file_id: i64,
    ) -> Result<Vec<(SubtitleTrack, SafePath)>, RuntimeError> {
        if media_file_id == 0 {
            return Ok(Vec::new());
        }
        let rows = self
            .database
            .list_subtitle_tracks(
                installation_scope(),
                media_file_id,
                PageRequest::first(MAX_SUBTITLE_TRACKS),
            )
            .await?;
        let mut playable = Vec::new();
        for row in rows {
            let Some(path) = row.track.path.as_deref() else {
                continue;
            };
            match self.guarded_media_path(Path::new(path)).await {
                Ok(safe) => playable.push((row.track, safe)),
                Err(RuntimeError::FileOutsideRoots) => {
                    warn!(
                        track = row.track.id,
                        "a subtitle sidecar outside every configured root was not offered"
                    );
                }
                Err(error) => return Err(error),
            }
        }
        Ok(playable)
    }

    /// Opens a session for a library file and answers the sidecar tracks it can serve.
    ///
    /// The sidecars are read before the listener opens, so the session is registered with the exact set
    /// it may serve: a request cannot name a track the session was not opened with.
    async fn open_file_session(
        &self,
        media_file_id: i64,
        title_id: i64,
        file: &SafePath,
    ) -> Result<(peregrine_playback::PlaybackSession, Vec<PlaybackSubtitle>), RuntimeError> {
        let tracks = self.playable_subtitles(media_file_id).await?;
        let sidecars = tracks
            .iter()
            .map(|(track, path)| peregrine_stream::SidecarSubtitle {
                track_id: track.id,
                path: path.clone(),
                content_type: subtitle_content_type(track.format),
            })
            .collect();
        let session = peregrine_playback::PlaybackSession::open(
            media_file_id,
            title_id,
            file,
            self.tokens.next_token(),
            sidecars,
            Some(Arc::clone(&self.posters) as Arc<dyn peregrine_stream::ThumbnailSink>),
        )
        .await
        .map_err(playback_error)?;
        let subtitles = tracks
            .into_iter()
            .map(|(track, _)| PlaybackSubtitle {
                id: track.id,
                language: track.language.as_str().to_owned(),
                label: track.label,
                format: track.format.as_str().to_owned(),
                forced: track.is_forced,
                sdh: track.is_sdh,
                default: track.is_default,
                url: session.subtitle_url(track.id),
            })
            .collect();
        Ok((session, subtitles))
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
            Vec::new(),
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

    /// The engine-backed source for a title or one of its episodes, when a live job holds it.
    ///
    /// **The job is the selector, and the episode is part of its identity.** `download_job.episode_id`
    /// is written by the acquire job for the episode the person asked for, so a show with two
    /// episodes in flight has two rows rather than one ambiguous set. A film asks for the row with no
    /// episode, which is what keeps a title-level Play from serving an episode of a show.
    ///
    /// `None` covers every reason there is nothing to stream — no engine, no live handle for the job
    /// (a job another process started has none here), metadata that has not named a media entry yet,
    /// or no job for this title and episode. The caller turns `None` into the same refusal a title
    /// with nothing on disk gets.
    async fn torrent_media(
        &self,
        title_id: i64,
        episode_id: Option<i64>,
    ) -> Result<Option<TorrentSource>, RuntimeError> {
        let (Ok(engine), Ok(acquisition)) = (self.torrent_engine(), self.acquisition()) else {
            return Ok(None);
        };
        let scope = installation_scope();
        // **A show has no title-level stream.** Its files are episodes, and playing one without an
        // episode identity would pick an arbitrary entry, record progress against the show, and give
        // the reader no way to say which episode they meant. The episode card is that surface; the
        // hero's Play stays refused until the core is asked for an episode.
        if episode_id.is_none()
            && self
                .database
                .library_title(scope, title_id)
                .await?
                .is_some_and(|row| {
                    row.library.title.kind == peregrine_domain::catalogue::TitleKind::Show
                })
        {
            return Ok(None);
        }
        let jobs = self
            .database
            .list_download_jobs(
                scope,
                peregrine_domain::DownloadState::ALL,
                peregrine_storage::PageRequest::first(50),
            )
            .await?;
        let Some(job) = jobs
            .into_iter()
            .find(|job| job.title_id == title_id && job.episode_id == episode_id)
        else {
            return Ok(None);
        };
        let Some(handle) = acquisition.handle_for(job.id) else {
            return Ok(None);
        };
        let files = self.database.download_files(scope, job.id).await?;
        let episode = match episode_id {
            Some(id) => self.database.episode_numbers(id).await?,
            None => None,
        };
        let index = choose_download_file(&files, episode).map_err(|reason| match reason {
            FileChoice::NotReady => RuntimeError::NoPlayableFile,
            FileChoice::Ambiguous => RuntimeError::MediaUnidentified,
        })?;
        let file = files.get(index).ok_or(RuntimeError::NoPlayableFile)?;
        let torrent_index = usize::try_from(file.file_index).unwrap_or_default();
        Ok(Some(TorrentSource {
            media: Arc::new(crate::media_source::EngineMedia::new(
                engine,
                handle,
                torrent_index,
                file.size_bytes,
                file.path.as_str(),
            )),
            support: LocalPlayback::of_name(file.path.as_str()),
        }))
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
    /// what lets a film or a named episode be watched before its last pieces arrive.
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

        let (media_file_id, support, needs_poster, session, subtitles) =
            match self.playable(title_id, episode_id).await {
                Ok((file, safe)) => {
                    let support = LocalPlayback::of_name(file.path.as_str());
                    // Whether a poster is worth capturing on the way, asked from the same read the play
                    // needs: only when there is nothing to draw, which is the rule that stops a film still
                    // replacing real artwork.
                    let needs_poster = self.needs_poster(title_id).await?;
                    let media_file_id = file.id;
                    let (session, subtitles) = self
                        .open_file_session(media_file_id, title_id, &safe)
                        .await?;
                    (media_file_id, support, needs_poster, session, subtitles)
                }
                // A title with nothing on disk may still be downloading: serve it from the engine. The
                // same arm answers an episode, because the job row names the episode and the payload's
                // file is chosen by that identity (see `torrent_media`).
                Err(RuntimeError::NoPlayableFile) => {
                    match self.torrent_media(title_id, episode_id).await? {
                        Some(source) => {
                            let session = peregrine_playback::PlaybackSession::open_torrent(
                                title_id,
                                source.media,
                                self.tokens.next_token(),
                                Some(Arc::clone(&self.posters)
                                    as Arc<dyn peregrine_stream::ThumbnailSink>),
                            )
                            .await
                            .map_err(playback_error)?;
                            (0, source.support, false, session, Vec::new())
                        }
                        None => return Err(RuntimeError::NoPlayableFile),
                    }
                }
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
            .watch_progress(installation_scope(), title_id, episode_id)
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
            subtitles,
            needs_poster,
            resume_position_ms,
        })
    }

    /// Plays one of a title's files — or its download — in mpv, for a container the webview cannot
    /// demux.
    ///
    /// The title, episode, and source resolution is [`Runtime::start_playback`]'s. A library file is
    /// opened by mpv itself; a title whose copy is still downloading is served by this process over
    /// loopback, and mpv is handed that session's URL. Whatever the webview was serving is stopped
    /// first, because two players for one press is one too many.
    ///
    /// # Errors
    ///
    /// The same refusals as [`Runtime::start_playback`], plus [`RuntimeError::Player`] when mpv is
    /// not installed or the operating system would not start it.
    #[allow(clippy::too_many_lines)] // One launch: resolve, open, report, and spawn, in that order.
    pub async fn start_external_playback(
        &self,
        title_id: i64,
        episode: Option<crate::jobs::acquire::EpisodePick>,
    ) -> Result<(), RuntimeError> {
        let opening = *self
            .session_id
            .lock()
            .expect("the session id lock is not poisoned");
        if let Some(held) = opening {
            self.stop_playback(held).await;
        }

        // Resolve the episode before the file: an external play opened from an episode card must
        // hand mpv that episode's file, not the first file the title happens to hold.
        let episode_id = match episode {
            Some(pick) => {
                self.database
                    .episode_id_for(title_id, pick.season, pick.episode)
                    .await?
            }
            None => None,
        };
        // A file the library holds, or the engine when it holds none and the title is downloading.
        // The stream case opens the loopback listener *before* mpv starts: mpv is handed the URL, not
        // a path, and the listener must already be answering when it connects.
        let (file, subtitle, stream) = match self.playable(title_id, episode_id).await {
            Ok((file, safe)) => {
                // The sidecar the webview would have offered, chosen by the same default flag. A
                // sidecar that cannot be read is not allowed to take the film down with it.
                let subtitle = match self.playable_subtitles(file.id).await {
                    Ok(tracks) => tracks
                        .iter()
                        .find(|(track, _)| track.is_default)
                        .or_else(|| tracks.first())
                        .map(|(_, path)| path.clone()),
                    Err(error) => {
                        warn!(%error, title = title_id, "the selected subtitle could not be offered to mpv");
                        None
                    }
                };
                (Some(safe), subtitle, None)
            }
            Err(RuntimeError::NoPlayableFile) => {
                let Some(source) = self.torrent_media(title_id, episode_id).await? else {
                    return Err(RuntimeError::NoPlayableFile);
                };
                let session = peregrine_playback::PlaybackSession::open_torrent(
                    title_id,
                    source.media,
                    self.tokens.next_token(),
                    // No poster capture: mpv owns the surface, and the renderer that posts a frame
                    // is the webview's.
                    None,
                )
                .await
                .map_err(playback_error)?;
                let url = LoopbackUrl::parse(session.url()).ok_or_else(|| {
                    RuntimeError::InvalidRequest {
                        detail: "the playback listener did not answer with a loopback URL"
                            .to_owned(),
                    }
                })?;
                // The session lives in the runtime for as long as the player may ask for bytes. The
                // next play, a stop, or the process ending closes it.
                let session_id = self.mint_session();
                *self
                    .session
                    .lock()
                    .expect("the session lock is not poisoned") = Some(session);
                *self
                    .session_id
                    .lock()
                    .expect("the session id lock is not poisoned") = Some(session_id);
                (None, None, Some((url, session_id)))
            }
            Err(error) => return Err(error),
        };
        // The app's own watch progress is the resume rule; mpv's watch_later is disabled.
        let resume_ms = self
            .database
            .watch_progress(installation_scope(), title_id, episode_id)
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
                if let Err(error) = write_reported_position(
                    &database,
                    clock.as_ref(),
                    title_id,
                    episode_id,
                    position,
                )
                .await
                {
                    warn!(%error, title = title_id, "an external player position could not be recorded");
                }
            }
        });

        let (stream_url, stored) = match stream {
            Some((url, id)) => (Some(url), Some(id)),
            None => (None, None),
        };
        let player = Arc::clone(&self.external_player);
        let positions: Arc<dyn peregrine_domain::PositionSink> = Arc::new(ChannelSink(sender));
        let launch = tokio::task::spawn_blocking(move || {
            let input = match (&file, &stream_url) {
                (Some(path), _) => PlayerInput::File(path),
                (_, Some(url)) => PlayerInput::Stream(url),
                // The match above guarantees one of the two; a caller that broke it gets a spawn
                // refusal rather than a panic in a player thread.
                (None, None) => return Err(peregrine_domain::PlayerError::Spawn),
            };
            player.play(input, resume_ms, subtitle.as_ref(), positions)
        })
        .await;
        let launch = match launch {
            Ok(result) => result,
            Err(_) => Err(peregrine_domain::PlayerError::Spawn),
        };
        if let (Err(_), Some(session_id)) = (&launch, stored) {
            // A player that never started must not leave a listener behind for the next press to
            // replace: close the one this attempt opened.
            self.stop_playback(session_id).await;
        }
        launch.map_err(RuntimeError::from)?;
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

    /// Where a title's watch state leaves the reader: what they last touched and what comes next.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the read fails.
    pub async fn title_watch_state(
        &self,
        title_id: i64,
    ) -> Result<peregrine_storage::TitleWatchState, RuntimeError> {
        Ok(self
            .database
            .title_watch_state(installation_scope(), title_id)
            .await?)
    }

    /// The titles this account has started and not finished, most recent first.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the read fails.
    pub async fn continue_watching(&self) -> Result<Vec<WatchProgressRow>, RuntimeError> {
        let rows = self
            .database
            .list_continue_watching(installation_scope(), PageRequest::first(20))
            .await?;
        // Continue Watching is one card per title: a show with two episodes in progress is one
        // thing the reader is watching, and the list is already most-recent first, so the first
        // row for a title is the episode to resume.
        let mut seen = std::collections::HashSet::new();
        Ok(rows
            .into_iter()
            .filter(|row| seen.insert(row.progress.title_id()))
            .collect())
    }

    /// The address the internet sees, read only when the reader asks for it.
    ///
    /// **One request, and only from a click.** This is the check a person runs after starting a
    /// tunnel; it is not a heartbeat, it is not cached, and the answer is never logged (AGENTS R23).
    /// The echo service is ipify's documented JSON endpoint, which is the only third party this
    /// command contacts and only for this answer.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::PublicIpUnavailable`] when the request fails, answers non-success, or does
    /// not carry a valid address.
    pub async fn public_ip(&self) -> Result<String, RuntimeError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .user_agent(concat!("Peregrine/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| RuntimeError::PublicIpUnavailable)?;
        let response = client
            .get(PUBLIC_IP_ECHO)
            .send()
            .await
            .map_err(|_| RuntimeError::PublicIpUnavailable)?;
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|length| length > 4096)
        {
            return Err(RuntimeError::PublicIpUnavailable);
        }
        let body = response
            .text()
            .await
            .map_err(|_| RuntimeError::PublicIpUnavailable)?;
        if body.len() > 4096 {
            return Err(RuntimeError::PublicIpUnavailable);
        }
        public_ip_from_json(&body).ok_or(RuntimeError::PublicIpUnavailable)
    }

    /// Watched time per local day, optionally bounded by the row's last write.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the read fails.
    pub async fn watch_metrics(
        &self,
        from_ms: Option<i64>,
        to_ms: Option<i64>,
    ) -> Result<Vec<peregrine_storage::WatchDayRow>, RuntimeError> {
        Ok(self
            .database
            .watch_by_day(installation_scope(), from_ms, to_ms)
            .await?)
    }

    /// Removes the account's watch progress for one title, or all of it when no title is named.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the delete fails.
    pub async fn clear_history(&self, title_id: Option<i64>) -> Result<u32, RuntimeError> {
        let removed = self
            .database
            .clear_watch_progress(installation_scope().account(), title_id)
            .await?;
        Ok(u32::try_from(removed).unwrap_or(u32::MAX))
    }

    /// The account's viewing history, newest first, for the history screen.
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

    /// One person and the library titles that credit them.
    ///
    /// `None` when no person row holds the id, which a route naming a deleted credit reaches.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when either read fails.
    pub async fn person_page(
        &self,
        id: i64,
        limit: u32,
    ) -> Result<Option<PersonPage>, RuntimeError> {
        let Some(person) = self.database.person(id).await? else {
            return Ok(None);
        };
        let rows = self
            .database
            .library_titles_for_person(installation_scope(), id, PageRequest::first(limit))
            .await?;
        Ok(Some(PersonPage {
            name: person.name,
            profile: person.profile,
            titles: rows.into_iter().map(|row| self.listed_title(row)).collect(),
        }))
    }

    /// The person's credits beyond this library, as the configured provider knows them.
    ///
    /// Two provider pages, films and series, merged in that order and capped: a person's filmography
    /// is a catalogue read, not a library read, and the local titles are already the page's own grid.
    /// Empty when the person carries no provider id — a manual or future provider seam.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the person cannot be read, and the provider's own error when a
    /// page cannot be fetched.
    pub async fn person_filmography(
        &self,
        id: i64,
    ) -> Result<Vec<crate::jobs::discover_rails::RailItem>, RuntimeError> {
        let Some(person) = self.database.person(id).await? else {
            return Ok(Vec::new());
        };
        let Some(cast) = person.provider_id else {
            return Ok(Vec::new());
        };
        let mut items = Vec::new();
        for kind in [
            peregrine_domain::catalogue::TitleKind::Movie,
            peregrine_domain::catalogue::TitleKind::Show,
        ] {
            let request = peregrine_providers::BrowseRequest::new(
                kind,
                std::num::NonZeroU32::MIN,
                peregrine_providers::BrowseSort::Popularity,
                None,
                None,
            )
            .with_cast(Some(cast.clone()));
            items.extend(self.browse(&request).await?.items);
            if items.len() >= 40 {
                break;
            }
        }
        items.truncate(40);
        Ok(items)
    }

    /// One title's cast, in billing order.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the read fails.
    pub async fn title_cast(
        &self,
        title_id: i64,
        limit: u32,
    ) -> Result<Vec<peregrine_storage::CastRow>, RuntimeError> {
        Ok(self.database.cast_for_title(title_id, limit).await?)
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

    /// Whether the account's watchlist holds one title.
    ///
    /// # Errors
    ///
    /// `RuntimeError::Storage` when the read fails.
    pub async fn is_watchlisted(&self, title_id: i64) -> Result<bool, RuntimeError> {
        Ok(self
            .database
            .watchlist_entry(installation_scope(), title_id)
            .await?
            .is_some())
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

    /// Where this title can be watched in one region, as the provider last answered.
    ///
    /// A cache read: the fetching is the title refresh job's, which is also what `refresh_due`
    /// starts. `fetched_at` is what lets the screen say "nobody has asked" rather than drawing an
    /// empty section as if the answer were "nothing is streaming it".
    ///
    /// The logos are resolved here for the same reason the poster is: the cache key is a hash this
    /// layer computes, and whether the blob is on disk is a `stat` only this layer can make.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::UnknownTitle`] when the title is not here or the scope may not see it, and
    /// [`RuntimeError::Storage`] when a read fails.
    pub async fn watch_availability(
        &self,
        title_id: i64,
        region: &Region,
    ) -> Result<WatchAvailabilityView, RuntimeError> {
        if self
            .database
            .library_title(installation_scope(), title_id)
            .await?
            .is_none()
        {
            return Err(RuntimeError::UnknownTitle);
        }
        let fetched_at = self.database.watch_synced_at(title_id).await?;
        let region_row = self
            .database
            .watch_region(installation_scope(), title_id, region.as_str())
            .await?;
        let source = region_row
            .as_ref()
            .and_then(|row| row.source.parse::<peregrine_domain::Provider>().ok());
        let offers = self
            .database
            .watch_offers(installation_scope(), title_id, region.as_str())
            .await?;

        // TMDB lists subscription tiers and channels as separate providers with the same artwork:
        // one row of five Paramount+ marks is the failure this collapses. The brand key handles the
        // named tiers; the logo set catches a variant whose name the key does not recognise. One
        // mark per category is what the row is for, and the raw rows keep every tier for a later
        // detail surface.
        let mut brands = std::collections::HashSet::new();
        let mut logos = std::collections::HashSet::new();
        let offers: Vec<_> = offers
            .into_iter()
            .filter(|offer| {
                if !brands.insert((offer.category, watch_brand(&offer.provider_name))) {
                    return false;
                }
                match offer.logo_path.as_deref() {
                    Some(path) => logos.insert((offer.category, path.to_owned())),
                    None => true,
                }
            })
            .collect();

        Ok(WatchAvailabilityView {
            region: region.clone(),
            link: region_row.and_then(|row| row.link),
            fetched_at,
            offers: offers
                .into_iter()
                .map(|offer| WatchOfferView {
                    category: offer.category,
                    provider_id: offer.provider_id,
                    provider_name: offer.provider_name,
                    logo: offer.logo_path.as_deref().and_then(|path| {
                        source.and_then(|provider| {
                            crate::artwork::cached_image(
                                &self.artwork,
                                provider,
                                path,
                                ImageKind::Logo,
                            )
                        })
                    }),
                    display_priority: offer.display_priority,
                })
                .collect(),
        })
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
        write_reported_position(
            &self.database,
            self.clock.as_ref(),
            title_id,
            episode_id,
            position,
        )
        .await?;
        Ok(())
    }

    /// Finalizes a playback that reached the media end and marks its progress watched.
    pub async fn finalize_progress(
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
        let now = self.clock.now();
        let existing = self
            .database
            .watch_progress(installation_scope(), title_id, episode_id)
            .await?;
        let mut progress = match existing {
            Some(mut progress) => {
                progress.set_position(position, now);
                progress
            }
            None => WatchProgress::record(
                installation_scope().account(),
                title_id,
                episode_id,
                position,
                ProgressSource::Local,
                now,
                None,
            ),
        };
        progress
            .apply_finalize(finalize_watch(position, true, now), now)
            .map_err(|error| RuntimeError::InvalidRequest {
                detail: error.to_string(),
            })?;
        self.database.record_watch_progress(progress).await?;
        Ok(())
    }

    /// Marks one episode watched or unwatched, on the user's authority.
    ///
    /// The row is created when the episode has none, so a season can be marked before anything is
    /// played. A manual mark is exempt from the storage floor (I6), which is why the origin is
    /// `Manual` rather than `Policy`.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::UnknownTitle`] when the title or episode is not in the library, and
    /// [`RuntimeError::Storage`] when the read or write fails.
    pub async fn set_episode_watched(
        &self,
        title_id: i64,
        season: u32,
        episode: u32,
        watched: bool,
    ) -> Result<(), RuntimeError> {
        let Some(episode_id) = self
            .database
            .episode_id_for(title_id, season, episode)
            .await?
        else {
            return Err(RuntimeError::InvalidRequest {
                detail: "that episode is not in your library".to_owned(),
            });
        };
        if watched
            && self
                .database
                .list_title_episodes(installation_scope(), title_id, Some(season), 500)
                .await?
                .into_iter()
                .find(|row| row.episode_number.get() == episode)
                .and_then(|row| row.air_date)
                .is_some_and(|air_date| air_date > self.today())
        {
            return Err(RuntimeError::NotReleased);
        }
        self.set_watched(title_id, Some(episode_id), watched).await
    }

    /// Marks every episode of one season watched or unwatched, answering how many rows it touched.
    ///
    /// The episodes are the season read's own rows, so a season the provider described but this
    /// build has not fetched marks nothing rather than inventing numbers.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Storage`] when a read or write fails.
    pub async fn set_season_watched(
        &self,
        title_id: i64,
        season: u32,
        watched: bool,
    ) -> Result<u32, RuntimeError> {
        let episodes = self
            .database
            .list_title_episodes(installation_scope(), title_id, Some(season), 500)
            .await?;
        let today = self.today();
        let mut touched = 0;
        for row in episodes {
            // A season that is still airing marks only the episodes that have aired. Refusing the
            // whole season would make the control useless for the weeks in between; marking an
            // unaired one would claim something nobody has seen.
            if watched && row.air_date.is_some_and(|air_date| air_date > today) {
                continue;
            }
            let Some(episode_id) = self
                .database
                .episode_id_for(title_id, row.season_number.get(), row.episode_number.get())
                .await?
            else {
                continue;
            };
            self.set_watched(title_id, Some(episode_id), watched)
                .await?;
            touched += 1;
        }
        Ok(touched)
    }

    /// Marks a whole title watched or unwatched, for a film rather than for one episode.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Storage`] when the read or write fails.
    pub async fn set_title_watched(
        &self,
        title_id: i64,
        watched: bool,
    ) -> Result<(), RuntimeError> {
        if watched
            && let Some(detail) = self
                .database
                .library_title(installation_scope(), title_id)
                .await?
            && detail
                .released
                .is_some_and(|released| released > self.today())
        {
            return Err(RuntimeError::NotReleased);
        }
        self.set_watched(title_id, None, watched).await
    }

    /// Whether the account has marked or finished a whole title, which a film's page draws.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Storage`] when the read fails.
    pub async fn title_watched(&self, title_id: i64) -> Result<bool, RuntimeError> {
        Ok(self
            .database
            .watch_progress(installation_scope(), title_id, None)
            .await?
            .is_some_and(|progress| progress.watched()))
    }

    /// Today in the calendar the stored release and air dates use (UTC).
    ///
    /// The screens compare against `toISOString().slice(0, 10)`, which is the same day, so the
    /// sentence beside a control and this refusal cannot disagree about whether it has aired.
    fn today(&self) -> jiff::civil::Date {
        self.clock.now().to_zoned(jiff::tz::TimeZone::UTC).date()
    }

    /// The one write both marking surfaces share.
    async fn set_watched(
        &self,
        title_id: i64,
        episode_id: Option<i64>,
        watched: bool,
    ) -> Result<(), RuntimeError> {
        let now = self.clock.now();
        let existing = self
            .database
            .watch_progress(installation_scope(), title_id, episode_id)
            .await?;
        let mut progress = match existing {
            Some(row) => row,
            // Unwatching something with no row is already the answer the caller wants.
            None if !watched => return Ok(()),
            None => WatchProgress::record(
                installation_scope().account(),
                title_id,
                episode_id,
                PlaybackPosition::new(0, None).map_err(|error| RuntimeError::InvalidRequest {
                    detail: error.to_string(),
                })?,
                ProgressSource::Manual,
                now,
                None,
            ),
        };
        if watched {
            progress
                .mark_watched(WatchOrigin::Manual, now)
                .map_err(|error| RuntimeError::InvalidRequest {
                    detail: error.to_string(),
                })?;
        } else {
            progress.mark_unwatched(now);
        }
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

    /// The notification consent and the catalogue defaults around it.
    ///
    /// # Errors
    ///
    /// [`crate::notify::ServiceError::Preferences`] when the stored consent cannot be read.
    pub async fn notification_preferences(
        &self,
    ) -> Result<Preferences, crate::notify::ServiceError> {
        self.notification_service.preferences().await
    }

    /// Records the user's answer to the notification consent question.
    ///
    /// A refusal is stored with `dont_ask_again`, so the wizard never re-asks a user who has
    /// answered; the Settings control remains theirs to change. The answer is persisted before
    /// anything acts on it (R31).
    ///
    /// # Errors
    ///
    /// [`crate::notify::ServiceError::Preferences`] when the answer cannot be stored.
    pub async fn set_notification_consent(
        &self,
        granted: bool,
    ) -> Result<Preferences, crate::notify::ServiceError> {
        let consent = if granted {
            Consent::granted()
        } else {
            Consent::declined(true)
        };
        self.notification_service.record_consent(consent).await
    }

    /// The in-app notification centre, newest first.
    ///
    /// R31 makes the centre row the record and the OS toast the bonus, so this read is how a
    /// notification reaches the user even when the system will not show one.
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

    /// The Usenet settings surface's read.
    ///
    /// # Errors
    ///
    /// The storage or keyring refusal the settings service reports.
    pub async fn usenet_status(
        &self,
    ) -> Result<crate::usenet::UsenetStatus, crate::usenet::UsenetConfigError> {
        crate::usenet::status(&self.database, Arc::clone(&self.secrets)).await
    }

    /// Inserts or updates one Usenet provider.
    pub async fn save_usenet_server(
        &self,
        input: crate::usenet::UsenetServerInput,
    ) -> Result<crate::usenet::UsenetStatus, crate::usenet::UsenetConfigError> {
        crate::usenet::save(
            &self.database,
            Arc::clone(&self.secrets),
            input,
            self.clock.now(),
        )
        .await
    }

    /// Removes one Usenet provider and its keyring entry.
    pub async fn remove_usenet_server(
        &self,
        id: i64,
    ) -> Result<crate::usenet::UsenetStatus, crate::usenet::UsenetConfigError> {
        crate::usenet::remove(&self.database, Arc::clone(&self.secrets), id).await
    }

    /// Tests one Usenet provider without storing it.
    pub async fn test_usenet_server(
        &self,
        input: crate::usenet::UsenetServerInput,
    ) -> Result<(), crate::usenet::UsenetConfigError> {
        crate::usenet::test(
            &self.database,
            Arc::clone(&self.secrets),
            input,
            self.clock.now(),
        )
        .await
    }

    /// Re-inspects a blocked Usenet job with the archive opt-in.
    pub async fn consent_usenet(
        &self,
        id: i64,
    ) -> Result<peregrine_domain::DownloadState, RuntimeError> {
        let servers = crate::usenet::server_configs(&self.database, Arc::clone(&self.secrets))
            .await
            .map_err(|error| RuntimeError::InvalidRequest {
                detail: error.to_string(),
            })?;
        self.acquisition()?
            .consent_usenet(id, servers)
            .await
            .map_err(|error| RuntimeError::InvalidRequest {
                detail: error.to_string(),
            })
    }

    /// Writes the Usenet archive policy.
    pub async fn set_usenet_archives(
        &self,
        allow_archives: bool,
        delete_archives_after_import: bool,
    ) -> Result<peregrine_storage::UsenetSetting, crate::usenet::UsenetConfigError> {
        crate::usenet::set_archives(
            &self.database,
            allow_archives,
            delete_archives_after_import,
            self.clock.now(),
        )
        .await
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
        // An external play of a torrent holds a loopback listener for mpv. The child is gone at this
        // point, so the listener has no reader left and is closed here rather than left to the drop.
        let held = *self
            .session_id
            .lock()
            .expect("the session id lock is not poisoned");
        if let Some(session_id) = held {
            self.stop_playback(session_id).await;
        }
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

/// How many sidecar subtitle tracks one session reads for the file it plays.
///
/// A page rather than an unbounded read, like every other list. A hundred tracks on one file is far
/// past any real release, and the tracks past the bound are simply not offered.
const MAX_SUBTITLE_TRACKS: u32 = 100;

/// The content type a sidecar subtitle is served as, from its stored format.
///
/// `WebVTT` has a registered type; `SRT`, `ASS`, and `SSA` have the de-facto ones their parsers
/// accept; the bitmap formats are labelled as bytes because no track element renders them as text.
/// The mapping lives here because it is the app's composition of a stored column, not the domain's
/// rule.
const fn subtitle_content_type(format: SubtitleFormat) -> &'static str {
    match format {
        SubtitleFormat::Srt => "application/x-subrip",
        SubtitleFormat::Vtt => "text/vtt",
        SubtitleFormat::Ass | SubtitleFormat::Ssa => "text/x-ssa",
        SubtitleFormat::Sub => "text/plain",
        SubtitleFormat::Pgs | SubtitleFormat::Vobsub => "application/octet-stream",
    }
}

/// How many configured roots are checked when deciding whether a media path may be served.
///
/// A bound rather than an iteration to exhaustion because the read pages: a caller that asked for "all
/// of them" would be a caller that could be made to hold an unbounded list. Two hundred roots is far
/// past any plausible library, and a path that is inside a root past this one is *refused* rather than
/// served — which is the direction that cannot leak (SECURITY.md's path-safety row).
const MAX_MEDIA_ROOTS: u32 = 200;

/// ipify's documented JSON echo endpoint. One request, only from the reader's click.
const PUBLIC_IP_ECHO: &str = "https://api.ipify.org?format=json";

/// The address in an ipify answer, validated as an address rather than trusted as text.
fn public_ip_from_json(body: &str) -> Option<String> {
    let answer: serde_json::Value = serde_json::from_str(body).ok()?;
    answer
        .get("ip")?
        .as_str()?
        .trim()
        .parse::<std::net::IpAddr>()
        .ok()
        .map(|ip| ip.to_string())
}

/// Why the runtime could not start, or could not do what it was asked.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// The torrent session did not start, so acquisition and streaming are unavailable.
    #[error("the torrent engine is not running")]
    TorrentEngineUnavailable,

    /// The public-address probe could not reach or understand its echo service.
    #[error("the public address could not be read; check the connection and try again")]
    PublicIpUnavailable,

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

    /// A download holds several episodes and none of them is the one asked about.
    ///
    /// Its own variant rather than `NoPlayableFile`, because the sentence a person needs is not "there
    /// is nothing" but "there is something and it could not be told which episode it is" — and the
    /// action differs: wait for the import, or remove the pack.
    #[error("that download holds more than one episode and none matches the one asked for")]
    MediaUnidentified,

    /// A watch mark named a title or episode that has not been released.
    ///
    /// A mark is a statement about what the account has seen, and nobody has seen a film that is not
    /// out. The refusal lives here rather than in the two screens because both surfaces — the
    /// desktop and the paired browser — write through this runtime.
    #[error("that title has not been released yet, so it cannot be marked watched")]
    NotReleased,

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
    use peregrine_domain::PlayerBackend as _;
    use peregrine_secrets::{SecretKey, SecretStore};
    use peregrine_testkit::MemorySecretStore;
    use std::fmt::Write as _;
    use tempfile::TempDir;

    /// One `download_file` row, as metadata named it.
    fn download_entry(name: &str, size: u64, is_media: bool) -> peregrine_storage::DownloadFileRow {
        peregrine_storage::DownloadFileRow {
            id: 1,
            download_job_id: 1,
            file_index: 0,
            path: NonEmptyText::new(name).expect("a file name"),
            size_bytes: size,
            selected: true,
            is_media,
        }
    }

    #[test]
    fn one_media_entry_is_the_answer_and_needs_no_episode_match() {
        let files = vec![download_entry("Film.2025.1080p.mkv", 900, true)];
        assert_eq!(choose_download_file(&files, None), Ok(0));
        assert_eq!(
            choose_download_file(&files, Some((1, 2))),
            Ok(0),
            "a single-entry episode release is the episode"
        );
    }

    #[test]
    fn a_film_with_extras_takes_the_largest_media_entry() {
        let files = vec![
            download_entry("Film.2025.1080p.mkv", 900, true),
            download_entry("sample.mkv", 10, true),
            download_entry("Film.2025.srt", 1, false),
        ];
        assert_eq!(choose_download_file(&files, None), Ok(0));
    }

    #[test]
    fn a_season_pack_is_resolved_by_the_episode_its_file_names() {
        let files = vec![
            download_entry("Show.S01.1080p/Show.S01E01.mkv", 10, true),
            download_entry("Show.S01.1080p/Show.S01E02.mkv", 10, true),
            download_entry("Show.S01.1080p/Show.S01E03E04.mkv", 10, true),
            download_entry("Show.S01.1080p/Show.S01E02.srt", 1, false),
        ];
        // The folder states S01 for every entry; only the file states which episode it is.
        assert_eq!(choose_download_file(&files, Some((1, 2))), Ok(1));
        assert_eq!(choose_download_file(&files, Some((1, 4))), Ok(2));
        assert_eq!(
            choose_download_file(&files, Some((1, 9))),
            Err(FileChoice::Ambiguous),
            "an unmatched episode is refused rather than guessed by size"
        );
    }

    #[test]
    fn a_payload_with_no_media_entry_is_not_ready() {
        let files = vec![download_entry("Show.S01E02.srt", 1, false)];
        assert_eq!(
            choose_download_file(&files, Some((1, 2))),
            Err(FileChoice::NotReady)
        );
    }

    struct NoPositions;

    impl peregrine_domain::PositionSink for NoPositions {
        fn position(&self, _position_ms: i64, _duration_ms: Option<i64>) {}
    }

    #[test]
    fn the_recording_player_keeps_a_stream_url() {
        let url = LoopbackUrl::parse("http://127.0.0.1:43123/media/token").expect("a loopback URL");
        let player = RecordingPlayer::default();
        player
            .play(PlayerInput::Stream(&url), None, None, Arc::new(NoPositions))
            .expect("the double records it");
        assert_eq!(player.streams(), vec![url.as_str().to_owned()]);
    }

    #[test]
    fn a_public_ip_answer_is_read_as_an_address() {
        assert_eq!(
            public_ip_from_json(r#"{"ip":"203.0.113.9"}"#).as_deref(),
            Some("203.0.113.9")
        );
        assert_eq!(
            public_ip_from_json(r#"{"ip":"2001:db8::1"}"#).as_deref(),
            Some("2001:db8::1")
        );
    }

    #[test]
    fn a_public_ip_answer_that_is_not_an_address_is_refused() {
        assert_eq!(public_ip_from_json(r#"{"ip":"not an address"}"#), None);
        assert_eq!(public_ip_from_json(r#"{"ip":""}"#), None);
        assert_eq!(public_ip_from_json("<html>no</html>"), None);
        assert_eq!(public_ip_from_json(r#"{"address":"203.0.113.9"}"#), None);
    }

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
        streams: Mutex<Vec<String>>,
        start: Mutex<Option<i64>>,
        subtitle: Mutex<Option<PathBuf>>,
    }

    impl RecordingPlayer {
        fn opened(&self) -> Vec<PathBuf> {
            self.opened
                .lock()
                .expect("the player log is not poisoned")
                .clone()
        }

        fn streams(&self) -> Vec<String> {
            self.streams
                .lock()
                .expect("the player stream log is not poisoned")
                .clone()
        }

        fn start(&self) -> Option<i64> {
            *self.start.lock().expect("the player start is not poisoned")
        }

        fn subtitle(&self) -> Option<PathBuf> {
            self.subtitle
                .lock()
                .expect("the player subtitle is not poisoned")
                .clone()
        }
    }

    impl peregrine_domain::PlayerBackend for RecordingPlayer {
        fn play(
            &self,
            input: peregrine_domain::PlayerInput<'_>,
            start_ms: Option<i64>,
            subtitle: Option<&SafePath>,
            positions: Arc<dyn peregrine_domain::PositionSink>,
        ) -> Result<(), peregrine_domain::PlayerError> {
            match input {
                peregrine_domain::PlayerInput::File(file) => self
                    .opened
                    .lock()
                    .expect("the player log is not poisoned")
                    .push(file.as_path().to_path_buf()),
                peregrine_domain::PlayerInput::Stream(url) => self
                    .streams
                    .lock()
                    .expect("the player stream log is not poisoned")
                    .push(url.as_str().to_owned()),
            }
            *self.start.lock().expect("the player start is not poisoned") = start_ms;
            *self
                .subtitle
                .lock()
                .expect("the player subtitle is not poisoned") =
                subtitle.map(|path| path.as_path().to_path_buf());
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

    /// An episode card's still is the cache's answer, and a captured frame stands behind a provider's.
    ///
    /// This is the wiring the whole capture path exists for: the blob is written under the episode's
    /// own key, the episode row records the provider's pair only when a provider gave one, and the
    /// read has to prefer the provider and fall back to the capture.
    #[tokio::test]
    async fn an_episode_card_draws_a_captured_still_when_no_provider_has_one() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;
        let show = runtime
            .database
            .insert_stub_title(peregrine_storage::StubTitle {
                kind: peregrine_domain::catalogue::TitleKind::Show,
                subtype: peregrine_domain::catalogue::TitleSubtype::Standard,
                name: "VisionQuest".to_owned(),
                original_name: None,
                sort_name: "visionquest".to_owned(),
                at: 0,
            })
            .await
            .unwrap();
        let season = runtime
            .database
            .insert_stub_season(peregrine_storage::StubSeason {
                title_id: show,
                season_number: 1,
                at: 0,
            })
            .await
            .unwrap();
        for episode_number in [3, 4] {
            runtime
                .database
                .insert_stub_episode(peregrine_storage::StubEpisode {
                    title_id: show,
                    season_id: season,
                    season_number: 1,
                    episode_number,
                    at: 0,
                })
                .await
                .unwrap();
        }
        let three = runtime
            .database
            .episode_id_for(show, 1, 3)
            .await
            .unwrap()
            .expect("episode three");
        let four = runtime
            .database
            .episode_id_for(show, 1, 4)
            .await
            .unwrap()
            .expect("episode four");

        let cache = peregrine_sync::ImageCache::new(runtime.config().artwork_directory());
        let captured = cache
            .store(
                &peregrine_sync::ImageKey::for_captured_still(three),
                b"a frame from the downloaded file",
            )
            .expect("the artwork directory is writable");
        let provider = cache
            .store(
                &peregrine_sync::ImageKey::for_image(
                    peregrine_domain::Provider::Tmdb,
                    "/s01e04.jpg",
                    peregrine_sync::ImageVariant::StillW300,
                ),
                b"the provider's own still",
            )
            .expect("the artwork directory is writable");
        runtime
            .database
            .set_episode_still(four, "tmdb", "/s01e04.jpg", 0)
            .await
            .unwrap();

        let episodes = runtime.title_episodes(show, Some(1), 50).await.unwrap();
        let three_row = episodes
            .iter()
            .find(|episode| episode.row.episode_number.get() == 3)
            .expect("episode three is listed");
        let four_row = episodes
            .iter()
            .find(|episode| episode.row.episode_number.get() == 4)
            .expect("episode four is listed");
        assert_eq!(
            three_row.still.as_deref(),
            Some(captured.disk_path()),
            "the capture is the card's still when no provider has one"
        );
        assert_eq!(
            four_row.still.as_deref(),
            Some(provider.disk_path()),
            "a provider's still wins over any capture"
        );
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

    /// Pressing Start while a start is already queued joins that row rather than failing on the
    /// journal.
    ///
    /// `job_dedupe_uq` refuses a second non-terminal row for the same key, and the storage layer
    /// answers that refusal with a bare "a statement failed" — which the screen shows as "The
    /// library database rejected a statement." for what is only a second press.
    #[tokio::test]
    async fn a_live_prowlarr_start_is_joined_rather_than_duplicated() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;
        let kind = crate::jobs::prowlarr::START_KIND;

        let journal_id = runtime
            .database
            .insert_job(NewJob {
                kind: kind.to_owned(),
                priority: crate::jobs::prowlarr::PRIORITY,
                dedupe_key: Some(kind.to_owned()),
                payload_json: "{}".to_owned(),
                max_attempts: 1,
                created_at: runtime.clock.now().as_millisecond(),
            })
            .await
            .unwrap();

        let joined = runtime.start_prowlarr().await.unwrap();
        assert_eq!(joined.journal_id, journal_id);
        assert!(
            joined.coalesced,
            "the second press joins the start already in flight"
        );
        let rows = runtime.jobs(None, 10).await.unwrap();
        assert_eq!(
            rows.iter().filter(|row| row.kind == kind).count(),
            1,
            "one row per start, not a second one the index would refuse"
        );
        runtime.shutdown().await;
    }

    /// Pressing Download season while that season's fan-out is still running joins the job the
    /// first press journalled rather than colliding with `job_dedupe_uq`.
    ///
    /// **This is the live failure on title 749.** The fan-out runs for minutes, and a second press
    /// inside that window inserted into a key a non-terminal row already held; the index refused
    /// it and `jobs::insert` mapped the driver error to `StorageError::Statement`, which the screen
    /// showed as "The library database rejected a statement." After the job finished, a later check
    /// found no new row and no live rows at all, which is what made the index look innocent.
    #[tokio::test]
    async fn a_second_season_acquire_joins_the_journalled_job_rather_than_colliding() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;
        let key = crate::jobs::acquire::dedupe_key(749, Some(1), None);

        // The row the first press writes, while the season's fan-out is running.
        let journal_id = runtime
            .database
            .insert_job(NewJob {
                kind: crate::jobs::acquire::KIND.to_owned(),
                priority: crate::jobs::acquire::PRIORITY,
                dedupe_key: Some(key.clone()),
                payload_json: "{\"titleId\":749}".to_owned(),
                max_attempts: 2,
                created_at: runtime.clock.now().as_millisecond(),
            })
            .await
            .expect("a season acquire is journalled");

        let joined = runtime
            .start_acquire_season(installation_scope(), 749, 1, 1)
            .await
            .expect("a second season request joins the job already running");
        assert_eq!(joined.journal_id, journal_id);
        assert!(
            joined.coalesced,
            "the second press reports the work it joined rather than a new job"
        );

        let rows = runtime.jobs(None, 10).await.unwrap();
        assert_eq!(
            rows.iter()
                .filter(|row| row.dedupe_key.as_deref() == Some(key.as_str()))
                .count(),
            1,
            "one journalled season job, not a second row the index would refuse"
        );
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
            .with_similar_answer(vec![
                candidate("604", "The Matrix Reloaded"),
                candidate("603", "The Matrix"),
            ]),
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
        scanned_files(directory, &[(name, vec![9_u8; 4096])]).await
    }

    /// The same scan over a folder holding several files: a film and the sidecars beside it.
    async fn scanned_files(directory: &TempDir, files: &[(&str, Vec<u8>)]) -> (Runtime, i64) {
        let movies = directory.path().join("Movies");
        std::fs::create_dir_all(&movies).expect("the library folder is creatable");
        for (name, bytes) in files {
            std::fs::write(movies.join(name), bytes).expect("the fixture file writes");
        }
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

    /// A sidecar the scan indexed is offered by the session and served over its own URL.
    ///
    /// This is the wiring `peregrine-stream` cannot assert alone: the row the scanner wrote, the path
    /// guard's verdict, the session's registration, and the route that answers — one path from a file on
    /// disk to bytes a track element can follow.
    #[tokio::test]
    async fn a_scanned_sidecar_is_offered_by_the_session_and_served_over_its_url() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        const SUBTITLE: &str = "1\n00:00:01,000 --> 00:00:02,000\nHello\n";
        let directory = TempDir::new().unwrap();
        let (runtime, id) = scanned_files(
            &directory,
            &[
                ("Alien (1979).mp4", vec![9_u8; 4096]),
                ("Alien (1979).en.srt", SUBTITLE.as_bytes().to_vec()),
            ],
        )
        .await;

        let started = runtime
            .start_playback(id, None)
            .await
            .expect("the film plays");

        assert_eq!(started.subtitles.len(), 1, "the sidecar the scan indexed");
        let track = &started.subtitles[0];
        assert_eq!(track.language, "en");
        assert_eq!(track.format, "srt");
        assert_eq!(track.label, None);
        assert!(!track.forced);
        assert!(!track.sdh);
        assert!(!track.default, "a scan writes no default");
        assert!(
            track.url.starts_with("http://127.0.0.1:"),
            "the bytes go over the session's own listener, not the asset protocol: {}",
            track.url
        );
        assert!(track.url.contains("/subtitle/"));
        assert!(track.url.ends_with(&format!("/{}", track.id)));

        // The route really answers, which a URL alone cannot show. A track element sends exactly this.
        let port: u16 = track
            .url
            .strip_prefix("http://127.0.0.1:")
            .and_then(|rest| rest.split('/').next())
            .and_then(|text| text.parse().ok())
            .expect("the track URL carries a port");
        let path = track
            .url
            .strip_prefix(&format!("http://127.0.0.1:{port}"))
            .expect("the track URL is loopback");
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
        let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("the session is listening");
        socket.write_all(request.as_bytes()).await.unwrap();
        socket.flush().await.unwrap();
        let mut answer = String::new();
        socket.read_to_string(&mut answer).await.unwrap();
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
        assert!(
            answer.contains(SUBTITLE),
            "the served bytes are the sidecar's: {answer}"
        );

        runtime.shutdown().await;
    }

    /// A sidecar row whose path is outside every configured root is refused track by track.
    ///
    /// The database is not the filesystem: a row can name anything (a stale scan, a moved library, a
    /// tampered database). Serving that path would hand the loopback listener a file the user never
    /// pointed the library at, so the guard refuses it and the film still plays.
    #[tokio::test]
    async fn a_sidecar_outside_the_guarded_roots_is_not_offered() {
        use peregrine_domain::{
            NonEmptyText, SubtitleFormat, SubtitleOrigin, SubtitleOwner, SubtitleTrack,
        };

        let directory = TempDir::new().unwrap();
        let (runtime, id) = scanned(&directory, "Alien (1979).mp4").await;

        // The media file's row id, which the scan wrote and the session reads sidecars by.
        let first = runtime
            .start_playback(id, None)
            .await
            .expect("the film plays");
        let media_file_id = first.media_file_id;
        runtime.stop_playback(first.session_id).await;

        runtime
            .database
            .insert_subtitle_track(SubtitleTrack {
                id: 0,
                owner: SubtitleOwner::File { media_file_id },
                origin: SubtitleOrigin::External,
                format: SubtitleFormat::Srt,
                language: NonEmptyText::new("en").expect("a language tag"),
                label: None,
                is_forced: false,
                is_sdh: false,
                is_default: false,
                stream_index: None,
                path: Some("/etc/passwd".to_owned()),
                provider: None,
                provider_subtitle_id: None,
                provider_url: None,
                release_name: None,
                encoding: None,
                offset_ms: 0,
                hash_matched: false,
                downloaded_at: None,
                last_used_at: None,
            })
            .await
            .expect("the row writes");

        let started = runtime
            .start_playback(id, None)
            .await
            .expect("the film plays");
        assert!(
            started.subtitles.is_empty(),
            "a path outside every root is never served: {:?}",
            started.subtitles
        );

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
            .start_external_playback(title_id, None)
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

    /// A downloaded film the browser demuxes, with a known runtime, for the direct and progress
    /// assertions. The bytes are a fixture, not a decodable film: nothing here plays them.
    async fn downloaded_mp4(runtime: &Runtime, directory: &TempDir) -> (i64, std::path::PathBuf) {
        use peregrine_domain::{MediaFile, MediaItem, MediaItemState, MediaOrigin, NonEmptyText};

        let folder = directory.path().join("downloads").join("A Film");
        std::fs::create_dir_all(&folder).expect("the download folder");
        let film = folder.join("A Film (2024).mp4");
        std::fs::write(&film, vec![0_u8; 4096]).expect("the fixture file");
        let at = jiff::Timestamp::from_millisecond(0).expect("in range");
        let title_id = runtime
            .database
            .insert_stub_title(peregrine_storage::StubTitle {
                kind: peregrine_domain::catalogue::TitleKind::Movie,
                subtype: peregrine_domain::catalogue::TitleSubtype::Standard,
                name: "A Film".to_owned(),
                original_name: None,
                sort_name: "a film".to_owned(),
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
                size_bytes: 4096,
                mtime_ms: at,
                added_at: at,
                missing_since: None,
                container: None,
                duration_ms: Some(7_200_000),
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

    /// The HLS routes over a real transcode: a playlist, an init segment, and a media segment.
    ///
    /// Ignored because it needs a real ffmpeg with a permitted encoder; run it with
    /// `cargo test -p peregrine-app a_real_mkv_plays_through_the_hls_routes -- --ignored`.
    #[tokio::test]
    #[ignore = "needs a real ffmpeg with a permitted H.264 encoder on PATH"]
    async fn a_real_mkv_plays_through_the_hls_routes() {
        use std::net::{IpAddr, Ipv4Addr};

        use crate::web::WebConfig;

        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;
        let (title_id, film) = downloaded_title(&runtime, &directory).await;
        let made = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=24",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440",
                "-t",
                "3",
                "-c:v",
                "mpeg4",
                "-c:a",
                "pcm_s16le",
            ])
            .arg(&film)
            .status()
            .expect("the fixture is generated");
        assert!(made.success());

        let service = Arc::new(Arc::new(runtime).web_service());
        let status = service
            .start(WebConfig::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .await
            .unwrap();
        let port = status.address.expect("a bound address").port();
        let base = format!("http://127.0.0.1:{port}");
        let offer = service
            .request_pair(IpAddr::V4(Ipv4Addr::LOCALHOST), "Test TV", None)
            .unwrap();
        let device = service.approve(&offer.pair_id, false).await.unwrap();
        let crate::web::PairStatus::Approved {
            token: Some(token), ..
        } = service.pair_status(&offer.pair_id)
        else {
            panic!("the approval yields a token");
        };
        let client = reqwest::Client::new();
        let ticket: serde_json::Value = client
            .post(format!("{base}/api/playback/{title_id}"))
            .bearer_auth(&token)
            .json(&serde_json::json!({ "positionMs": 0 }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(ticket["kind"], "hls", "an MKV is transcoded: {ticket}");
        let playlist_url = ticket["url"].as_str().expect("a playlist URL").to_owned();
        let playlist = client
            .get(format!("{base}{playlist_url}"))
            .send()
            .await
            .unwrap();
        assert_eq!(playlist.status(), 200);
        let body = playlist.text().await.unwrap();
        assert!(body.contains("#EXTM3U"));
        assert!(body.contains("init.mp4"));
        let directory_url = playlist_url.replace("index.m3u8", "");
        let init = client
            .get(format!("{base}{directory_url}init.mp4"))
            .send()
            .await
            .unwrap();
        assert_eq!(init.status(), 200);
        let segment = body
            .lines()
            .find(|line| {
                std::path::Path::new(line)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("m4s"))
            })
            .expect("a segment");
        let media = client
            .get(format!("{base}{directory_url}{segment}"))
            .send()
            .await
            .unwrap();
        assert_eq!(media.status(), 200);
        assert_eq!(media.headers().get("content-type").unwrap(), "video/mp4");
        assert!(service.revoke(device.id).await.unwrap());
        service.stop().await;
    }

    /// A mark is a statement about what the account has seen, and a title that is not out has
    /// nothing to have seen. The screen hides the control; the core refuses the write too, because
    /// the paired browser can reach the same runtime.
    #[tokio::test]
    async fn a_watch_mark_on_a_future_release_is_refused() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;
        let title_id = runtime
            .database
            .insert_stub_title(peregrine_storage::StubTitle {
                kind: peregrine_domain::catalogue::TitleKind::Movie,
                subtype: peregrine_domain::catalogue::TitleSubtype::Standard,
                name: "Dune: Part Three".to_owned(),
                original_name: None,
                sort_name: "dune part three".to_owned(),
                at: 0,
            })
            .await
            .expect("the title");
        let mut future = details("Dune: Part Three", None);
        future.released = Some(jiff::civil::date(2999, 1, 1));
        runtime
            .database
            .update_title_metadata(
                title_id,
                future,
                "dune part three".to_owned(),
                peregrine_storage::MetadataState::Complete,
                "tmdb".to_owned(),
                0,
            )
            .await
            .expect("the release date");

        let error = runtime
            .set_title_watched(title_id, true)
            .await
            .expect_err("a future release cannot be marked watched");
        assert!(matches!(error, RuntimeError::NotReleased));
        runtime
            .set_title_watched(title_id, false)
            .await
            .expect("clearing a mark is always allowed");
    }

    /// A mark the user set outranks a playback report in both directions: watching a film someone
    /// marked watched must not unmark it, and replaying one they marked unwatched must not mark it.
    #[tokio::test]
    async fn a_manual_watch_mark_survives_a_progress_report() {
        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;
        let (title_id, _film) = downloaded_title(&runtime, &directory).await;

        runtime
            .set_title_watched(title_id, true)
            .await
            .expect("the user marks it");
        assert!(runtime.title_watched(title_id).await.expect("the read"));
        runtime
            .record_progress(title_id, None, 60_000, Some(7_200_000))
            .await
            .expect("a report lands");
        assert!(
            runtime.title_watched(title_id).await.expect("the read"),
            "a tick is a measurement, not a statement that the mark is wrong"
        );
        let row = runtime
            .database
            .watch_progress(installation_scope(), title_id, None)
            .await
            .expect("the row")
            .expect("it exists");
        assert_eq!(row.position_ms(), 60_000, "the position still moves");
        assert_eq!(row.source(), ProgressSource::Manual);

        runtime
            .set_title_watched(title_id, false)
            .await
            .expect("the user clears it");
        runtime
            .record_progress(title_id, None, 7_100_000, Some(7_200_000))
            .await
            .expect("a second report lands");
        assert!(
            !runtime.title_watched(title_id).await.expect("the read"),
            "a finished playback does not overwrite a manual unwatch"
        );
    }

    /// Tier and channel entries of one service are one mark on the row, which is the defect a
    /// real answer exposed: five Paramount+ boxes and two Netflix boxes looked like a bug.
    #[tokio::test]
    async fn watch_offers_collapse_tiers_of_one_brand() {
        use peregrine_domain::catalogue::{WatchAvailability, WatchOffer};

        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;
        let (title_id, _film) = downloaded_title(&runtime, &directory).await;
        let offer = |category, provider_id, name: &str| WatchOffer {
            category,
            provider_id,
            provider_name: NonEmptyText::new(name).expect("a name"),
            logo_path: None,
            display_priority: None,
        };
        runtime
            .database
            .replace_watch_availability(
                title_id,
                "tmdb".to_owned(),
                vec![WatchAvailability {
                    region: Region::new("US").expect("a region"),
                    link: None,
                    offers: vec![
                        offer(WatchCategory::Flatrate, 1, "Paramount Plus"),
                        offer(WatchCategory::Flatrate, 2, "Paramount Plus Premium"),
                        offer(WatchCategory::Flatrate, 3, "Paramount Plus Essential"),
                        offer(WatchCategory::Flatrate, 4, "Netflix"),
                        offer(WatchCategory::Flatrate, 5, "Netflix Standard with Ads"),
                        offer(WatchCategory::Rent, 6, "Apple TV"),
                    ],
                }],
                1,
            )
            .await
            .expect("the answer is written");

        let view = runtime
            .watch_availability(title_id, &Region::new("US").expect("a region"))
            .await
            .expect("the read answers");
        let names: Vec<&str> = view
            .offers
            .iter()
            .map(|offer| offer.provider_name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["Apple TV", "Netflix", "Paramount Plus"],
            "one mark per brand per category, in provider order"
        );
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
            .start_external_playback(title_id, None)
            .await
            .expect("the player starts");
        assert_eq!(
            player.start(),
            Some(42_000),
            "the stored position is where mpv starts"
        );
        assert_eq!(
            player.subtitle(),
            None,
            "a fixture with no sidecar hands mpv no subtitle"
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

    /// The LAN surface, end to end: a code, an approval, a cookie, a read, a revoke.
    ///
    /// This is the security acceptance of ADR 0028 in one test. It binds a real loopback socket and
    /// speaks HTTP to it, because the parts that can be wrong -- the Host check, the cookie the
    /// browser carries, the token hash the database stores, and the immediate refusal after a revoke
    /// -- are all between the pieces a unit test would mock.
    #[tokio::test]
    async fn a_device_approved_for_downloads_gets_the_acquire_scope() {
        use std::net::{IpAddr, Ipv4Addr};

        use crate::web::WebConfig;

        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;
        let service = Arc::new(Arc::new(runtime).web_service());
        let status = service
            .start(WebConfig::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .await
            .unwrap();
        let port = status.address.expect("a bound address").port();
        let base = format!("http://127.0.0.1:{port}");
        let offer = service
            .request_pair(IpAddr::V4(Ipv4Addr::LOCALHOST), "Download TV", None)
            .unwrap();
        let device = service.approve(&offer.pair_id, true).await.unwrap();
        assert_eq!(device.scopes, "read,stream,acquire");
        let crate::web::PairStatus::Approved {
            token: Some(token), ..
        } = service.pair_status(&offer.pair_id)
        else {
            panic!("the approval yields a token");
        };
        let client = reqwest::Client::new();
        let me: serde_json::Value = client
            .get(format!("{base}/api/me"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(me["canAcquire"], true);

        let half_an_episode = client
            .post(format!("{base}/api/acquire"))
            .bearer_auth(&token)
            .json(&serde_json::json!({ "titleId": 1, "season": 1 }))
            .send()
            .await
            .unwrap();
        assert_eq!(half_an_episode.status(), 400);
        let unknown = client
            .post(format!("{base}/api/acquire"))
            .bearer_auth(&token)
            .json(&serde_json::json!({ "titleId": 999_999 }))
            .send()
            .await
            .unwrap();
        assert_eq!(unknown.status(), 404);

        assert!(service.revoke(device.id).await.unwrap());
        let after = client
            .get(format!("{base}/api/me"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(after.status(), 401);
        service.stop().await;
    }
    #[tokio::test]
    #[allow(clippy::too_many_lines)] // One scenario read top to bottom: pairing, auth, refusal, revoke.
    async fn the_lan_surface_pairs_a_device_reads_the_library_and_refuses_it_after_a_revoke() {
        use std::net::{IpAddr, Ipv4Addr};

        use crate::web::WebConfig;

        let directory = TempDir::new().unwrap();
        let (runtime, _sink) = runtime(&directory).await;
        let (direct_id, _direct_file) = downloaded_mp4(&runtime, &directory).await;
        let (hls_id, _hls_file) = downloaded_title(&runtime, &directory).await;
        // The desktop writes a position first; the TV read has to answer with it, which is the
        // other direction of the same row this test writes over HTTP below.
        runtime
            .record_progress(hls_id, None, 30_000, Some(7_200_000))
            .await
            .unwrap();
        let service = Arc::new(Arc::new(runtime).web_service());
        // The built client is served when the checkout has one, which is the debug path
        // `web_client_dir` uses; a checkout that has not run the web build still gets the API.
        let web_dir =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/web/dist");
        let mut config = WebConfig::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        if web_dir.join("index.html").is_file() {
            config = config.with_app_dir(web_dir);
        }
        let status = service.start(config).await.unwrap();
        let port = status.address.expect("a bound address").port();
        let base = format!("http://127.0.0.1:{port}");
        let client = reqwest::Client::new();

        let offer: serde_json::Value = client
            .post(format!("{base}/api/pair"))
            .json(&serde_json::json!({ "name": "Test TV", "platform": "test" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let pair_id = offer["pairId"].as_str().expect("a pair id").to_owned();
        let code = offer["code"].as_str().expect("a code").to_owned();
        assert_eq!(code.len(), 8);

        let refused = client
            .get(format!("{base}/api/library"))
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), 401, "an unpaired read is refused");

        let device = service.approve(&pair_id, false).await.unwrap();
        assert_eq!(device.scopes, "read,stream");

        let approved = client
            .get(format!("{base}/api/pair/{pair_id}"))
            .send()
            .await
            .unwrap();
        let cookie = approved
            .headers()
            .get("set-cookie")
            .expect("the approval sets the browser cookie")
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let body: serde_json::Value = approved.json().await.unwrap();
        assert_eq!(body["state"], "approved");
        assert!(body["token"].as_str().expect("a token").len() >= 32);

        let library = client
            .get(format!("{base}/api/library"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(library.status(), 200);
        let rows: serde_json::Value = library.json().await.unwrap();

        // The home read is the rails the client draws: a hero, the local rails, and whatever the
        // provider cache holds.
        let home: serde_json::Value = client
            .get(format!("{base}/api/home"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(
            home["hero"].is_object(),
            "a library title is offered as the hero: {home}"
        );
        let rails = home["rails"].as_array().expect("rails");
        assert!(
            rails.iter().any(|rail| rail["id"] == "library"),
            "the library rail is always drawn: {rails:?}"
        );
        assert!(
            home["heroes"]
                .as_array()
                .is_some_and(|heroes| !heroes.is_empty()),
            "the hero rotation is never empty when a hero is drawn"
        );

        assert!(
            rows.as_array().expect("an array").len() >= 2,
            "the two seeded titles are listed: {rows}"
        );

        // My List, search, browse, and genres are the four reads the discovery tabs need.
        let list = client
            .get(format!("{base}/api/watchlist"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(list.status(), 200);
        let search = client
            .get(format!("{base}/api/search?q=Interstellar"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(search.status(), 200);
        let browse: serde_json::Value = client
            .get(format!("{base}/api/browse?kind=movie"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            browse["source"], "library",
            "no provider falls back locally"
        );
        assert!(
            browse["items"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
        );
        let genres = client
            .get(format!("{base}/api/genres?kind=movie"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(genres.status(), 200);
        let downloads = client
            .get(format!("{base}/api/downloads"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(downloads.status(), 200);

        // The default pairing cannot acquire; the same read says so, and the write refuses.
        let me: serde_json::Value = client
            .get(format!("{base}/api/me"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(me["canAcquire"], false);
        let refused_acquire = client
            .post(format!("{base}/api/acquire"))
            .header("cookie", &cookie)
            .json(&serde_json::json!({ "titleId": direct_id }))
            .send()
            .await
            .unwrap();
        assert_eq!(refused_acquire.status(), 403);

        // A title nobody has played has no Resume control; after one report it has the position
        // the desktop would also read from the same row.
        let fresh: serde_json::Value = client
            .get(format!("{base}/api/title/{direct_id}"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(fresh["progress"].is_null(), "nothing has been watched yet");
        assert_eq!(fresh["inLibrary"], true);
        assert!(
            fresh["episodes"].as_array().is_some() && fresh["similar"].as_array().is_some(),
            "the title page always carries the two rail arrays"
        );
        let from_desktop: serde_json::Value = client
            .get(format!("{base}/api/title/{hls_id}"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            from_desktop["progress"]["positionMs"], 30_000,
            "a desktop position is offered as Resume on the TV"
        );
        let written = client
            .post(format!("{base}/api/progress"))
            .header("cookie", &cookie)
            .json(&serde_json::json!({
                "titleId": direct_id,
                "episodeId": null,
                "positionMs": 60_000,
                "durationMs": 7_200_000
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(written.status(), 204);
        let resumed: serde_json::Value = client
            .get(format!("{base}/api/title/{direct_id}"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(resumed["progress"]["positionMs"], 60_000);
        assert_eq!(resumed["progress"]["durationMs"], 7_200_000);

        let wrong_episode = client
            .post(format!("{base}/api/progress"))
            .header("cookie", &cookie)
            .json(&serde_json::json!({
                "titleId": direct_id,
                "episodeId": 999,
                "positionMs": 1_000,
                "durationMs": 7_200_000
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            wrong_episode.status(),
            404,
            "a report cannot name an episode this title has no file for"
        );

        // The direct ticket carries the resume position and a URL that answers a byte range.
        let ticket: serde_json::Value = client
            .post(format!("{base}/api/playback/{direct_id}"))
            .header("cookie", &cookie)
            .json(&serde_json::json!({ "positionMs": 60_000 }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(ticket["kind"], "direct");
        assert_eq!(ticket["startPositionMs"], 60_000);
        assert_eq!(ticket["durationMs"], 7_200_000);
        let media_url = ticket["url"].as_str().expect("a media URL");
        let ranged = client
            .get(format!("{base}{media_url}"))
            .header("cookie", &cookie)
            .header("range", "bytes=0-15")
            .send()
            .await
            .unwrap();
        assert_eq!(ranged.status(), 206);
        assert!(ranged.headers().get("content-range").is_some());
        let stopped = client
            .post(format!("{base}/api/playback/stop"))
            .header("cookie", &cookie)
            .json(&serde_json::json!({ "url": media_url }))
            .send()
            .await
            .unwrap();
        assert_eq!(stopped.status(), 204);
        let gone = client
            .get(format!("{base}{media_url}"))
            .header("cookie", &cookie)
            .header("range", "bytes=0-15")
            .send()
            .await
            .unwrap();
        assert_eq!(gone.status(), 404, "a stopped grant is gone");

        // A container the webview does not demux goes to the transcoder; a fixture whose bytes are
        // not a film is refused with a named failure rather than served as a broken element.
        let transcode = client
            .post(format!("{base}/api/playback/{hls_id}"))
            .header("cookie", &cookie)
            .json(&serde_json::json!({ "positionMs": 0 }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            transcode.status(),
            503,
            "a file ffmpeg cannot read is refused"
        );

        let missing = client
            .get(format!("{base}/api/title/999999"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(missing.status(), 404, "an unknown title is not found");
        let no_file = client
            .post(format!("{base}/api/playback/999999"))
            .header("cookie", &cookie)
            .json(&serde_json::json!({ "positionMs": 0 }))
            .send()
            .await
            .unwrap();
        assert_eq!(no_file.status(), 404, "an unknown title cannot be played");

        // The client page is the same origin as the API, and it carries the policy the listener
        // promises: no inline script, no frame, no foreign origin.
        if service.status().await.running {
            let page = client.get(format!("{base}/")).send().await.unwrap();
            if page.status() == 200 {
                assert!(page.headers().get("content-security-policy").is_some());
                assert!(page.text().await.unwrap().contains("Peregrine"));
            }
        }

        let foreign = client
            .get(format!("{base}/api/library"))
            .header("host", "evil.example")
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(foreign.status(), 403, "a foreign Host is refused");

        assert!(service.revoke(device.id).await.unwrap());
        let after = client
            .get(format!("{base}/api/library"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(after.status(), 401, "a revoked token is refused at once");

        // A refused request is terminal, and one address cannot ask without a limit.
        let second: serde_json::Value = client
            .post(format!("{base}/api/pair"))
            .json(&serde_json::json!({ "name": "Second TV" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let second_id = second["pairId"].as_str().expect("a pair id").to_owned();
        service.deny(&second_id).unwrap();
        let denied: serde_json::Value = client
            .get(format!("{base}/api/pair/{second_id}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(denied["state"], "denied");

        for _ in 0..3 {
            let allowed = client
                .post(format!("{base}/api/pair"))
                .json(&serde_json::json!({ "name": "Another TV" }))
                .send()
                .await
                .unwrap();
            assert_eq!(allowed.status(), 200);
        }
        let limited = client
            .post(format!("{base}/api/pair"))
            .json(&serde_json::json!({ "name": "One too many" }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            limited.status(),
            429,
            "an address cannot ask without a limit"
        );

        service.stop().await;
    }
}
