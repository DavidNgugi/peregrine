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
use std::net::{IpAddr, SocketAddr};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use peregrine_domain::catalogue::TitleKind;
use peregrine_domain::{Clock, LocalPlayback, SessionToken, TokenSource};
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
    /// The device was not approved to start downloads.
    #[error("this device may not start downloads")]
    NotPermitted,
    /// The request named an episode incompletely or an impossible season.
    #[error("the download request is not valid")]
    InvalidRequest,
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
}

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
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<()>,
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
        *self
            .hosts
            .lock()
            .expect("the host list lock is not poisoned") =
            allowed_hosts(address, config.hostname.as_deref());
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
        *running = Some(Running {
            address,
            shutdown,
            task,
        });
        drop(running);
        info!(%address, "the LAN surface is listening");
        Ok(self.status().await)
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
        WebStatus {
            running: running.is_some(),
            address: running.as_ref().map(|running| running.address),
            pending,
        }
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

        if hero.is_none()
            && let Some(first) = library.first()
            && let Some(detail) = self.runtime.library_title(first.id).await?
        {
            hero = Some(tile_from_detail(detail, None, 0));
        }

        // The desktop's hero rotates through several titles; collect one candidate per title that
        // has a backdrop, keeping the rails' order so the rotation starts on the same film the
        // rest of the screen leads with.
        let heroes = hero_candidates(&rails, hero.as_ref());
        if hero.is_none() {
            hero = heroes.first().cloned();
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
        Ok(self
            .runtime
            .search_tiles(&query, 60, kind)
            .await?
            .into_iter()
            .map(tile_from_rail)
            .collect())
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
        if let Ok(answer) = self.runtime.browse(&request).await {
            return Ok(BrowseDto {
                items: answer.items.into_iter().map(tile_from_rail).collect(),
                has_more: answer.has_more,
                source: "provider".to_owned(),
            });
        }
        // No provider, or a provider having a bad minute: the library is still a browse screen.
        let items = self
            .runtime
            .library_titles(None, 100)
            .await?
            .into_iter()
            .filter(|entry| entry.row.title.kind == kind)
            .map(|entry| tile_from_entry(&entry))
            .collect();
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
        let (seasons, episodes) = self.title_seasons_page(id, is_show, season).await?;
        let refreshing = is_show && seasons.is_empty() && !identity.metadata_state.has_details();
        if refreshing {
            let start_now = self
                .refresh_started
                .lock()
                .expect("the refresh lock is not poisoned")
                .insert(id);
            if start_now
                && let Err(error) = self
                    .runtime
                    .start_provider_refresh(id, TitleKind::Show)
                    .await
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
        );
        Ok((seasons, episodes))
    }

    async fn title_similar_page(
        &self,
        id: i64,
        in_library: bool,
    ) -> Result<(Vec<TileDto>, Vec<GenreDto>), WebError> {
        if !in_library {
            return Ok((Vec::new(), Vec::new()));
        }
        // A title page is useful without the network: the provider's similar list is a bonus, so
        // a slow or unreachable provider gets three seconds and then an empty rail, not a spinner.
        let (similar, genres): (Vec<TileDto>, Vec<GenreDto>) =
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
        if !similar.is_empty() {
            return Ok((similar, genres));
        }
        // A provider with nothing to say should not leave the page without a rail. The library's
        // other titles are a truthful "more like this" and cost one local read.
        let similar = self
            .runtime
            .library_titles(None, 20)
            .await?
            .into_iter()
            .filter(|entry| entry.row.title.id != id)
            .map(|entry| tile_from_entry(&entry))
            .collect();
        Ok((similar, genres))
    }
    /// Mints a playback session for one device and one title.
    ///
    /// # Errors
    ///
    /// [`WebError::NoFile`], [`WebError::TranscodeRequired`], [`WebError::Unsupported`], or a
    /// runtime failure.
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
        let Some(target) = target else {
            return Err(WebError::NoFile);
        };
        let start = target.duration_ms.map_or(position_ms.max(0), |duration| {
            position_ms.clamp(0, duration)
        });
        // A container the webview demuxes is served as bytes; everything else is transcoded. The
        // choice is the domain's `LocalPlayback` rule, not this module's codec table.
        // The container is the first question; the streams inside it are the second. An MP4
        // carrying HEVC or AC3 demuxes and still fails, so a probe that names another codec goes
        // to the transcoder just as an MKV does.
        if target.playback != LocalPlayback::Webview || !transcode::browser_safe(&target.path).await
        {
            return self.start_hls(device, &target, start).await;
        }
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
        Ok(PlaybackTicket {
            kind: "direct",
            url: format!("/media/{}", token.as_str()),
            mime: mime.to_owned(),
            start_position_ms: start,
            duration_ms: target.duration_ms,
            episode_id: target.episode_id,
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
    async fn start_hls(
        &self,
        device: &PairedDevice,
        target: &WebPlaybackTarget,
        start_ms: i64,
    ) -> Result<PlaybackTicket, WebError> {
        let transcoder = self.transcoder().await?;
        // One session per device: a resume or a seek replaces the previous transcode rather than
        // leaving two encoders fighting for the same GPU.
        self.evict_hls_device(device.id).await;
        let token = self.tokens.next_token();
        let directory = self.transcode_root.join(token.as_str());
        let mut child = transcoder
            .start(&directory, &target.path, start_ms)
            .await
            .map_err(|error| {
                warn!(%error, "the transcode could not be started");
                WebError::TranscodeUnavailable
            })?;
        {
            let mut sessions = self.hls.lock().await;
            if sessions.len() >= HLS_SESSION_LIMIT {
                drop(sessions);
                let _ = child.kill().await;
                let _ = tokio::fs::remove_dir_all(&directory).await;
                return Err(WebError::TranscodeBusy);
            }
            sessions.insert(
                token.as_str().to_owned(),
                HlsSession {
                    dir: directory,
                    child,
                    device_id: device.id,
                    expires_at_ms: self.now() + PLAYBACK_TTL_MS,
                },
            );
        }
        Ok(PlaybackTicket {
            kind: "hls",
            url: format!("/hls/{}/index.m3u8", token.as_str()),
            mime: "application/vnd.apple.mpegurl".to_owned(),
            start_position_ms: start_ms,
            duration_ms: target.duration_ms,
            episode_id: target.episode_id,
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
        sessions.get(token).map(|grant| MediaGrant {
            device_id: grant.device_id,
            path: grant.path.clone(),
            size_bytes: grant.size_bytes,
            mime: grant.mime,
        })
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
) -> Vec<EpisodeDto> {
    episodes
        .into_iter()
        .map(|episode| {
            let row = episode.row;
            let found = progress.iter().find(|held| {
                held.season_number == row.season_number.get()
                    && held.episode_number == row.episode_number.get()
            });
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
            }
        })
        .collect()
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
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use peregrine_domain::SessionToken;

    use super::{code_for, hash_token, mime_for, scope_allows};

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
}
