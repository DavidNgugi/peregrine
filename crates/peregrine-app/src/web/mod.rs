//! The LAN surface: a read-only HTTP API, the pairing ceremony, and the client it serves.
//!
//! ADR 0028 is the decision this module implements, and the shape of the module is the decision:
//! the listener is off until somebody starts it, it binds one address, and the service below is
//! the only thing a route can ask. There is no path from a route to the IPC command surface, so a
//! token cannot acquire, delete, or change a setting -- not because a handler refuses, but because
//! no handler exists.
//!
//! **The pairing token is stored as a hash and delivered once.** A pair request produces an
//! eight-digit code and a secret request id; the desktop approves the code, and the approved
//! request answers the token exactly once. The database keeps sha256(token), so a copy of the
//! database is a device list rather than a set of credentials.
//!
//! **A browser authenticates with an `HttpOnly` cookie.** Images and video are fetched by elements
//! that cannot attach an Authorization header, so the token a paired browser receives is set as a
//! SameSite=Strict cookie scoped to the server's own origin. Tooling may present the same token as
//! a bearer header; both paths end at [`WebService::authenticate`].
//!
//! **Playback is a second, shorter credential.** The device token mints a playback session bound
//! to the device and one file for two hours; the media URL carries that session, not the device
//! token. Revoking the device drops its sessions.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::net::{IpAddr, SocketAddr};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use peregrine_domain::catalogue::TitleKind;
use peregrine_domain::{
    Clock, DownloadState, LocalPlayback, PlaybackPreference, SafePath, SessionToken,
    SubtitleFormat, SubtitleMode, TokenSource,
};
use peregrine_platform::{Inhibitor, PowerSave};
use peregrine_providers::{BrowseRequest, BrowseSort};
use peregrine_storage::{Database, NewPairedDevice, PairedDevice, StorageError};
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::Runtime;
use crate::events::{Event, EventBus, WEB_PAIR_REQUESTED};

mod routes;
mod transcode;

/// How long a pairing code waits for a person, in milliseconds.
const PAIR_TTL_MS: i64 = 5 * 60 * 1_000;
/// The window a single address may request pairings in, in milliseconds.
const PAIR_REQUEST_WINDOW_MS: i64 = 60 * 1_000;
/// How many pairing requests one address may make inside the window.
const PAIR_REQUEST_LIMIT: u32 = 5;
/// How many requests may wait for a person at once.
const PENDING_CAPACITY: usize = 16;
/// How long a playback session lives, in milliseconds.
const PLAYBACK_TTL_MS: i64 = 6 * 60 * 60 * 1_000;
/// How many transcodes may run at once. One device holds one; this bounds the host.
const HLS_SESSION_LIMIT: usize = 4;
/// How often a device's last-seen stamp is written at most, in milliseconds.
const LAST_SEEN_INTERVAL_MS: i64 = 60 * 1_000;
/// The scopes a device receives unless its approval also grants acquisition.
pub const SCOPES: &str = "read,stream";
/// The scope that lets a paired device ask the core to start a download. It is never a default.
pub const ACQUIRE_SCOPE: &str = "acquire";
/// The scopes an approval with the "Allow downloads" box ticked issues.
pub const ACQUIRING_SCOPES: &str = "read,stream,acquire";
/// The cookie a paired browser carries.
pub const TOKEN_COOKIE: &str = "peregrine_token";

/// Where the listener binds and what it may serve.
#[derive(Debug, Clone)]
pub struct WebConfig {
    /// The address to bind, which must not be unspecified.
    pub bind: IpAddr,
    /// The port to bind; zero asks the operating system for one.
    pub port: u16,
    /// The built client's directory, when one is installed.
    pub app_dir: Option<PathBuf>,
    /// An optional hostname that is accepted besides the bound address.
    pub hostname: Option<String>,
}

impl WebConfig {
    /// A configuration for one address and port.
    #[must_use]
    pub const fn new(bind: IpAddr, port: u16) -> Self {
        Self {
            bind,
            port,
            app_dir: None,
            hostname: None,
        }
    }

    /// Serves a built client from `directory`.
    #[must_use]
    pub fn with_app_dir(mut self, directory: impl Into<PathBuf>) -> Self {
        self.app_dir = Some(directory.into());
        self
    }

    /// Accepts `hostname` as a Host value besides the bound address.
    #[must_use]
    pub fn with_hostname(mut self, hostname: impl Into<String>) -> Self {
        self.hostname = Some(hostname.into());
        self
    }
}

/// What the listener is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebStatus {
    /// Whether a socket is bound.
    pub running: bool,
    /// Where it is bound, when it is.
    pub address: Option<SocketAddr>,
    /// The hostname the listener also answers to, when one was configured.
    ///
    /// A name is what keeps a browser's pairing cookie valid across an address change: the cookie
    /// belongs to the origin, and a name is one origin where two addresses are two.
    pub hostname: Option<String>,
    /// Whether the person asked for the machine to stay awake while the surface is on.
    pub keep_awake: bool,
    /// How many pairing requests are waiting for a person.
    pub pending: usize,
}

/// Why the LAN surface refused.
#[derive(Debug, Error)]
pub enum WebError {
    /// The unspecified address is never a bind: it exposes every interface at once.
    #[error("the LAN surface must bind one address, not every interface")]
    UnspecifiedBind,
    /// A second start while one listener is live.
    #[error("the LAN surface is already listening")]
    AlreadyRunning,
    /// The socket could not be bound.
    #[error("the LAN surface could not bind {address}")]
    Bind {
        /// The address it tried.
        address: SocketAddr,
        /// What the operating system said.
        #[source]
        source: std::io::Error,
    },
    /// The database refused a read or a write.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// The core refused a read.
    #[error(transparent)]
    Runtime(#[from] crate::RuntimeError),
    /// No request holds that pair id.
    #[error("the pairing request is not known")]
    PairUnknown,
    /// The request expired before a person answered.
    #[error("the pairing request has expired")]
    PairExpired,
    /// The request is not waiting for approval, because it was used, denied, or already approved.
    #[error("the pairing request is not waiting for approval")]
    PairNotPending,
    /// One address asked too often.
    #[error("too many pairing requests from this address")]
    RateLimited,
    /// The pending list is full.
    #[error("too many pairing requests are waiting")]
    PairingFull,
    /// The token is not one this server issued, or the device was revoked.
    #[error("the token is not valid")]
    Unauthorized,
    /// The title has no playable file.
    #[error("the title has no playable file")]
    NoFile,
    /// The file needs a transcode this build cannot perform.
    #[error("the file needs a transcode this build does not have")]
    TranscodeRequired,
    /// The container is not one a browser demuxes.
    #[error("the container is not playable in a browser")]
    Unsupported,
    /// This host has no encoder the build may use for a transcode.
    #[error("this host has no H.264 encoder available for a transcode")]
    TranscodeUnavailable,
    /// A progress report named an episode the playable file is not.
    #[error("the progress report does not match the title's playable file")]
    InvalidProgress,
    /// Too many transcodes are already running on this host.
    #[error("too many transcodes are running")]
    TranscodeBusy,
    /// The title is not in the library, or this scope may not see it.
    #[error("the title is not in the library")]
    UnknownTitle,
    /// The episode is not in the library, or this scope may not see it.
    #[error("the episode is not in the library")]
    UnknownEpisode,
    /// The device was not approved to start downloads.
    #[error("this device may not start downloads")]
    NotPermitted,
    /// The request named an episode incompletely or an impossible season.
    #[error("the download request is not valid")]
    InvalidRequest,
    /// A device name was blank, too long, or carried a control character.
    #[error("the device name is not valid")]
    InvalidName,
    /// The configured hostname is not one a browser can put in a Host header.
    #[error("the hostname is not valid")]
    InvalidHostname,
    /// A row the insert just wrote could not be read back.
    #[error("the paired device could not be read back")]
    DeviceMissing,
}

/// One file a browser may be handed directly.
#[derive(Debug, Clone)]
pub struct WebPlaybackTarget {
    /// The guarded path the core resolved.
    pub path: PathBuf,
    /// The file size, for the range answer.
    pub size_bytes: u64,
    /// What this build would do with the container.
    pub playback: LocalPlayback,
    /// The episode this file is, when the library holds a series.
    pub episode_id: Option<i64>,
    /// The runtime the probe recorded, when it recorded one.
    pub duration_ms: Option<i64>,
}

/// A pairing request as the desktop lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingPairView {
    /// The secret the TV polls with.
    pub pair_id: String,
    /// The code the person compares with the screen.
    pub code: String,
    /// The name the client offered.
    pub name: String,
    /// Its platform, when it named one.
    pub platform: Option<String>,
    /// The address it came from.
    pub address: Option<String>,
    /// Unix milliseconds.
    pub created_at_ms: i64,
    /// Unix milliseconds.
    pub expires_at_ms: i64,
}

/// What a pairing request answers when it is created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairOffer {
    /// The secret the TV polls with.
    pub pair_id: String,
    /// The code shown on the TV and compared on the desktop.
    pub code: String,
    /// Unix milliseconds.
    pub expires_at_ms: i64,
}

/// The state a pairing request is in, as the TV polls it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairStatus {
    /// Waiting for a person.
    Pending {
        /// Unix milliseconds.
        expires_at_ms: i64,
    },
    /// Approved; the token is returned exactly once.
    Approved {
        /// The token, on the first poll only.
        token: Option<String>,
        /// The device name the person approved.
        name: String,
    },
    /// Refused by a person.
    Denied,
    /// No request holds that id.
    Unknown,
}

/// A playback URL minted for one device and one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackTicket {
    /// `direct` for a file the browser demuxes, `hls` for one this host transcodes.
    pub kind: &'static str,
    /// The path to play, relative to this origin.
    pub url: String,
    /// The content type the response will carry.
    pub mime: String,
    /// Where the timeline starts, in milliseconds: a transcode resumed at an offset starts at zero.
    pub start_position_ms: i64,
    /// The title's runtime, when the probe recorded one.
    pub duration_ms: Option<i64>,
    /// The episode the session plays, for a series' progress rows.
    pub episode_id: Option<i64>,
    /// `movie` or `show`, so the player knows whether a next episode can exist.
    pub title_kind: &'static str,
    /// The title's display name, for the next-up line.
    pub title_name: String,
    /// The sidecar subtitle tracks this session can serve, default already chosen.
    pub subtitles: Vec<SubtitleTrackDto>,
    /// The playable episode after this one, for autoplay.
    pub next_episode: Option<NextEpisodeDto>,
    /// The audio language the host selected for a transcode, when it selected one.
    pub audio_language: Option<String>,
}

/// One sidecar subtitle track, as the player draws and fetches it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleTrackDto {
    /// The track row id.
    pub id: i64,
    /// The language tag the row carries.
    pub language: String,
    /// The label to show instead of the language, when the row gave one.
    pub label: Option<String>,
    /// The sidecar's container.
    pub format: String,
    /// Whether the track only covers forced narrative.
    pub forced: bool,
    /// Whether the track carries descriptions for the hard of hearing.
    pub sdh: bool,
    /// The track this device's preferences choose, so the player starts on it.
    pub r#default: bool,
    /// The `WebVTT` URL the player fetches this track from.
    pub url: String,
}

/// The episode that follows the one a playback ticket opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NextEpisodeDto {
    /// The season the next episode sits in.
    pub season_number: u32,
    /// Its number within that season.
    pub episode_number: u32,
    /// The episode row id a new ticket names.
    pub episode_id: i64,
    /// Its name, when a provider filled one in.
    pub name: Option<String>,
}

/// The account's playback language preference, as the web client reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreferencesDto {
    /// The preferred audio language.
    pub audio_language: String,
    /// The preferred subtitle language.
    pub subtitle_language: String,
    /// `auto`, `off`, or `forced`.
    pub subtitles: String,
}

/// What a season acquisition queued, and what it could not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeasonAcquireDto {
    /// Episodes the queue accepted.
    pub queued: u32,
    /// Episodes whose file was already on disk.
    pub already_playable: u32,
    /// Episodes the queue refused.
    pub failed: u32,
}

/// A subtitle track's metadata without its path, which never leaves the core (AGENTS R11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSubtitleTrack {
    /// The track row id.
    pub id: i64,
    /// The language tag the row carries.
    pub language: String,
    /// The label to show instead of the language, when the row gave one.
    pub label: Option<String>,
    /// The sidecar's container.
    pub format: String,
    /// Whether the track only covers forced narrative.
    pub forced: bool,
    /// Whether the track carries descriptions for the hard of hearing.
    pub sdh: bool,
    /// Whether the row itself is preferred when no language matches.
    pub is_default: bool,
}

/// What one title's download jobs say, keyed the way its page draws them.
#[derive(Debug, Default)]
struct TitleDownloads {
    title: Option<JobDownload>,
    episodes: HashMap<(u32, u32), JobDownload>,
}

/// One job's lifecycle state and completion.
#[derive(Debug, Clone)]
struct JobDownload {
    state: String,
    percent: f64,
}

impl TitleDownloads {
    /// The state a title page shows when no single job covers the whole title.
    ///
    /// A series the user downloaded episode by episode has no title-level job, so its page
    /// aggregates the listed season's jobs: the first one still doing something wins, and a
    /// season whose every listed episode completed reads `completed`. A failed or removed job
    /// is neither active nor done, so it contributes nothing to the summary.
    fn summary(&self, is_show: bool, episodes: &[EpisodeDto]) -> (Option<String>, Option<f64>) {
        if let Some(title) = &self.title {
            return (Some(title.state.clone()), Some(title.percent));
        }
        if !is_show || episodes.is_empty() {
            return (None, None);
        }
        let jobs: Vec<&JobDownload> = episodes
            .iter()
            .filter_map(|episode| {
                self.episodes
                    .get(&(episode.season_number, episode.episode_number))
            })
            .collect();
        if let Some(active) = jobs
            .iter()
            .find(|job| DownloadState::parse(&job.state).is_some_and(|state| !state.is_terminal()))
        {
            return (Some(active.state.clone()), Some(active.percent));
        }
        let all_done = episodes.iter().all(|episode| {
            self.episodes
                .get(&(episode.season_number, episode.episode_number))
                .is_some_and(|job| job.state == DownloadState::Completed.as_str())
        });
        if all_done {
            return (
                Some(DownloadState::Completed.as_str().to_owned()),
                Some(100.0),
            );
        }
        (None, None)
    }
}

/// Where a subtitle track is served from and how it was encoded.
type SubtitleSource = (SafePath, SubtitleFormat);

/// A media request the server will answer, resolved from its playback token.
#[derive(Debug, Clone)]
pub struct MediaGrant {
    /// The device that minted it.
    pub device_id: i64,
    /// The guarded path.
    pub path: PathBuf,
    /// The file size.
    pub size_bytes: u64,
    /// The content type.
    pub mime: &'static str,
}

#[derive(Debug)]
struct PendingPair {
    code: String,
    name: String,
    platform: Option<String>,
    address: Option<String>,
    created_at_ms: i64,
    expires_at_ms: i64,
    state: PairState,
    token: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PairState {
    Pending,
    Approving,
    Approved,
    Denied,
}

#[derive(Debug, Default)]
struct Pairing {
    by_id: HashMap<String, PendingPair>,
    by_code: HashMap<String, String>,
    requests: HashMap<IpAddr, (i64, u32)>,
}

impl Pairing {
    fn purge(&mut self, now_ms: i64) {
        self.by_id.retain(|_, pair| pair.expires_at_ms > now_ms);
        let live = &self.by_id;
        self.by_code.retain(|_, id| live.contains_key(id));
        self.requests
            .retain(|_, (started, _)| now_ms - *started < PAIR_REQUEST_WINDOW_MS);
    }

    fn waiting(&self) -> usize {
        self.by_id
            .values()
            .filter(|pair| matches!(pair.state, PairState::Pending | PairState::Approving))
            .count()
    }
}

#[derive(Debug)]
struct HlsSession {
    dir: PathBuf,
    child: tokio::process::Child,
    device_id: i64,
    expires_at_ms: i64,
}

/// What a started transcode hands the ticket it was minted for.
struct StartedHls {
    url: String,
    audio_language: Option<String>,
}

/// A live HLS session, as the route that serves it needs it.
#[derive(Debug, Clone)]
pub struct HlsGrant {
    /// The device that started it.
    pub device_id: i64,
    /// The directory its playlist and segments live in.
    pub dir: PathBuf,
}

#[derive(Debug, Clone)]
struct Grant {
    device_id: i64,
    path: PathBuf,
    size_bytes: u64,
    mime: &'static str,
    expires_at_ms: i64,
}

struct Running {
    address: SocketAddr,
    hostname: Option<String>,
    app_dir: Option<PathBuf>,
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

/// How long after the last media request the playback assertion is released.
///
/// Long enough to cover the gap between HLS segments and a paused player's next range request,
/// short enough that a film that ended does not keep the machine awake for an hour.
const STREAM_POWER_WINDOW_MS: i64 = 60_000;

/// What the surface is holding awake, and why.
#[derive(Default)]
struct PowerState {
    /// The person's switch: hold the surface's assertion while the listener runs.
    keep_awake: bool,
    /// Held while the listener runs and `keep_awake` is on.
    listening: Option<Box<dyn Inhibitor>>,
    /// Held while a device is asking for media.
    streaming: Option<Box<dyn Inhibitor>>,
    /// The last media request, in Unix milliseconds.
    last_stream_ms: i64,
}

/// The LAN surface, its pairing registry, and its playback grants.
///
/// One value rather than three, because all three die together: stopping the listener clears the
/// grants, and a device revoked while a film is playing must fail its next range request.
pub struct WebService {
    runtime: Arc<Runtime>,
    transcode_root: PathBuf,
    database: Arc<Database>,
    bus: Arc<EventBus>,
    artwork: Arc<peregrine_sync::ImageCache>,
    clock: Arc<dyn Clock + Send + Sync>,
    tokens: Arc<dyn TokenSource + Send + Sync>,
    pairing: Mutex<Pairing>,
    sessions: Mutex<HashMap<String, Grant>>,
    hls: tokio::sync::Mutex<HashMap<String, HlsSession>>,
    transcoder: tokio::sync::Mutex<Option<Arc<transcode::Transcoder>>>,
    seen: Mutex<HashMap<i64, i64>>,
    hosts: Mutex<Vec<String>>,
    power: Arc<dyn PowerSave>,
    power_state: Mutex<PowerState>,
    running: tokio::sync::Mutex<Option<Running>>,
    refresh_started: Mutex<HashSet<i64>>,
}

impl WebService {
    /// A service over one runtime. It does not listen until [`WebService::start`] is called.
    #[must_use]
    pub fn new(
        runtime: Arc<Runtime>,
        database: Arc<Database>,
        bus: Arc<EventBus>,
        artwork: Arc<peregrine_sync::ImageCache>,
        clock: Arc<dyn Clock + Send + Sync>,
        tokens: Arc<dyn TokenSource + Send + Sync>,
        transcode_root: PathBuf,
    ) -> Self {
        let power = runtime.power();
        Self {
            runtime,
            transcode_root,
            database,
            bus,
            artwork,
            clock,
            tokens,
            pairing: Mutex::new(Pairing::default()),
            sessions: Mutex::new(HashMap::new()),
            hls: tokio::sync::Mutex::new(HashMap::new()),
            transcoder: tokio::sync::Mutex::new(None),
            seen: Mutex::new(HashMap::new()),
            hosts: Mutex::new(Vec::new()),
            power,
            power_state: Mutex::new(PowerState::default()),
            running: tokio::sync::Mutex::new(None),
            refresh_started: Mutex::new(HashSet::new()),
        }
    }

    fn now(&self) -> i64 {
        self.clock.now().as_millisecond()
    }

    /// Starts the listener, refusing the unspecified address and a second start.
    ///
    /// # Errors
    ///
    /// [`WebError::UnspecifiedBind`], [`WebError::AlreadyRunning`], or [`WebError::Bind`].
    pub async fn start(self: &Arc<Self>, config: WebConfig) -> Result<WebStatus, WebError> {
        if config.bind.is_unspecified() {
            return Err(WebError::UnspecifiedBind);
        }
        let mut running = self.running.lock().await;
        if running.is_some() {
            return Err(WebError::AlreadyRunning);
        }
        // A previous process can leave a transcode directory behind. Nothing from it is
        // addressable by this instance, and those directories are the largest thing the surface
        // writes, so a start clears them instead of accumulating gigabytes across launches.
        self.clear_transcode_root().await;
        let next = self.listen(&config).await?;
        *self
            .hosts
            .lock()
            .expect("the host list lock is not poisoned") =
            allowed_hosts(next.address, next.hostname.as_deref());
        let address = next.address;
        *running = Some(next);
        drop(running);
        self.sync_listening(true);
        info!(%address, "the LAN surface is listening");
        Ok(self.status().await)
    }

    /// Moves the listener to another address on the same port.
    ///
    /// The new socket is bound before the old one closes, so a failed bind — an address the OS has
    /// not finished assigning, or a port another process took in the meantime — leaves the surface
    /// where it was instead of taking it down. A listener that is not running is left alone.
    ///
    /// # Errors
    ///
    /// [`WebError::UnspecifiedBind`] for the address every interface shares, or [`WebError::Bind`]
    /// when the new socket cannot be opened.
    pub async fn rebind(self: &Arc<Self>, bind: IpAddr) -> Result<WebStatus, WebError> {
        if bind.is_unspecified() {
            return Err(WebError::UnspecifiedBind);
        }
        let mut running = self.running.lock().await;
        let Some(current) = running.as_ref() else {
            drop(running);
            return Ok(self.status().await);
        };
        if current.address.ip() == bind {
            drop(running);
            return Ok(self.status().await);
        }
        let config = WebConfig {
            bind,
            port: current.address.port(),
            app_dir: current.app_dir.clone(),
            hostname: current.hostname.clone(),
        };
        let next = self.listen(&config).await?;
        *self
            .hosts
            .lock()
            .expect("the host list lock is not poisoned") =
            allowed_hosts(next.address, next.hostname.as_deref());
        let previous = running
            .replace(next)
            .expect("the listener was checked above");
        let address = previous.address;
        let moved_to = running.as_ref().map(|running| running.address);
        drop(running);
        previous.shutdown.cancel();
        let _ = previous.task.await;
        info!(from = %address, to = ?moved_to, "the LAN surface moved to the machine's new address");
        Ok(self.status().await)
    }

    /// Opens one listener and hands back the handle that owns it.
    async fn listen(self: &Arc<Self>, config: &WebConfig) -> Result<Running, WebError> {
        if let Some(hostname) = config.hostname.as_deref()
            && !is_hostname(hostname)
        {
            return Err(WebError::InvalidHostname);
        }
        let wanted = SocketAddr::new(config.bind, config.port);
        let listener = TcpListener::bind(wanted)
            .await
            .map_err(|source| WebError::Bind {
                address: wanted,
                source,
            })?;
        let address = listener.local_addr().map_err(|source| WebError::Bind {
            address: wanted,
            source,
        })?;
        let router = routes::router(Arc::clone(self), config.app_dir.clone());
        let shutdown = CancellationToken::new();
        let token = shutdown.clone();
        let task = tokio::spawn(async move {
            let served = axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async move { token.cancelled().await })
            .await;
            if let Err(error) = served {
                warn!(%error, "the LAN listener stopped with an error");
            }
        });
        Ok(Running {
            address,
            hostname: config.hostname.clone(),
            app_dir: config.app_dir.clone(),
            shutdown,
            task,
        })
    }

    async fn clear_transcode_root(&self) {
        let Ok(mut entries) = tokio::fs::read_dir(&self.transcode_root).await else {
            return;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_dir() {
                let _ = tokio::fs::remove_dir_all(&path).await;
            }
        }
    }

    /// Stops the listener and drops every playback grant.
    pub async fn stop(&self) {
        let held = self.running.lock().await.take();
        if let Some(running) = held {
            running.shutdown.cancel();
            let _ = running.task.await;
            self.sessions
                .lock()
                .expect("the session lock is not poisoned")
                .clear();
            self.evict_all_hls().await;
            self.hosts
                .lock()
                .expect("the host list lock is not poisoned")
                .clear();
            self.release_power();
            info!("the LAN surface stopped");
        }
    }

    /// What the listener is doing.
    pub async fn status(&self) -> WebStatus {
        let running = self.running.lock().await;
        let pending = self
            .pairing
            .lock()
            .expect("the pairing lock is not poisoned")
            .waiting();
        let keep_awake = self
            .power_state
            .lock()
            .expect("the power state lock is not poisoned")
            .keep_awake;
        WebStatus {
            running: running.is_some(),
            address: running.as_ref().map(|running| running.address),
            hostname: running
                .as_ref()
                .and_then(|running| running.hostname.clone()),
            keep_awake,
            pending,
        }
    }

    /// Turns the person's keep-awake switch on or off and answers the new status.
    ///
    /// Playback holds its own assertion regardless: a stream that dies because the host slept is
    /// a defect, not a preference.
    pub async fn set_keep_awake(&self, enabled: bool) -> WebStatus {
        self.power_state
            .lock()
            .expect("the power state lock is not poisoned")
            .keep_awake = enabled;
        let running = self.running.lock().await.is_some();
        self.sync_listening(running);
        self.status().await
    }

    /// Releases the playback assertion once no device has asked for media for a minute.
    ///
    /// Called from the shell's address watcher, which already wakes on a fixed interval; the
    /// policy stays here so a second host does not have to reimplement the window.
    pub fn tick_power(&self) {
        let now = self.now();
        let mut state = self
            .power_state
            .lock()
            .expect("the power state lock is not poisoned");
        if state.streaming.is_some()
            && now.saturating_sub(state.last_stream_ms) >= STREAM_POWER_WINDOW_MS
        {
            state.streaming = None;
        }
    }

    /// Holds or releases the surface's own assertion to match `keep_awake` and `running`.
    fn sync_listening(&self, running: bool) {
        let mut state = self
            .power_state
            .lock()
            .expect("the power state lock is not poisoned");
        let wanted = running && state.keep_awake;
        if wanted && state.listening.is_none() {
            state.listening = Some(self.power.inhibit("Web access is on"));
        } else if !wanted {
            state.listening = None;
        }
    }

    /// Records a media request and holds the playback assertion if it is not held already.
    fn touch_stream(&self) {
        let now = self.now();
        let mut state = self
            .power_state
            .lock()
            .expect("the power state lock is not poisoned");
        state.last_stream_ms = now;
        if state.streaming.is_none() {
            state.streaming = Some(self.power.inhibit("A device is streaming"));
        }
    }

    /// Drops both assertions; the listener is going away.
    fn release_power(&self) {
        let mut state = self
            .power_state
            .lock()
            .expect("the power state lock is not poisoned");
        state.listening = None;
        state.streaming = None;
    }

    /// Whether a Host value names this listener.
    #[must_use]
    pub fn host_allowed(&self, host: &str) -> bool {
        self.hosts
            .lock()
            .expect("the host list lock is not poisoned")
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(host))
    }

    /// Whether an Origin header is this listener's own.
    #[must_use]
    pub fn origin_allowed(&self, origin: &str) -> bool {
        let Some(rest) = origin.strip_prefix("http://") else {
            return false;
        };
        self.host_allowed(rest)
    }

    /// Creates a pairing request, publishing it to the desktop.
    ///
    /// # Errors
    ///
    /// [`WebError::RateLimited`] or [`WebError::PairingFull`].
    pub fn request_pair(
        &self,
        address: IpAddr,
        name: &str,
        platform: Option<&str>,
    ) -> Result<PairOffer, WebError> {
        let now_ms = self.now();
        let mut pairing = self
            .pairing
            .lock()
            .expect("the pairing lock is not poisoned");
        pairing.purge(now_ms);
        {
            let window = pairing.requests.entry(address).or_insert((now_ms, 0));
            if now_ms - window.0 >= PAIR_REQUEST_WINDOW_MS {
                *window = (now_ms, 0);
            }
            if window.1 >= PAIR_REQUEST_LIMIT {
                return Err(WebError::RateLimited);
            }
            window.1 += 1;
        }
        if pairing.by_id.len() >= PENDING_CAPACITY {
            return Err(WebError::PairingFull);
        }
        let Some((pair_id, code)) = self.mint_pair(&pairing) else {
            return Err(WebError::PairingFull);
        };
        let expires_at_ms = now_ms + PAIR_TTL_MS;
        pairing.by_id.insert(
            pair_id.clone(),
            PendingPair {
                code: code.clone(),
                name: name.to_owned(),
                platform: platform.map(str::to_owned),
                address: Some(address.to_string()),
                created_at_ms: now_ms,
                expires_at_ms,
                state: PairState::Pending,
                token: None,
            },
        );
        pairing.by_code.insert(code.clone(), pair_id.clone());
        drop(pairing);
        self.bus.publish(Event::new(
            WEB_PAIR_REQUESTED,
            pair_id.clone(),
            serde_json::json!({
                "pairId": pair_id.clone(),
                "code": code.clone(),
                "name": name,
                "platform": platform,
                "address": address.to_string(),
                "expiresAtMs": expires_at_ms,
            }),
        ));
        info!(%address, "a device asked to pair with the LAN surface");
        Ok(PairOffer {
            pair_id,
            code,
            expires_at_ms,
        })
    }

    fn mint_pair(&self, pairing: &Pairing) -> Option<(String, String)> {
        for _ in 0..16 {
            let pair_id = self.tokens.next_token();
            let code = code_for(&pair_id);
            if pairing.by_id.contains_key(pair_id.as_str()) || pairing.by_code.contains_key(&code) {
                continue;
            }
            return Some((pair_id.as_str().to_owned(), code));
        }
        None
    }

    /// The pairing requests waiting for a person.
    pub fn pending_pairs(&self) -> Vec<PendingPairView> {
        let now_ms = self.now();
        let mut pairing = self
            .pairing
            .lock()
            .expect("the pairing lock is not poisoned");
        pairing.purge(now_ms);
        let mut waiting: Vec<_> = pairing
            .by_id
            .iter()
            .filter(|(_, pair)| matches!(pair.state, PairState::Pending | PairState::Approving))
            .map(|(id, pair)| PendingPairView {
                pair_id: id.clone(),
                code: pair.code.clone(),
                name: pair.name.clone(),
                platform: pair.platform.clone(),
                address: pair.address.clone(),
                created_at_ms: pair.created_at_ms,
                expires_at_ms: pair.expires_at_ms,
            })
            .collect();
        waiting.sort_by_key(|pair| pair.created_at_ms);
        waiting
    }

    /// The state one request is in, taking the token on the first approved poll.
    pub fn pair_status(&self, pair_id: &str) -> PairStatus {
        let now_ms = self.now();
        let mut pairing = self
            .pairing
            .lock()
            .expect("the pairing lock is not poisoned");
        pairing.purge(now_ms);
        let Some(pair) = pairing.by_id.get_mut(pair_id) else {
            return PairStatus::Unknown;
        };
        match pair.state {
            PairState::Pending | PairState::Approving => PairStatus::Pending {
                expires_at_ms: pair.expires_at_ms,
            },
            PairState::Denied => PairStatus::Denied,
            PairState::Approved => PairStatus::Approved {
                token: pair.token.take(),
                name: pair.name.clone(),
            },
        }
    }

    /// Approves one request by its secret id.
    ///
    /// `allow_acquire` is the desktop's explicit **Allow downloads** decision; every other
    /// approval issues the default `read,stream` only.
    ///
    /// # Errors
    ///
    /// [`WebError::PairUnknown`], [`WebError::PairExpired`], [`WebError::PairNotPending`], or a
    /// storage failure.
    pub async fn approve(
        &self,
        pair_id: &str,
        allow_acquire: bool,
    ) -> Result<PairedDevice, WebError> {
        let now_ms = self.now();
        let (name, platform, address) = {
            let mut pairing = self
                .pairing
                .lock()
                .expect("the pairing lock is not poisoned");
            pairing.purge(now_ms);
            let pair = pairing
                .by_id
                .get_mut(pair_id)
                .ok_or(WebError::PairUnknown)?;
            if pair.expires_at_ms <= now_ms {
                return Err(WebError::PairExpired);
            }
            if pair.state != PairState::Pending {
                return Err(WebError::PairNotPending);
            }
            pair.state = PairState::Approving;
            (
                pair.name.clone(),
                pair.platform.clone(),
                pair.address.clone(),
            )
        };
        let token = self.tokens.next_token();
        let inserted = self
            .database
            .insert_paired_device(NewPairedDevice {
                name,
                platform,
                token_hash: hash_token(token.as_str()),
                scopes: if allow_acquire {
                    ACQUIRING_SCOPES.to_owned()
                } else {
                    SCOPES.to_owned()
                },
                address,
                at: now_ms,
            })
            .await;
        match inserted {
            Ok(id) => {
                {
                    let mut pairing = self
                        .pairing
                        .lock()
                        .expect("the pairing lock is not poisoned");
                    if let Some(pair) = pairing.by_id.get_mut(pair_id) {
                        pair.state = PairState::Approved;
                        pair.token = Some(token.as_str().to_owned());
                    }
                }
                info!(device = id, "a device was paired to the LAN surface");
                self.database
                    .paired_device(id)
                    .await?
                    .ok_or(WebError::DeviceMissing)
            }
            Err(error) => {
                let mut pairing = self
                    .pairing
                    .lock()
                    .expect("the pairing lock is not poisoned");
                if let Some(pair) = pairing.by_id.get_mut(pair_id) {
                    pair.state = PairState::Pending;
                }
                Err(WebError::Storage(error))
            }
        }
    }

    /// Approves whichever request shows `code`.
    ///
    /// # Errors
    ///
    /// [`WebError::PairUnknown`] when no request shows the code, or the same errors as
    /// [`WebService::approve`].
    pub async fn approve_code(
        &self,
        code: &str,
        allow_acquire: bool,
    ) -> Result<PairedDevice, WebError> {
        let now_ms = self.now();
        let pair_id = {
            let mut pairing = self
                .pairing
                .lock()
                .expect("the pairing lock is not poisoned");
            pairing.purge(now_ms);
            pairing
                .by_code
                .get(code)
                .cloned()
                .ok_or(WebError::PairUnknown)?
        };
        self.approve(&pair_id, allow_acquire).await
    }

    /// Refuses one request.
    ///
    /// # Errors
    ///
    /// [`WebError::PairUnknown`] when no request holds the id.
    pub fn deny(&self, pair_id: &str) -> Result<(), WebError> {
        let now_ms = self.now();
        let mut pairing = self
            .pairing
            .lock()
            .expect("the pairing lock is not poisoned");
        pairing.purge(now_ms);
        let code = {
            let pair = pairing
                .by_id
                .get_mut(pair_id)
                .ok_or(WebError::PairUnknown)?;
            if pair.state != PairState::Pending {
                return Err(WebError::PairNotPending);
            }
            pair.state = PairState::Denied;
            pair.code.clone()
        };
        pairing.by_code.remove(&code);
        info!("a device's pairing request was refused");
        Ok(())
    }

    /// Every device, revoked ones included, for the desktop list.
    ///
    /// # Errors
    ///
    /// [`WebError::Storage`] when the read fails.
    pub async fn devices(&self) -> Result<Vec<PairedDevice>, WebError> {
        Ok(self.database.paired_devices().await?)
    }

    /// Revokes one device and drops the playback sessions it minted.
    ///
    /// # Errors
    ///
    /// [`WebError::Storage`] when the write fails.
    pub async fn revoke(&self, id: i64) -> Result<bool, WebError> {
        let changed = self.database.revoke_paired_device(id, self.now()).await?;
        if changed {
            self.sessions
                .lock()
                .expect("the session lock is not poisoned")
                .retain(|_, grant| grant.device_id != id);
            self.evict_hls_device(id).await;
            info!(device = id, "a paired device was revoked");
        }
        Ok(changed)
    }

    /// Renames one device, changing the label the list draws and nothing else.
    ///
    /// # Errors
    ///
    /// [`WebError::InvalidName`] for a blank, oversized, or control-character name;
    /// [`WebError::DeviceMissing`] for an id no active row holds; or [`WebError::Storage`].
    pub async fn rename_device(&self, id: i64, name: &str) -> Result<PairedDevice, WebError> {
        let name = device_name(name)?;
        if !self.database.rename_paired_device(id, &name).await? {
            return Err(WebError::DeviceMissing);
        }
        self.database
            .paired_device(id)
            .await?
            .ok_or(WebError::DeviceMissing)
    }

    /// The host's charge, when the host has a battery.
    #[must_use]
    pub fn battery(&self) -> Option<peregrine_platform::BatteryState> {
        peregrine_platform::battery::state()
    }

    /// Authenticates a token and records the device as seen.
    ///
    /// # Errors
    ///
    /// [`WebError::Unauthorized`] for a token this server did not mint, a revoked device, or a
    /// device without the read scope; [`WebError::Storage`] when the lookup fails.
    pub async fn authenticate(&self, presented: &str) -> Result<PairedDevice, WebError> {
        let token = SessionToken::new(presented).map_err(|_| WebError::Unauthorized)?;
        let device = self
            .database
            .paired_device_for_token_hash(&hash_token(token.as_str()))
            .await?
            .ok_or(WebError::Unauthorized)?;
        if device.revoked_at.is_some() || !scope_allows(&device.scopes, "read") {
            return Err(WebError::Unauthorized);
        }
        self.touch(&device).await;
        Ok(device)
    }

    async fn touch(&self, device: &PairedDevice) {
        let now_ms = self.now();
        let due = {
            let mut seen = self.seen.lock().expect("the seen lock is not poisoned");
            match seen.get(&device.id) {
                Some(last) if now_ms - last < LAST_SEEN_INTERVAL_MS => false,
                _ => {
                    seen.insert(device.id, now_ms);
                    true
                }
            }
        };
        if due
            && let Err(error) = self
                .database
                .touch_paired_device(device.id, now_ms, None)
                .await
        {
            warn!(%error, "a device's last-seen stamp could not be written");
        }
    }

    /// The library page a client draws, newest first.
    ///
    /// # Errors
    ///
    /// [`WebError::Runtime`] when the library read fails.
    pub async fn library_page(
        &self,
        after: Option<i64>,
        limit: u32,
    ) -> Result<Vec<LibraryRowDto>, WebError> {
        let entries = self.runtime.library_titles(after, limit).await?;
        Ok(entries
            .into_iter()
            .map(|entry| LibraryRowDto {
                id: entry.row.title.id,
                name: entry.row.title.name.clone(),
                kind: entry.row.title.kind.as_str().to_owned(),
                poster: image_url(entry.poster.as_deref()),
                rating: entry.row.rating,
                overview: entry.row.overview.clone(),
                items: entry.row.items,
                added_at: entry.row.added_at,
            })
            .collect())
    }

    /// The home screen's one read: a hero, Continue Watching, the library, and the provider rails.
    ///
    /// The provider rails are cache-first; a read with an empty cache starts the same fetch the
    /// desktop's Discover starts and answers `fetching`, so the client draws the local rails and
    /// fills the rest on a later read rather than showing a spinner.
    ///
    /// # Errors
    ///
    /// [`WebError::Runtime`] when a local read fails. A provider failure is an empty rail, not a
    /// failed page: the library is still worth drawing without the network.
    pub async fn home_page(&self) -> Result<HomeDto, WebError> {
        let mut rails = Vec::new();
        let mut hero: Option<TileDto> = None;

        let continued = self.runtime.continue_watching().await?;
        let mut continue_items = Vec::new();
        for row in continued {
            let id = row.progress.title_id();
            let Some(detail) = self.runtime.library_title(id).await? else {
                continue;
            };
            let tile = tile_from_detail(
                detail,
                Some(row.progress.percent()),
                row.progress.position_ms(),
            );
            if hero.is_none() && tile.backdrop.is_some() {
                hero = Some(tile.clone());
            }
            continue_items.push(tile);
        }
        if !continue_items.is_empty() {
            rails.push(RailDto {
                id: "continue".to_owned(),
                title: "Continue watching".to_owned(),
                items: continue_items,
            });
        }

        if let Some(rail) = self.my_list_rail().await? {
            rails.push(rail);
        }

        let library: Vec<TileDto> = self
            .runtime
            .library_titles(None, 40)
            .await?
            .into_iter()
            .map(|entry| tile_from_entry(&entry))
            .collect();
        if !library.is_empty() {
            rails.push(RailDto {
                id: "library".to_owned(),
                title: "My library".to_owned(),
                items: library.clone(),
            });
        }

        let provider_rails = self.runtime.home_rails(None).await.unwrap_or_default();
        let mut state = "ready";
        for rail in provider_rails {
            state = match rail.state {
                crate::RailsState::NoProvider => "no_provider",
                crate::RailsState::Fetching => "fetching",
                crate::RailsState::Ready => "ready",
            };
            let items: Vec<TileDto> = rail.items.into_iter().map(tile_from_rail).collect();
            if items.is_empty() {
                continue;
            }
            if hero.is_none() {
                hero = items.iter().find(|tile| tile.backdrop.is_some()).cloned();
            }
            rails.push(RailDto {
                id: rail.rail.as_str().to_owned(),
                title: rail_title(rail.rail).to_owned(),
                items,
            });
        }

        // The watchlist is one read for the whole page, and every rail and hero candidate takes
        // its mark from it. A tile in Browse and the same title in Continue Watching cannot then
        // disagree about whether it is saved.
        let watchlisted = self.watchlisted_ids().await?;
        for rail in &mut rails {
            mark_watchlisted(&mut rail.items, &watchlisted);
        }

        if hero.is_none()
            && let Some(first) = library.first()
            && let Some(detail) = self.runtime.library_title(first.id).await?
        {
            hero = Some(tile_from_detail(detail, None, 0));
        }

        // The desktop's hero rotates through several titles; collect one candidate per title that
        // has a backdrop, keeping the rails' order so the rotation starts on the same film the
        // rest of the screen leads with.
        let mut heroes = hero_candidates(&rails, hero.as_ref());
        if hero.is_none() {
            hero = heroes.first().cloned();
        }
        if let Some(hero) = &mut hero {
            hero.watchlisted = watchlisted.contains(&hero.id);
        }
        for tile in &mut heroes {
            tile.watchlisted = watchlisted.contains(&tile.id);
        }

        Ok(HomeDto {
            hero,
            rails,
            heroes,
            state: state.to_owned(),
        })
    }

    /// The download queue with the progress a second screen polls.
    ///
    /// # Errors
    ///
    /// [`WebError::Runtime`] when the queue or a title read fails.
    pub async fn downloads_page(&self) -> Result<Vec<DownloadDto>, WebError> {
        let mut downloads = Vec::new();
        for row in self.runtime.download_jobs().await? {
            let (title, kind, poster) = match self.runtime.library_title(row.title_id).await? {
                Some(detail) => {
                    let entry = detail.entry;
                    (
                        Some(entry.row.title.name.clone()),
                        Some(entry.row.title.kind.as_str().to_owned()),
                        image_url(entry.poster.as_deref()),
                    )
                }
                None => match self.runtime.title_identity(row.title_id).await? {
                    Some(identity) => (
                        Some(identity.name),
                        Some(identity.kind.as_str().to_owned()),
                        None,
                    ),
                    None => (None, None, None),
                },
            };
            let episode = match row.episode_id {
                Some(id) => self.runtime.episode_numbers(id).await?,
                None => None,
            };
            downloads.push(DownloadDto {
                id: row.id,
                title_id: row.title_id,
                title,
                kind,
                poster,
                state: row.state.as_str().to_owned(),
                progress_permille: row.progress_permille,
                downloaded_bytes: row.downloaded_bytes,
                total_bytes: row.reported_size_bytes,
                rate_bps: row.download_rate_bps,
                eta_seconds: row.eta_seconds,
                season_number: episode.map(|(season, _)| season),
                episode_number: episode.map(|(_, number)| number),
                error: row.error_message,
            });
        }
        Ok(downloads)
    }
    /// The device behind a request, with the one capability the client gates its buttons on.
    #[must_use]
    pub fn me(&self, device: &PairedDevice) -> MeDto {
        MeDto {
            name: device.name.clone(),
            scopes: device.scopes.clone(),
            can_acquire: scope_allows(&device.scopes, ACQUIRE_SCOPE),
        }
    }

    /// Starts the same automatic acquisition the desktop's Download button starts.
    ///
    /// The request is audited before the queue is touched, so a refusal or a failed submission
    /// still says which device asked for which title.
    ///
    /// # Errors
    ///
    /// [`WebError::NotPermitted`] when the device lacks the scope, [`WebError::InvalidRequest`]
    /// for half an episode or a zero marker, [`WebError::UnknownTitle`] when the id is unknown, or
    /// the runtime's acquisition failures.
    pub async fn acquire_title(
        &self,
        device: &PairedDevice,
        title_id: i64,
        season: Option<u32>,
        episode: Option<u32>,
    ) -> Result<AcquireDto, WebError> {
        if !scope_allows(&device.scopes, ACQUIRE_SCOPE) {
            return Err(WebError::NotPermitted);
        }
        let pick = match (season, episode) {
            (Some(season), Some(episode)) if season > 0 && episode > 0 => {
                Some(crate::jobs::acquire::EpisodePick { season, episode })
            }
            (None, None) => None,
            _ => return Err(WebError::InvalidRequest),
        };
        if self.runtime.title_identity(title_id).await?.is_none() {
            return Err(WebError::UnknownTitle);
        }
        let episode_id = match pick {
            Some(pick) => {
                self.runtime
                    .episode_id(title_id, pick.season, pick.episode)
                    .await?
            }
            None => None,
        };
        let audit_id = self
            .database
            .record_web_audit(peregrine_storage::NewWebAudit {
                device_id: device.id,
                action: peregrine_storage::WebAuditAction::Acquire,
                title_id,
                episode_id,
                job_id: None,
                at: self.now(),
            })
            .await?;
        let started = self
            .runtime
            .start_acquire(crate::runtime::installation_scope(), title_id, pick, 1)
            .await?;
        if let Err(error) = self
            .database
            .set_web_audit_job(audit_id, started.journal_id)
            .await
        {
            warn!(%error, "the acquisition audit could not name its job");
        }
        info!(
            device = device.id,
            title = title_id,
            "a paired device started an acquisition"
        );
        Ok(AcquireDto {
            job_id: started.job.to_string(),
            journal_id: started.journal_id,
            coalesced: started.coalesced,
        })
    }

    /// The account's playback language preference.
    ///
    /// # Errors
    ///
    /// [`WebError::Runtime`] when the row cannot be read.
    pub async fn preferences(&self) -> Result<PreferencesDto, WebError> {
        let preference = self.runtime.playback_preference().await?;
        Ok(PreferencesDto {
            audio_language: preference.audio_language().to_owned(),
            subtitle_language: preference.subtitle_language().to_owned(),
            subtitles: preference.subtitles().as_str().to_owned(),
        })
    }

    /// Adds or removes one title on the account's watchlist.
    ///
    /// # Errors
    ///
    /// [`WebError::UnknownTitle`] when the title is not visible here, or [`WebError::Runtime`]
    /// when the write fails.
    pub async fn set_watchlist(
        &self,
        device: &PairedDevice,
        title_id: i64,
        saved: bool,
    ) -> Result<(), WebError> {
        if self.runtime.title_identity(title_id).await?.is_none() {
            return Err(WebError::UnknownTitle);
        }
        if saved {
            self.runtime.add_to_watchlist(title_id).await?;
        } else {
            self.runtime.remove_from_watchlist(title_id).await?;
        }
        info!(
            device = device.id,
            title = title_id,
            saved,
            "a paired device changed the watchlist"
        );
        Ok(())
    }

    /// Marks one title, episode, or season watched or unwatched.
    ///
    /// The scope is an exact word rather than a prefix, and an episode needs both of its numbers
    /// because a season alone is a different write. A title or episode the caller named that the
    /// library does not hold is a 404 rather than a storage error surfacing as a 500.
    ///
    /// # Errors
    ///
    /// [`WebError::InvalidRequest`] for a missing or unknown scope or number,
    /// [`WebError::UnknownTitle`] or [`WebError::UnknownEpisode`] when the row is absent, and
    /// [`WebError::Runtime`] when the write fails.
    pub async fn set_watched(
        &self,
        device: &PairedDevice,
        title_id: i64,
        scope: &str,
        season: Option<u32>,
        episode: Option<u32>,
        watched: bool,
    ) -> Result<(), WebError> {
        if self.runtime.title_identity(title_id).await?.is_none() {
            return Err(WebError::UnknownTitle);
        }
        match scope {
            "title" => self.runtime.set_title_watched(title_id, watched).await?,
            "episode" => {
                let (Some(season), Some(episode)) = (season, episode) else {
                    return Err(WebError::InvalidRequest);
                };
                if season == 0 || episode == 0 {
                    return Err(WebError::InvalidRequest);
                }
                if self
                    .runtime
                    .episode_id(title_id, season, episode)
                    .await?
                    .is_none()
                {
                    return Err(WebError::UnknownEpisode);
                }
                match self
                    .runtime
                    .set_episode_watched(title_id, season, episode, watched)
                    .await
                {
                    Ok(()) => {}
                    // The episode was resolved a statement ago; a row that vanished since is
                    // still the caller's 404, not an internal failure.
                    Err(crate::RuntimeError::InvalidRequest { .. }) => {
                        return Err(WebError::UnknownEpisode);
                    }
                    Err(error) => return Err(WebError::Runtime(error)),
                }
            }
            "season" => {
                let Some(season) = season.filter(|season| *season > 0) else {
                    return Err(WebError::InvalidRequest);
                };
                self.runtime
                    .set_season_watched(title_id, season, watched)
                    .await?;
            }
            _ => return Err(WebError::InvalidRequest),
        }
        info!(
            device = device.id,
            title = title_id,
            scope,
            watched,
            "a paired device changed a watched mark"
        );
        Ok(())
    }

    /// Queues every episode of one season that has no file on disk.
    ///
    /// One episode's failure is counted rather than returned: a season is fifteen downloads, and
    /// a release the indexer cannot find for episode four must not cost the other fourteen. The
    /// audit row is written before the queue is touched, so a failed submission still names the
    /// device and the episode; when a row cannot be written the episode is not queued at all,
    /// because an unattributed acquisition is the thing ADR 0029 forbids.
    ///
    /// # Errors
    ///
    /// [`WebError::NotPermitted`] when the device lacks the acquire scope,
    /// [`WebError::InvalidRequest`] for a zero season, [`WebError::UnknownTitle`] for an id no
    /// catalogue row holds, and [`WebError::Runtime`] when the season read fails.
    #[allow(clippy::too_many_lines)] // One loop over the season; splitting it hides the audit order.
    pub async fn acquire_season(
        &self,
        device: &PairedDevice,
        title_id: i64,
        season: u32,
    ) -> Result<SeasonAcquireDto, WebError> {
        if !scope_allows(&device.scopes, ACQUIRE_SCOPE) {
            return Err(WebError::NotPermitted);
        }
        if season == 0 {
            return Err(WebError::InvalidRequest);
        }
        if self.runtime.title_identity(title_id).await?.is_none() {
            return Err(WebError::UnknownTitle);
        }
        let episodes = self
            .runtime
            .title_episodes(title_id, Some(season), 500)
            .await?;
        let mut result = SeasonAcquireDto {
            queued: 0,
            already_playable: 0,
            failed: 0,
        };
        for episode in episodes {
            if episode.row.files > 0 {
                result.already_playable += 1;
                continue;
            }
            let episode_number = episode.row.episode_number.get();
            let episode_id = self
                .runtime
                .episode_id(title_id, episode.row.season_number.get(), episode_number)
                .await?;
            let audit_id = match self
                .database
                .record_web_audit(peregrine_storage::NewWebAudit {
                    device_id: device.id,
                    action: peregrine_storage::WebAuditAction::Acquire,
                    title_id,
                    episode_id,
                    job_id: None,
                    at: self.now(),
                })
                .await
            {
                Ok(id) => id,
                Err(error) => {
                    warn!(
                        %error,
                        title = title_id,
                        season,
                        episode = episode_number,
                        "a season acquisition could not be audited"
                    );
                    result.failed += 1;
                    continue;
                }
            };
            let pick = crate::jobs::acquire::EpisodePick {
                season: episode.row.season_number.get(),
                episode: episode_number,
            };
            match self
                .runtime
                .start_acquire(
                    crate::runtime::installation_scope(),
                    title_id,
                    Some(pick),
                    1,
                )
                .await
            {
                Ok(started) => {
                    result.queued += 1;
                    if let Err(error) = self
                        .database
                        .set_web_audit_job(audit_id, started.journal_id)
                        .await
                    {
                        warn!(%error, "an acquisition audit could not name its job");
                    }
                }
                Err(error) => {
                    result.failed += 1;
                    warn!(
                        %error,
                        title = title_id,
                        season,
                        episode = episode_number,
                        "a season acquisition could not be queued"
                    );
                }
            }
        }
        info!(
            device = device.id,
            title = title_id,
            season,
            queued = result.queued,
            already_playable = result.already_playable,
            failed = result.failed,
            "a paired device queued a season"
        );
        Ok(result)
    }

    /// The title ids the account's watchlist holds, read once for a page's tiles.
    ///
    /// # Errors
    ///
    /// [`WebError::Runtime`] when the watchlist read fails.
    async fn watchlisted_ids(&self) -> Result<HashSet<i64>, WebError> {
        Ok(self
            .runtime
            .watchlist()
            .await?
            .into_iter()
            .map(|row| row.entry.title_id)
            .collect())
    }

    /// The account's My List as tiles, newest first.
    ///
    /// # Errors
    ///
    /// [`WebError::Runtime`] when the watchlist or a title read fails.
    pub async fn watchlist_page(&self) -> Result<Vec<TileDto>, WebError> {
        let mut items = Vec::new();
        for row in self.runtime.watchlist().await? {
            let Some(detail) = self.runtime.library_title(row.entry.title_id).await? else {
                continue;
            };
            items.push(tile_from_detail(detail, None, 0));
        }
        for item in &mut items {
            item.watchlisted = true;
        }
        Ok(items)
    }

    /// A search over the provider and the local index.
    ///
    /// # Errors
    ///
    /// [`WebError::Runtime`] when both the provider and the local index fail.
    pub async fn search_page(
        &self,
        query: &str,
        kind: Option<TitleKind>,
    ) -> Result<Vec<TileDto>, WebError> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let query: String = query.chars().take(120).collect();
        let watchlisted = self.watchlisted_ids().await?;
        let mut items: Vec<TileDto> = self
            .runtime
            .search_tiles(&query, 60, kind)
            .await?
            .into_iter()
            .map(tile_from_rail)
            .collect();
        mark_watchlisted(&mut items, &watchlisted);
        Ok(items)
    }

    /// One page of the provider's browse list, or the library when the provider is absent.
    ///
    /// # Errors
    ///
    /// [`WebError::Runtime`] when a local read fails while assembling the fallback.
    pub async fn browse_page(
        &self,
        kind: TitleKind,
        page: u32,
        sort: BrowseSort,
        genre: Option<u32>,
    ) -> Result<BrowseDto, WebError> {
        let request = BrowseRequest::new(
            kind,
            NonZeroU32::new(page).unwrap_or(NonZeroU32::MIN),
            sort,
            genre,
            None,
        );
        let watchlisted = self.watchlisted_ids().await?;
        if let Ok(answer) = self.runtime.browse(&request).await {
            let mut items: Vec<TileDto> = answer.items.into_iter().map(tile_from_rail).collect();
            mark_watchlisted(&mut items, &watchlisted);
            return Ok(BrowseDto {
                items,
                has_more: answer.has_more,
                source: "provider".to_owned(),
            });
        }
        // No provider, or a provider having a bad minute: the library is still a browse screen.
        let mut items: Vec<TileDto> = self
            .runtime
            .library_titles(None, 100)
            .await?
            .into_iter()
            .filter(|entry| entry.row.title.kind == kind)
            .map(|entry| tile_from_entry(&entry))
            .collect();
        mark_watchlisted(&mut items, &watchlisted);
        Ok(BrowseDto {
            items,
            has_more: false,
            source: "library".to_owned(),
        })
    }

    /// The provider's genres for one kind; empty when no provider is configured.
    ///
    /// # Errors
    ///
    /// [`WebError::Runtime`] is not reported: a filter control without genres is still a filter.
    pub async fn genres_page(&self, kind: TitleKind) -> Vec<GenreDto> {
        self.runtime
            .genres(kind)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|genre| GenreDto {
                id: genre.id(),
                name: genre.name().as_str().to_owned(),
            })
            .collect()
    }

    /// My List as a rail, when the watchlist has a title the library still holds.
    ///
    /// # Errors
    ///
    /// [`WebError::Runtime`] when the watchlist or a title read fails.
    async fn my_list_rail(&self) -> Result<Option<RailDto>, WebError> {
        let mut items = Vec::new();
        for row in self.runtime.watchlist().await? {
            let Some(detail) = self.runtime.library_title(row.entry.title_id).await? else {
                continue;
            };
            items.push(tile_from_detail(detail, None, 0));
        }
        Ok((!items.is_empty()).then(|| RailDto {
            id: "watchlist".to_owned(),
            title: "My list".to_owned(),
            items,
        }))
    }
    /// One title as the client's page draws it.
    ///
    /// # Errors
    ///
    /// [`WebError::Runtime`] when the read fails.
    pub async fn title_page(
        &self,
        id: i64,
        season: Option<u32>,
    ) -> Result<Option<TitleDto>, WebError> {
        let detail = self.runtime.library_title(id).await?;
        // A rail can name a catalogue title the library does not hold. It still has a page: the
        // identity supplies the name and kind, and a provider refresh fills in details and
        // episodes for a series.
        let Some(identity) = self.runtime.title_identity(id).await? else {
            return Ok(None);
        };
        let is_show = identity.kind == TitleKind::Show;
        let downloads = self.title_downloads(id).await?;
        let (seasons, episodes) = self
            .title_seasons_page(id, is_show, season, &downloads)
            .await?;
        // A provider fill that never cached its poster (or a series the catalogue only listed)
        // is a page with no main image. The same refresh fills both, and the client already polls
        // while `refreshing` is true.
        let artwork_due = detail.as_ref().is_some_and(|detail| detail.refresh_due);
        let refreshing = artwork_due
            || (is_show && seasons.is_empty() && !identity.metadata_state.has_details());
        if refreshing {
            let start_now = self
                .refresh_started
                .lock()
                .expect("the refresh lock is not poisoned")
                .insert(id);
            if start_now
                && let Err(error) = self.runtime.start_provider_refresh(id, identity.kind).await
            {
                warn!(%error, "a title refresh could not be started");
                self.refresh_started
                    .lock()
                    .expect("the refresh lock is not poisoned")
                    .remove(&id);
            }
        }
        let (similar, similar_genres) = self.title_similar_page(id, detail.is_some()).await?;
        let (poster, backdrop, overview, rating, released, items, files, playable, progress) =
            match &detail {
                Some(detail) => (
                    image_url(detail.entry.poster.as_deref()),
                    image_url(detail.backdrop.as_deref()),
                    detail.overview.clone(),
                    detail.rating,
                    detail.released.map(|date| date.to_string()),
                    detail.entry.row.items,
                    detail.entry.row.files,
                    detail.playback.support.is_some(),
                    self.runtime.web_progress(id).await?,
                ),
                None => (None, None, None, None, None, 0, 0, false, None),
            };
        let (download_state, download_percent) = downloads.summary(is_show, &episodes);
        Ok(Some(TitleDto {
            id,
            name: identity.name,
            kind: identity.kind.as_str().to_owned(),
            poster,
            backdrop,
            overview,
            rating,
            released,
            items,
            files,
            playable,
            progress,
            in_library: detail.is_some(),
            watchlisted: self.runtime.is_watchlisted(id).await?,
            watched: self.runtime.title_watched(id).await?,
            download_state,
            download_percent,
            seasons,
            episodes,
            similar,
            similar_genres,
            refreshing,
        }))
    }

    async fn title_seasons_page(
        &self,
        id: i64,
        is_show: bool,
        season: Option<u32>,
        downloads: &TitleDownloads,
    ) -> Result<(Vec<SeasonDto>, Vec<EpisodeDto>), WebError> {
        if !is_show {
            return Ok((Vec::new(), Vec::new()));
        }
        let seasons: Vec<SeasonDto> = self
            .runtime
            .title_seasons(id, 50)
            .await?
            .into_iter()
            .map(|season| SeasonDto {
                number: season.row.season_number.get(),
                name: season.row.name,
                episode_count: season.row.episode_count,
                poster: image_url(season.poster.as_deref()),
            })
            .collect();
        let chosen = season.or_else(|| {
            seasons
                .iter()
                .find(|held| held.episode_count.unwrap_or(0) > 0 || held.number > 0)
                .map(|held| held.number)
        });
        let progress = self.runtime.title_watch_progress(id).await?;
        let episodes = episode_dtos(
            self.runtime.title_episodes(id, chosen, 500).await?,
            &progress,
            downloads,
        );
        Ok((seasons, episodes))
    }

    /// One read of the title's download jobs, keyed by the numbers a page draws.
    ///
    /// The queue is read once rather than per episode: the title-level job is the newest that
    /// names no episode, and an episode's job is the newest that names it. `episode_numbers`
    /// turns the job's episode id into the season and number the page already has.
    async fn title_downloads(&self, title_id: i64) -> Result<TitleDownloads, WebError> {
        let jobs = self.runtime.download_jobs().await?;
        let mut downloads = TitleDownloads::default();
        for job in jobs.iter().filter(|job| job.title_id == title_id) {
            let download = JobDownload {
                state: job.state.as_str().to_owned(),
                percent: job.progress_permille as f64 / 10.0,
            };
            match job.episode_id {
                None => {
                    if downloads.title.is_none() {
                        downloads.title = Some(download);
                    }
                }
                Some(episode_id) => {
                    if let Some(numbers) = self.runtime.episode_numbers(episode_id).await? {
                        downloads.episodes.entry(numbers).or_insert(download);
                    }
                }
            }
        }
        Ok(downloads)
    }

    async fn title_similar_page(
        &self,
        id: i64,
        in_library: bool,
    ) -> Result<(Vec<TileDto>, Vec<GenreDto>), WebError> {
        if !in_library {
            return Ok((Vec::new(), Vec::new()));
        }
        let watchlisted = self.watchlisted_ids().await?;
        // A title page is useful without the network: the provider's similar list is a bonus, so
        // a slow or unreachable provider gets three seconds and then an empty rail, not a spinner.
        let (mut similar, genres): (Vec<TileDto>, Vec<GenreDto>) =
            match tokio::time::timeout(std::time::Duration::from_secs(3), self.runtime.similar(id))
                .await
            {
                Ok(Ok(found)) => (
                    found.items.into_iter().map(tile_from_rail).collect(),
                    found
                        .genres
                        .into_iter()
                        .map(|genre| GenreDto {
                            id: genre.id(),
                            name: genre.name().as_str().to_owned(),
                        })
                        .collect(),
                ),
                _ => (Vec::new(), Vec::new()),
            };
        mark_watchlisted(&mut similar, &watchlisted);
        if !similar.is_empty() {
            return Ok((similar, genres));
        }
        // A provider with nothing to say should not leave the page without a rail. The library's
        // other titles are a truthful "more like this" and cost one local read.
        let mut similar: Vec<TileDto> = self
            .runtime
            .library_titles(None, 20)
            .await?
            .into_iter()
            .filter(|entry| entry.row.title.id != id)
            .map(|entry| tile_from_entry(&entry))
            .collect();
        mark_watchlisted(&mut similar, &watchlisted);
        Ok((similar, genres))
    }
    /// Mints a playback session for one device and one title.
    ///
    /// # Errors
    ///
    /// [`WebError::NoFile`], [`WebError::TranscodeRequired`], [`WebError::Unsupported`], or a
    /// runtime failure.
    #[allow(clippy::too_many_lines)] // One mint: resolve, choose delivery, then build the ticket.
    pub async fn mint_playback(
        &self,
        device: &PairedDevice,
        title_id: i64,
        season_number: Option<u32>,
        episode_number: Option<u32>,
        episode_id: Option<i64>,
        position_ms: i64,
    ) -> Result<PlaybackTicket, WebError> {
        if !scope_allows(&device.scopes, "stream") {
            return Err(WebError::Unauthorized);
        }
        // A seek restarts from the ticket's episode id; a fresh play names the season and number.
        let episode_id = match episode_id {
            Some(id) => Some(id),
            None => match (season_number, episode_number) {
                (Some(season), Some(episode)) => Some(
                    self.runtime
                        .episode_id(title_id, season, episode)
                        .await?
                        .ok_or(WebError::NoFile)?,
                ),
                _ => None,
            },
        };
        let target = self
            .runtime
            .web_playback(title_id, episode_id)
            .await
            .map_err(|error| match error {
                crate::RuntimeError::UnknownTitle => WebError::UnknownTitle,
                other => WebError::Runtime(other),
            })?;
        let Some(mut target) = target else {
            return Err(WebError::NoFile);
        };
        // A library row can predate the probe. Without a duration the client would treat a growing
        // HLS playlist's current length as the whole film, so the mint asks ffprobe once and the
        // ticket carries the real length from then on.
        if target.duration_ms.is_none() {
            target.duration_ms = transcode::duration_ms(&target.path).await;
        }
        let start = target.duration_ms.map_or(position_ms.max(0), |duration| {
            position_ms.clamp(0, duration)
        });
        let identity = self
            .runtime
            .title_identity(title_id)
            .await?
            .ok_or(WebError::UnknownTitle)?;
        let preference = self.runtime.playback_preference().await?;
        let next_episode = self
            .runtime
            .web_next_episode(title_id, target.episode_id)
            .await?;
        // A container the webview demuxes is served as bytes; everything else is transcoded. The
        // choice is the domain's `LocalPlayback` rule, not this module's codec table.
        // The container is the first question; the streams inside it are the second. An MP4
        // carrying HEVC or AC3 demuxes and still fails, so a probe that names another codec goes
        // to the transcoder just as an MKV does.
        let (kind, url, mime, audio_language, subtitle_offset) = if target.playback
            != LocalPlayback::Webview
            || !transcode::browser_safe(&target.path).await
        {
            let started = self
                .start_hls(device, &target, start, preference.audio_language())
                .await?;
            (
                "hls",
                started.url,
                "application/vnd.apple.mpegurl".to_owned(),
                started.audio_language,
                start,
            )
        } else {
            let mime = mime_for(&target.path);
            let token = self.tokens.next_token();
            let now_ms = self.now();
            self.sessions
                .lock()
                .expect("the session lock is not poisoned")
                .insert(
                    token.as_str().to_owned(),
                    Grant {
                        device_id: device.id,
                        path: target.path,
                        size_bytes: target.size_bytes,
                        mime,
                        expires_at_ms: now_ms + PLAYBACK_TTL_MS,
                    },
                );
            (
                "direct",
                format!("/media/{}", token.as_str()),
                mime.to_owned(),
                None,
                0,
            )
        };
        // The sidecar timeline is the film's own. A transcode's playlist starts at zero, so the
        // URL carries the start offset and the route retimes the cues onto that timeline.
        let subtitles = subtitle_dtos(
            self.runtime
                .web_subtitles(title_id, target.episode_id)
                .await,
            &preference,
            title_id,
            target.episode_id,
            subtitle_offset,
        );
        Ok(PlaybackTicket {
            kind,
            url,
            mime,
            start_position_ms: start,
            duration_ms: target.duration_ms,
            episode_id: target.episode_id,
            title_kind: identity.kind.as_str(),
            title_name: identity.name,
            subtitles,
            next_episode,
            audio_language,
        })
    }

    /// Finds and caches the one ffmpeg this process will use.
    async fn transcoder(&self) -> Result<Arc<transcode::Transcoder>, WebError> {
        let mut slot = self.transcoder.lock().await;
        if let Some(found) = slot.as_ref() {
            return Ok(Arc::clone(found));
        }
        match transcode::Transcoder::detect().await {
            Ok(found) => {
                let found = Arc::new(found);
                info!(
                    encoder = found.encoder(),
                    "the LAN surface found a transcoder"
                );
                *slot = Some(Arc::clone(&found));
                Ok(found)
            }
            Err(error) => {
                warn!(%error, "no permitted H.264 encoder is available for the LAN surface");
                Err(WebError::TranscodeUnavailable)
            }
        }
    }

    /// Kills and removes every transcode this device started.
    async fn evict_hls_device(&self, device_id: i64) {
        let removed = {
            let mut sessions = self.hls.lock().await;
            let keys: Vec<String> = sessions
                .iter()
                .filter(|(_, session)| session.device_id == device_id)
                .map(|(key, _)| key.clone())
                .collect();
            keys.into_iter()
                .filter_map(|key| sessions.remove(&key))
                .collect::<Vec<_>>()
        };
        for mut session in removed {
            let _ = session.child.kill().await;
            let _ = tokio::fs::remove_dir_all(&session.dir).await;
        }
    }

    /// Kills and removes every transcode, for a stop or a shutdown.
    async fn evict_all_hls(&self) {
        let removed: Vec<HlsSession> = {
            let mut sessions = self.hls.lock().await;
            sessions.drain().map(|(_, session)| session).collect()
        };
        for mut session in removed {
            let _ = session.child.kill().await;
            let _ = tokio::fs::remove_dir_all(&session.dir).await;
        }
    }

    /// Stops a playback session the device owns, whether it was direct bytes or a transcode.
    ///
    /// # Errors
    ///
    /// [`WebError::Unauthorized`] when the session belongs to another device.
    pub async fn stop_playback(&self, device: &PairedDevice, url: &str) -> Result<(), WebError> {
        if !scope_allows(&device.scopes, "stream") {
            return Err(WebError::Unauthorized);
        }
        let token = url
            .trim_start_matches('/')
            .split('/')
            .nth(1)
            .unwrap_or_default();
        if token.is_empty() {
            return Ok(());
        }
        let direct = {
            let mut grants = self
                .sessions
                .lock()
                .expect("the session lock is not poisoned");
            match grants.get(token) {
                Some(grant) if grant.device_id == device.id => grants.remove(token),
                _ => None,
            }
        };
        drop(direct);
        let session = {
            let mut sessions = self.hls.lock().await;
            match sessions.get(token) {
                Some(held) if held.device_id == device.id => sessions.remove(token),
                _ => None,
            }
        };
        if let Some(mut session) = session {
            let _ = session.child.kill().await;
            let _ = tokio::fs::remove_dir_all(&session.dir).await;
        }
        Ok(())
    }

    /// Starts a transcode at `start_ms` and answers the HLS playlist URL.
    ///
    /// The preferred audio language is the host's one selection: the transcoder maps the first
    /// stream whose tag matches it and reports the tag back so the ticket can say which language
    /// this session carries. A host that finds no match maps the first stream and reports none.
    async fn start_hls(
        &self,
        device: &PairedDevice,
        target: &WebPlaybackTarget,
        start_ms: i64,
        audio_language: &str,
    ) -> Result<StartedHls, WebError> {
        let transcoder = self.transcoder().await?;
        // One session per device: a resume or a seek replaces the previous transcode rather than
        // leaving two encoders fighting for the same GPU.
        self.evict_hls_device(device.id).await;
        let token = self.tokens.next_token();
        let directory = self.transcode_root.join(token.as_str());
        let mut started = transcoder
            .start(&directory, &target.path, start_ms, Some(audio_language))
            .await
            .map_err(|error| {
                warn!(%error, "the transcode could not be started");
                WebError::TranscodeUnavailable
            })?;
        {
            let mut sessions = self.hls.lock().await;
            if sessions.len() >= HLS_SESSION_LIMIT {
                drop(sessions);
                let _ = started.child.kill().await;
                let _ = tokio::fs::remove_dir_all(&directory).await;
                return Err(WebError::TranscodeBusy);
            }
            sessions.insert(
                token.as_str().to_owned(),
                HlsSession {
                    dir: directory,
                    child: started.child,
                    device_id: device.id,
                    expires_at_ms: self.now() + PLAYBACK_TTL_MS,
                },
            );
        }
        Ok(StartedHls {
            url: format!("/hls/{}/index.m3u8", token.as_str()),
            audio_language: started.audio_language,
        })
    }

    /// The transcode a playlist URL names, when it is live.
    pub async fn hls_grant(&self, token: &str) -> Option<HlsGrant> {
        let now_ms = self.now();
        let (grant, expired) = {
            let mut sessions = self.hls.lock().await;
            let keys: Vec<String> = sessions
                .iter()
                .filter(|(_, session)| session.expires_at_ms <= now_ms)
                .map(|(key, _)| key.clone())
                .collect();
            let expired: Vec<HlsSession> = keys
                .into_iter()
                .filter_map(|key| sessions.remove(&key))
                .collect();
            let grant = sessions.get(token).map(|session| HlsGrant {
                device_id: session.device_id,
                dir: session.dir.clone(),
            });
            (grant, expired)
        };
        for mut session in expired {
            let _ = session.child.kill().await;
            let _ = tokio::fs::remove_dir_all(&session.dir).await;
        }
        if grant.is_some() {
            self.touch_stream();
        }
        grant
    }

    /// Writes where the client is, through the same store and the same policy as the desktop.
    ///
    /// The episode must be the one the playable file belongs to: a client cannot invent a row for
    /// another episode of the title, and a film's report carries no episode at all.
    ///
    /// # Errors
    ///
    /// [`WebError::Unauthorized`], [`WebError::NoFile`], [`WebError::InvalidProgress`], or a
    /// storage failure.
    pub async fn report_progress(
        &self,
        device: &PairedDevice,
        title_id: i64,
        episode_id: Option<i64>,
        position_ms: i64,
        duration_ms: Option<i64>,
    ) -> Result<(), WebError> {
        if !scope_allows(&device.scopes, "stream") {
            return Err(WebError::Unauthorized);
        }
        let Some(target) = self.runtime.web_playback(title_id, episode_id).await? else {
            return Err(WebError::NoFile);
        };
        if target.episode_id != episode_id {
            return Err(WebError::InvalidProgress);
        }
        self.runtime
            .record_progress(title_id, episode_id, position_ms.max(0), duration_ms)
            .await
            .map_err(WebError::Runtime)
    }

    /// The grant a media URL names, when it is live.
    #[must_use]
    pub fn media_grant(&self, token: &str) -> Option<MediaGrant> {
        let now_ms = self.now();
        let mut sessions = self
            .sessions
            .lock()
            .expect("the session lock is not poisoned");
        sessions.retain(|_, grant| grant.expires_at_ms > now_ms);
        let grant = sessions.get(token).map(|grant| MediaGrant {
            device_id: grant.device_id,
            path: grant.path.clone(),
            size_bytes: grant.size_bytes,
            mime: grant.mime,
        });
        drop(sessions);
        if grant.is_some() {
            self.touch_stream();
        }
        grant
    }

    /// Whether a grant's device is still paired and not revoked.
    ///
    /// # Errors
    ///
    /// [`WebError::Storage`] when the read fails.
    pub async fn device_active(&self, id: i64) -> Result<bool, WebError> {
        Ok(self
            .database
            .paired_device(id)
            .await?
            .is_some_and(|device| device.revoked_at.is_none()))
    }

    /// The sidecar a subtitle URL names, resolved through the title's own playable file.
    ///
    /// The client names a title, an episode, and a track id; the path comes from the core's own
    /// guard. `None` covers every refusal — an unknown title, no playable file, a track that
    /// belongs to another file, a sidecar outside every root — because the route's answer to all
    /// of them is one 404.
    pub async fn subtitle_track(
        &self,
        title_id: i64,
        episode_id: Option<i64>,
        track_id: i64,
    ) -> Option<SubtitleSource> {
        self.runtime
            .web_subtitle_path(title_id, episode_id, track_id)
            .await
    }

    /// The cache root, which is the only directory image requests may read.
    #[must_use]
    pub fn artwork_root(&self) -> &Path {
        self.artwork.root()
    }
}

/// A library row as the client draws it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryRowDto {
    /// The title id the client navigates with.
    pub id: i64,
    /// The display name.
    pub name: String,
    /// movie or show.
    pub kind: String,
    /// The image URL, or null when the cache holds none.
    pub poster: Option<String>,
    /// The provider score out of ten.
    pub rating: Option<f64>,
    /// The provider synopsis.
    pub overview: Option<String>,
    /// How many items the library holds.
    pub items: i64,
    /// Unix milliseconds.
    pub added_at: i64,
}

/// A title as its page draws it.
// Five independent flags, each a column or a derived answer a screen draws: playable, in the
// library, on the watchlist, watched, and refreshing. Collapsing them into one state would
// misrepresent a title that is several at once.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleDto {
    /// The title id.
    pub id: i64,
    /// The display name.
    pub name: String,
    /// movie or show.
    pub kind: String,
    /// The poster URL, or null.
    pub poster: Option<String>,
    /// The backdrop URL, or null.
    pub backdrop: Option<String>,
    /// The provider synopsis.
    pub overview: Option<String>,
    /// The provider score.
    pub rating: Option<f64>,
    /// The release date, as YYYY-MM-DD.
    pub released: Option<String>,
    /// How many items the library holds.
    pub items: i64,
    /// How many files those items have.
    pub files: i64,
    /// Whether this build can hand the browser the file directly.
    pub playable: bool,
    /// Where the desktop or this client last left the title, when anybody has played it.
    pub progress: Option<WebProgress>,
    /// Whether the library holds a copy; false for a title opened from a discovery rail.
    pub in_library: bool,
    /// Whether the account's watchlist holds this title.
    pub watchlisted: bool,
    /// Whether a whole-title progress row is marked watched.
    pub watched: bool,
    /// The title-level download job's lifecycle state, or null when there is none.
    pub download_state: Option<String>,
    /// The title-level job's completion, 0 to 100.
    pub download_percent: Option<f64>,
    /// The seasons a series offers, empty for a film.
    pub seasons: Vec<SeasonDto>,
    /// The episodes of the requested season, empty for a film.
    pub episodes: Vec<EpisodeDto>,
    /// The titles a provider considers like this one, for the More Like This feed.
    pub similar: Vec<TileDto>,
    /// The genres those recommendations share, which the feed pages through after them.
    pub similar_genres: Vec<GenreDto>,
    /// A provider refresh is filling this title's details and episodes in the background.
    pub refreshing: bool,
}

/// One tile a rail or a grid draws.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TileDto {
    /// The catalogue id the tile opens.
    pub id: i64,
    /// The display name.
    pub name: String,
    /// movie or show.
    pub kind: String,
    /// The release year, when one is known.
    pub year: Option<i32>,
    /// The poster URL, or null.
    pub poster: Option<String>,
    /// The backdrop URL, or null.
    pub backdrop: Option<String>,
    /// The provider score out of ten.
    pub rating: Option<f64>,
    /// The provider synopsis.
    pub overview: Option<String>,
    /// Whether a file is on disk for it.
    pub playable: bool,
    /// How far through it is, 0 to 100, when it is in Continue Watching.
    pub percent: Option<f64>,
    /// Where it was left, in milliseconds, so a hero's Play resumes.
    pub position_ms: i64,
    /// Whether the account's watchlist holds this title, so every tile can carry its mark.
    pub watchlisted: bool,
}

/// One horizontal rail on the home screen.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RailDto {
    /// The stable identifier the client keys its heading on.
    pub id: String,
    /// The heading, as this build spells it.
    pub title: String,
    /// The tiles, in the provider's order.
    pub items: Vec<TileDto>,
}

/// The home screen's one read.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeDto {
    /// The title the hero draws, when there is one.
    pub hero: Option<TileDto>,
    /// The rails below it, in drawing order.
    pub rails: Vec<RailDto>,
    /// The titles the desktop hero rotates through, when there are several.
    pub heroes: Vec<TileDto>,
    /// What the provider rails are doing: `no_provider`, `fetching`, or `ready`.
    pub state: String,
}

/// One page of a browse or search answer.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseDto {
    /// The tiles on this page.
    pub items: Vec<TileDto>,
    /// Whether asking for the next page would find anything.
    pub has_more: bool,
    /// Where the answer came from: `provider` or `library`.
    pub source: String,
}

/// One genre a browse filter offers.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenreDto {
    /// The provider's identifier.
    pub id: u32,
    /// The name the filter draws.
    pub name: String,
}

/// The device behind the current request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeDto {
    /// The name the person approved.
    pub name: String,
    /// The scopes the row holds.
    pub scopes: String,
    /// Whether this device may start a download.
    pub can_acquire: bool,
}

/// One acquisition a device asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquireDto {
    /// The scheduler's id; nil when the request joined a job already running.
    pub job_id: String,
    /// The journal row, which outlives the scheduler handle.
    pub journal_id: i64,
    /// Whether the request joined a job already running.
    pub coalesced: bool,
}

/// One download job as the progress screen draws it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadDto {
    /// The row id.
    pub id: i64,
    /// The title being downloaded.
    pub title_id: i64,
    /// Its display name, when the catalogue can answer it.
    pub title: Option<String>,
    /// movie or show, when known.
    pub kind: Option<String>,
    /// The poster URL, or null.
    pub poster: Option<String>,
    /// The lifecycle state's stored spelling.
    pub state: String,
    /// Completion in permille, 1000 being the whole payload.
    pub progress_permille: i64,
    /// Payload bytes received so far.
    pub downloaded_bytes: u64,
    /// The size the indexer claimed, when it claimed one.
    pub total_bytes: Option<u64>,
    /// Last reported download rate, in bytes per second.
    pub rate_bps: u64,
    /// Seconds the transfer is expected to take, when there is an estimate.
    pub eta_seconds: Option<u64>,
    /// The episode's season, for an episode job.
    pub season_number: Option<u32>,
    /// The episode's number within the season.
    pub episode_number: Option<u32>,
    /// The engine's redacted failure message, when it failed.
    pub error: Option<String>,
}

/// One season a switcher offers.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeasonDto {
    /// The number, zero for specials.
    pub number: u32,
    /// The name, when the provider gave one.
    pub name: Option<String>,
    /// How many episodes the provider says it holds.
    pub episode_count: Option<u32>,
    /// The poster URL, or null.
    pub poster: Option<String>,
}

/// One episode row a series page draws.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeDto {
    /// The season number.
    pub season_number: u32,
    /// The number within the season.
    pub episode_number: u32,
    /// The name, when one is known.
    pub name: Option<String>,
    /// The synopsis.
    pub overview: Option<String>,
    /// The still URL, or null.
    pub still: Option<String>,
    /// The runtime in minutes.
    pub runtime_minutes: Option<u32>,
    /// The provider's score for this episode.
    pub rating: Option<f64>,
    /// The air date, as YYYY-MM-DD.
    pub air_date: Option<String>,
    /// How many files the library holds for it.
    pub files: i64,
    /// How far through it is, 0 to 100.
    pub percent: f64,
    /// Where it was left, in milliseconds.
    pub position_ms: i64,
    /// Whether it is marked watched.
    pub watched: bool,
    /// Whether a file is on disk for it.
    pub playable: bool,
    /// The download job's lifecycle state for this episode, or null when nobody asked.
    pub download_state: Option<String>,
    /// The job's completion, 0 to 100, when one is running.
    pub download_percent: Option<f64>,
}

/// Where a title was left, as the client's Resume control reads it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebProgress {
    /// The episode the position belongs to, when the title is a series.
    pub episode_id: Option<i64>,
    /// Milliseconds into the episode or film.
    pub position_ms: i64,
    /// The runtime, when the probe recorded one.
    pub duration_ms: Option<i64>,
    /// The stored percentage.
    pub percent: f64,
    /// Whether the row is marked watched.
    pub watched: bool,
}

/// The token hash the database stores.
#[must_use]
pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

fn code_for(token: &SessionToken) -> String {
    let digest = Sha256::digest(token.as_str().as_bytes());
    let value = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) % 100_000_000;
    format!("{value:08}")
}

fn scope_allows(scopes: &str, wanted: &str) -> bool {
    scopes.split(',').any(|scope| scope.trim() == wanted)
}

fn hero_candidates(rails: &[RailDto], hero: Option<&TileDto>) -> Vec<TileDto> {
    let mut heroes: Vec<TileDto> = Vec::new();
    for rail in rails {
        for tile in &rail.items {
            if tile.backdrop.is_some() && !heroes.iter().any(|held| held.id == tile.id) {
                heroes.push(tile.clone());
            }
        }
    }
    heroes.truncate(6);
    if heroes.is_empty()
        && let Some(first) = hero
    {
        heroes.push(first.clone());
    }
    heroes
}

fn episode_dtos(
    episodes: Vec<crate::TitleEpisodeDetail>,
    progress: &[peregrine_storage::EpisodeProgressRow],
    downloads: &TitleDownloads,
) -> Vec<EpisodeDto> {
    episodes
        .into_iter()
        .map(|episode| {
            let row = episode.row;
            let found = progress.iter().find(|held| {
                held.season_number == row.season_number.get()
                    && held.episode_number == row.episode_number.get()
            });
            let download = downloads
                .episodes
                .get(&(row.season_number.get(), row.episode_number.get()));
            EpisodeDto {
                season_number: row.season_number.get(),
                episode_number: row.episode_number.get(),
                name: row.name,
                overview: row.overview,
                still: image_url(episode.still.as_deref()),
                runtime_minutes: row.runtime_minutes,
                rating: row.rating,
                air_date: row.air_date.map(|date| date.to_string()),
                files: row.files,
                percent: found.map_or(0.0, |held| held.percent * 100.0),
                position_ms: found.map_or(0, |held| held.position_ms),
                watched: found.is_some_and(|held| held.watched),
                playable: row.files > 0,
                download_state: download.map(|job| job.state.clone()),
                download_percent: download.map(|job| job.percent),
            }
        })
        .collect()
}

/// The player's subtitle tracks, with exactly one `default` chosen by the account's preference.
fn subtitle_dtos(
    tracks: Vec<WebSubtitleTrack>,
    preference: &PlaybackPreference,
    title_id: i64,
    episode_id: Option<i64>,
    offset_ms: i64,
) -> Vec<SubtitleTrackDto> {
    let selected = chosen_subtitle(&tracks, preference);
    tracks
        .into_iter()
        .map(|track| SubtitleTrackDto {
            id: track.id,
            language: track.language,
            label: track.label,
            format: track.format,
            forced: track.forced,
            sdh: track.sdh,
            r#default: selected == Some(track.id),
            url: subtitle_url(track.id, title_id, episode_id, offset_ms),
        })
        .collect()
}

/// The row id the preference chooses, or `None` when no track should start on.
fn chosen_subtitle(tracks: &[WebSubtitleTrack], preference: &PlaybackPreference) -> Option<i64> {
    match preference.subtitles() {
        SubtitleMode::Off => None,
        SubtitleMode::Forced => {
            let forced: Vec<&WebSubtitleTrack> =
                tracks.iter().filter(|track| track.forced).collect();
            forced
                .iter()
                .copied()
                .find(|track| tag_matches(&track.language, preference.subtitle_language()))
                .or_else(|| forced.first().copied())
                .map(|track| track.id)
        }
        SubtitleMode::Auto => tracks
            .iter()
            .find(|track| tag_matches(&track.language, preference.subtitle_language()))
            .or_else(|| tracks.iter().find(|track| track.is_default))
            // A lone sidecar tagged "und" is the common shape of a downloaded subtitle: the
            // library has no language to match, and starting with it off would hide the only
            // track the file has. More than one track keeps the preference honest.
            .or_else(|| {
                let only = tracks.first()?;
                (tracks.len() == 1 && only.language.eq_ignore_ascii_case("und")).then_some(only)
            })
            .map(|track| track.id),
    }
}

/// Whether a track's tag names the wanted language: a case-insensitive prefix either way.
///
/// The either-way comparison is what makes `en` match a provider's `eng` and a row's `en-US`,
/// without requiring the preference and the row to agree on how much of the tag they wrote.
fn tag_matches(tag: &str, wanted: &str) -> bool {
    let tag = tag.trim().to_ascii_lowercase();
    let wanted = wanted.trim().to_ascii_lowercase();
    !tag.is_empty() && !wanted.is_empty() && (tag.starts_with(&wanted) || wanted.starts_with(&tag))
}

/// The URL a track is fetched from, with the offset that puts its cues on the session's timeline.
fn subtitle_url(track_id: i64, title_id: i64, episode_id: Option<i64>, offset_ms: i64) -> String {
    let mut url = format!("/api/subtitle/{track_id}?titleId={title_id}");
    if let Some(episode_id) = episode_id {
        let _ = write!(url, "&episodeId={episode_id}");
    }
    let _ = write!(url, "&offsetMs={offset_ms}");
    url
}

fn tile_from_entry(entry: &crate::LibraryTitleEntry) -> TileDto {
    TileDto {
        id: entry.row.title.id,
        name: entry.row.title.name.clone(),
        kind: entry.row.title.kind.as_str().to_owned(),
        year: None,
        poster: image_url(entry.poster.as_deref()),
        backdrop: None,
        rating: entry.row.rating,
        overview: entry.row.overview.clone(),
        playable: entry.row.files > 0,
        percent: None,
        position_ms: 0,
        watchlisted: false,
    }
}

fn tile_from_detail(
    detail: crate::LibraryTitleDetail,
    percent: Option<f64>,
    position_ms: i64,
) -> TileDto {
    let entry = detail.entry;
    TileDto {
        id: entry.row.title.id,
        name: entry.row.title.name.clone(),
        kind: entry.row.title.kind.as_str().to_owned(),
        year: None,
        poster: image_url(entry.poster.as_deref()),
        backdrop: image_url(detail.backdrop.as_deref()),
        rating: entry.row.rating,
        overview: entry.row.overview.clone(),
        playable: detail.playback.support.is_some(),
        percent,
        position_ms,
        watchlisted: false,
    }
}
fn tile_from_rail(item: crate::RailItem) -> TileDto {
    TileDto {
        id: item.title_id,
        name: item.name,
        kind: item.kind.as_str().to_owned(),
        year: item.year,
        poster: image_url(item.poster.as_deref()),
        backdrop: image_url(item.backdrop.as_deref()),
        rating: item.rating,
        overview: item.overview,
        playable: item.playable,
        percent: None,
        position_ms: 0,
        watchlisted: false,
    }
}

/// Marks every tile the account's watchlist holds, from one set the page read once.
fn mark_watchlisted(tiles: &mut [TileDto], ids: &HashSet<i64>) {
    for tile in tiles {
        tile.watchlisted = ids.contains(&tile.id);
    }
}

fn rail_title(rail: peregrine_providers::RailKind) -> &'static str {
    match rail {
        peregrine_providers::RailKind::TrendingWeek => "Trending this week",
        peregrine_providers::RailKind::PopularMovies => "Popular films",
        peregrine_providers::RailKind::TopRatedMovies => "Top rated films",
        peregrine_providers::RailKind::UpcomingMovies => "Coming soon",
        peregrine_providers::RailKind::PopularShows => "Popular series",
        peregrine_providers::RailKind::OnTheAirShows => "On the air",
        peregrine_providers::RailKind::TopRatedShows => "Top rated series",
    }
}

fn image_url(path: Option<&Path>) -> Option<String> {
    path.and_then(Path::file_name)
        .and_then(std::ffi::OsStr::to_str)
        .map(|key| format!("/img/{key}"))
}

fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("mp4" | "m4v" | "mov") => "video/mp4",
        Some("webm") => "video/webm",
        _ => "application/octet-stream",
    }
}

/// Whether a hostname could arrive in a Host header: an ASCII DNS name and nothing else.
fn is_hostname(hostname: &str) -> bool {
    !hostname.is_empty()
        && hostname.len() <= 253
        && !hostname.starts_with('.')
        && !hostname.ends_with('.')
        && !hostname.contains("..")
        && hostname
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
}

/// The longest label a person may give a device.
const DEVICE_NAME_MAX: usize = 64;

/// A display label a person typed: trimmed, non-empty, bounded, and free of control characters.
fn device_name(raw: &str) -> Result<String, WebError> {
    let name = raw.trim();
    if name.is_empty()
        || name.chars().count() > DEVICE_NAME_MAX
        || name.chars().any(char::is_control)
    {
        return Err(WebError::InvalidName);
    }
    Ok(name.to_owned())
}

fn allowed_hosts(address: SocketAddr, hostname: Option<&str>) -> Vec<String> {
    let mut hosts = vec![address.to_string()];
    if address.ip().is_loopback() {
        hosts.push(format!("localhost:{}", address.port()));
        hosts.push(format!("127.0.0.1:{}", address.port()));
        hosts.push(format!("[::1]:{}", address.port()));
    }
    if let Some(hostname) = hostname {
        hosts.push(format!("{hostname}:{}", address.port()));
        hosts.push(hostname.to_owned());
    }
    hosts
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use peregrine_domain::SessionToken;
    use peregrine_platform::{Inhibitor, NoPowerSave, PowerSave};
    use peregrine_testkit::MemorySecretStore;
    use tempfile::TempDir;

    use peregrine_domain::{PlaybackPreference, SubtitleMode};

    use super::{
        EpisodeDto, JobDownload, TileDto, TitleDownloads, WebConfig, WebError, WebSubtitleTrack,
        chosen_subtitle, code_for, hash_token, mark_watchlisted, mime_for, scope_allows,
        subtitle_dtos, subtitle_url,
    };
    use crate::events::EventSink;
    use crate::testing::RecordingSink;
    use crate::{Observability, Runtime, RuntimeConfig};

    fn tile(id: i64) -> TileDto {
        TileDto {
            id,
            name: format!("Title {id}"),
            kind: "movie".to_owned(),
            year: None,
            poster: None,
            backdrop: None,
            rating: None,
            overview: None,
            playable: false,
            percent: None,
            position_ms: 0,
            watchlisted: false,
        }
    }

    #[test]
    fn a_tiles_watchlist_mark_comes_from_the_pages_one_set() {
        let ids = HashSet::from([2_i64]);
        let mut tiles = vec![tile(1), tile(2)];
        mark_watchlisted(&mut tiles, &ids);
        assert!(!tiles[0].watchlisted);
        assert!(tiles[1].watchlisted);
    }

    #[test]
    fn a_token_hash_is_sha256_lowercase_hex_and_never_the_token() {
        let hash = hash_token("abc");
        assert_eq!(hash.len(), 64);
        assert!(
            hash.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_ne!(hash, "abc");
    }

    #[test]
    fn a_code_is_eight_digits_and_depends_on_the_token() {
        let token = SessionToken::new("a".repeat(32)).expect("a token");
        let code = code_for(&token);
        assert_eq!(code.len(), 8);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
        let other = SessionToken::new("b".repeat(32)).expect("a token");
        assert_ne!(code, code_for(&other));
    }

    #[test]
    fn a_scope_is_an_exact_word_rather_than_a_prefix() {
        assert!(scope_allows("read,stream", "read"));
        assert!(scope_allows("read, stream", "stream"));
        assert!(!scope_allows("read,stream", "write"));
        assert!(!scope_allows("read,stream", "acquire"));
        assert!(scope_allows("read,stream,acquire", "acquire"));
        assert!(!scope_allows("reader", "read"));
    }

    #[test]
    fn the_content_type_follows_the_container_a_browser_demuxes() {
        assert_eq!(mime_for(std::path::Path::new("a.MP4")), "video/mp4");
        assert_eq!(mime_for(std::path::Path::new("a.webm")), "video/webm");
        assert_eq!(
            mime_for(std::path::Path::new("a.mkv")),
            "application/octet-stream"
        );
    }

    #[test]
    fn a_bind_address_is_the_only_host_accepted_by_default() {
        let hosts = super::allowed_hosts(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)), 8787),
            None,
        );
        assert_eq!(hosts, vec!["192.168.1.10:8787".to_owned()]);
    }

    #[test]
    fn a_configured_hostname_is_accepted_besides_the_bound_address() {
        let hosts = super::allowed_hosts(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)), 8787),
            Some("studio-mac.local"),
        );
        assert!(hosts.contains(&"studio-mac.local:8787".to_owned()));
        assert!(hosts.contains(&"studio-mac.local".to_owned()));
    }

    /// A power policy that counts the assertions it is holding.
    #[derive(Default)]
    struct RecordingPower {
        held: Arc<AtomicUsize>,
    }

    impl RecordingPower {
        fn held(&self) -> usize {
            self.held.load(Ordering::SeqCst)
        }
    }

    impl PowerSave for RecordingPower {
        fn inhibit(&self, _reason: &str) -> Box<dyn Inhibitor> {
            self.held.fetch_add(1, Ordering::SeqCst);
            Box::new(HeldPower(Arc::clone(&self.held)))
        }
    }

    struct HeldPower(Arc<AtomicUsize>);

    impl Inhibitor for HeldPower {}

    impl Drop for HeldPower {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// A listener over a temporary runtime, for the tests that need a real socket.
    async fn listening(directory: &TempDir) -> (Arc<super::WebService>, Arc<Runtime>, u16) {
        listening_with(directory, Arc::new(NoPowerSave)).await
    }

    async fn listening_with(
        directory: &TempDir,
        power: Arc<dyn PowerSave>,
    ) -> (Arc<super::WebService>, Arc<Runtime>, u16) {
        let config = RuntimeConfig::new(
            directory.path(),
            directory.path().join("cache/images"),
            Arc::new(RecordingSink::default()) as Arc<dyn EventSink>,
        )
        .with_secrets(Arc::new(MemorySecretStore::new()))
        .with_power(power);
        let (subscriber, logging) = Observability::build(&config.log_config()).unwrap();
        drop(subscriber);
        let runtime = Arc::new(Runtime::start(config, logging).await.unwrap());
        let web = Arc::new(runtime.web_service());
        let started = web
            .start(WebConfig::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .await
            .unwrap();
        let port = started.address.expect("a bound socket").port();
        (web, runtime, port)
    }

    #[tokio::test]
    async fn a_rebind_that_cannot_bind_leaves_the_listener_where_it_was() {
        let directory = TempDir::new().unwrap();
        let (web, runtime, port) = listening(&directory).await;
        let first = IpAddr::V4(Ipv4Addr::LOCALHOST);

        // TEST-NET-1 is documented and never assigned, so this is the "address the machine does not
        // own" path: the new socket cannot be opened, and the old one must keep serving.
        let failed = web.rebind(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))).await;
        assert!(matches!(failed, Err(WebError::Bind { .. })));
        assert_eq!(
            web.status().await.address,
            Some(SocketAddr::new(first, port)),
            "a failed move is not a stop"
        );
        let still_serving = std::net::TcpStream::connect_timeout(
            &SocketAddr::new(first, port),
            Duration::from_millis(500),
        );
        assert!(still_serving.is_ok(), "the old socket kept serving");

        web.stop().await;
        runtime.shutdown().await;
    }

    // Linux assigns the whole 127/8 block to loopback, so this is the one platform where a second
    // local address exists without root and without touching a real interface.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_listener_moves_to_another_loopback_address_on_the_same_port() {
        let directory = TempDir::new().unwrap();
        let (web, runtime, port) = listening(&directory).await;
        let first = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        let second_ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2));
        let second = SocketAddr::new(second_ip, port);

        let moved = web.rebind(second_ip).await.unwrap();
        assert_eq!(moved.address, Some(second));
        assert!(
            std::net::TcpStream::connect_timeout(&first, Duration::from_millis(500)).is_err(),
            "the address it left no longer answers"
        );
        assert!(
            std::net::TcpStream::connect_timeout(&second, Duration::from_millis(500)).is_ok(),
            "the address it moved to answers"
        );

        web.stop().await;
        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn the_keep_awake_switch_holds_and_releases_the_surface_assertion() {
        let directory = TempDir::new().unwrap();
        let power = Arc::new(RecordingPower::default());
        let (web, runtime, _port) =
            listening_with(&directory, Arc::clone(&power) as Arc<dyn PowerSave>).await;
        assert_eq!(power.held(), 0, "the switch is off by default");

        web.set_keep_awake(true).await;
        assert_eq!(power.held(), 1);
        web.set_keep_awake(false).await;
        assert_eq!(power.held(), 0);
        web.set_keep_awake(true).await;
        assert_eq!(power.held(), 1);

        web.stop().await;
        assert_eq!(power.held(), 0, "stopping the surface drops what it held");
        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn playback_holds_until_a_minute_after_the_last_media_request() {
        let directory = TempDir::new().unwrap();
        let power = Arc::new(RecordingPower::default());
        let (web, runtime, _port) =
            listening_with(&directory, Arc::clone(&power) as Arc<dyn PowerSave>).await;

        web.touch_stream();
        assert_eq!(power.held(), 1, "a stream keeps the machine awake");

        let now = web.now();
        {
            let mut state = web
                .power_state
                .lock()
                .expect("the power state lock is not poisoned");
            state.last_stream_ms = now - super::STREAM_POWER_WINDOW_MS - 1;
        }
        web.tick_power();
        assert_eq!(power.held(), 0, "an idle player releases it");

        web.stop().await;
        runtime.shutdown().await;
    }

    #[test]
    fn a_device_name_is_trimmed_and_refused_when_it_is_blank_long_or_control() {
        assert_eq!(
            super::device_name("  Living room  ").unwrap(),
            "Living room"
        );
        assert!(matches!(
            super::device_name("   "),
            Err(WebError::InvalidName)
        ));
        assert!(matches!(
            super::device_name("TV"),
            Err(WebError::InvalidName)
        ));
        assert!(matches!(
            super::device_name(&"a".repeat(65)),
            Err(WebError::InvalidName)
        ));
        assert!(super::device_name(&"a".repeat(64)).is_ok());
    }

    fn track(id: i64, language: &str, forced: bool, is_default: bool) -> WebSubtitleTrack {
        WebSubtitleTrack {
            id,
            language: language.to_owned(),
            label: None,
            format: "srt".to_owned(),
            forced,
            sdh: false,
            is_default,
        }
    }

    #[test]
    fn the_subtitle_default_follows_the_account_preference() {
        let tracks = vec![
            track(1, "eng", false, false),
            track(2, "fra", true, false),
            track(3, "deu", false, true),
        ];
        let auto = PlaybackPreference::new("en", "fr", SubtitleMode::Auto).expect("a preference");
        assert_eq!(
            chosen_subtitle(&tracks, &auto),
            Some(2),
            "a language match beats the row's own default"
        );

        let english =
            PlaybackPreference::new("en", "en", SubtitleMode::Auto).expect("a preference");
        assert_eq!(chosen_subtitle(&tracks, &english), Some(1));

        let unmatched =
            PlaybackPreference::new("en", "es", SubtitleMode::Auto).expect("a preference");
        assert_eq!(
            chosen_subtitle(&tracks, &unmatched),
            Some(3),
            "with no language match the row's default is chosen"
        );

        let off = PlaybackPreference::new("en", "fr", SubtitleMode::Off).expect("a preference");
        assert_eq!(chosen_subtitle(&tracks, &off), None);

        let forced =
            PlaybackPreference::new("en", "fr", SubtitleMode::Forced).expect("a preference");
        assert_eq!(chosen_subtitle(&tracks, &forced), Some(2));

        let forced_other =
            PlaybackPreference::new("en", "de", SubtitleMode::Forced).expect("a preference");
        assert_eq!(
            chosen_subtitle(&tracks, &forced_other),
            Some(2),
            "a forced preference with no matching forced track takes the first forced one"
        );

        let english_only = vec![track(1, "eng", false, false)];
        let none = PlaybackPreference::new("en", "es", SubtitleMode::Auto).expect("a preference");
        assert_eq!(chosen_subtitle(&english_only, &none), None);

        let undetermined = vec![track(1, "und", false, false)];
        assert_eq!(
            chosen_subtitle(&undetermined, &none),
            Some(1),
            "a lone undetermined sidecar is still the only subtitles the file has"
        );
    }

    #[test]
    fn exactly_one_served_track_is_marked_default_and_the_url_carries_the_offset() {
        let tracks = vec![track(1, "eng", false, false), track(2, "fra", false, true)];
        let preference =
            PlaybackPreference::new("en", "fr", SubtitleMode::Auto).expect("a preference");
        let served = subtitle_dtos(tracks, &preference, 7, Some(9), 4_000);
        assert_eq!(served.iter().filter(|track| track.r#default).count(), 1);
        assert!(served[1].r#default);
        assert_eq!(
            served[0].url,
            "/api/subtitle/1?titleId=7&episodeId=9&offsetMs=4000"
        );
        let json = serde_json::to_value(&served).expect("serializes");
        assert_eq!(json[0]["default"], serde_json::json!(false));
        assert_eq!(json[1]["default"], serde_json::json!(true));
    }

    #[test]
    fn a_title_url_omits_an_episode_it_does_not_have() {
        assert_eq!(
            subtitle_url(3, 4, None, 0),
            "/api/subtitle/3?titleId=4&offsetMs=0"
        );
    }

    fn episode(season_number: u32, episode_number: u32) -> EpisodeDto {
        EpisodeDto {
            season_number,
            episode_number,
            name: None,
            overview: None,
            still: None,
            runtime_minutes: None,
            rating: None,
            air_date: None,
            files: 0,
            percent: 0.0,
            position_ms: 0,
            watched: false,
            playable: false,
            download_state: None,
            download_percent: None,
        }
    }

    #[test]
    fn a_series_download_state_aggregates_the_listed_episodes() {
        let mut downloads = TitleDownloads::default();
        let episodes = vec![episode(1, 1), episode(1, 2)];
        assert_eq!(downloads.summary(true, &episodes), (None, None));

        downloads.episodes.insert(
            (1, 1),
            JobDownload {
                state: "completed".to_owned(),
                percent: 100.0,
            },
        );
        downloads.episodes.insert(
            (1, 2),
            JobDownload {
                state: "downloading".to_owned(),
                percent: 42.0,
            },
        );
        assert_eq!(
            downloads.summary(true, &episodes),
            (Some("downloading".to_owned()), Some(42.0)),
            "the first active episode wins"
        );

        downloads.episodes.insert(
            (1, 2),
            JobDownload {
                state: "completed".to_owned(),
                percent: 100.0,
            },
        );
        assert_eq!(
            downloads.summary(true, &episodes),
            (Some("completed".to_owned()), Some(100.0))
        );

        assert_eq!(
            downloads.summary(false, &episodes),
            (None, None),
            "a film does not aggregate episodes"
        );

        let incomplete = vec![episode(1, 1), episode(1, 3)];
        assert_eq!(
            downloads.summary(true, &incomplete),
            (None, None),
            "an episode with no job is not done"
        );

        downloads.title = Some(JobDownload {
            state: "paused".to_owned(),
            percent: 12.5,
        });
        assert_eq!(
            downloads.summary(true, &episodes),
            (Some("paused".to_owned()), Some(12.5)),
            "a title-level job is the page's answer"
        );
    }
}
