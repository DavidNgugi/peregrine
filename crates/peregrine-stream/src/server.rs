//! One file, one client, one session: the loopback media server.
//!
//! **What this is for.** The webview's own media stack is what plays a local file in this build
//! (ADR [0023](../../../docs/adr/0023-in-app-video-surface.md)), and it needs a URL. The asset
//! protocol deliberately never exposes the library root — `ARCHITECTURE` §17 scopes it to the image
//! cache and nothing else — so the bytes travel over loopback HTTP instead, which is the other channel
//! §17 names for media.
//!
//! # The four controls, and why each one is here
//!
//! SECURITY.md T10 names them, and this module is where they exist rather than where they are
//! described.
//!
//! * **Loopback-only bind.** `127.0.0.1` and port 0: the socket is unreachable from the network, and
//!   the ephemeral port means two sessions cannot collide.
//! * **A per-session token, in the request path.** A `<video>` element cannot set an `Authorization`
//!   header, so a header-borne bearer token is not available to the only client this has. §2 of the
//!   ADR records the concession and its bounds. The token is compared byte for byte
//!   ([`SessionToken::matches`]).
//! * **A validated `Host`.** This is the half that actually refuses a DNS-rebinding request: a page at
//!   `evil.com` that resolves to `127.0.0.1` reaches this socket, but the browser sets `Host` from the
//!   URL it fetched, so the request arrives naming `evil.com` and is refused before the token is even
//!   read.
//! * **One file, one socket, for the session.** The path is decided when the server starts and the
//!   router serves that path whatever the request asks for, so there is no route that takes a file
//!   name and therefore no traversal to defend against. The listener stops when the session does.
//!
//! # What is deliberately absent
//!
//! No directory listing, no second file, and no `Range` handling of our own: byte ranges,
//! `Content-Range`, `Accept-Ranges`, `HEAD`, and the `416` for an unsatisfiable range are HTTP, and
//! `tower-http`'s `ServeFile` is where they are implemented and tested. `cargo xtask check-arch` permits
//! that crate for this one, and `AGENTS R2` is satisfied by the fact that it replaces code rather than
//! wrapping it.
//!
//! **One write route exists** — a frame the renderer captured, posted back so the film becomes its own
//! poster — and **one CORS policy**, because a canvas cannot read a cross-origin frame without it. Both
//! are §6 of the ADR, both are bounded by the same token as the read, and the route is absent from a
//! session whose caller has nowhere to store a frame.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures::stream;
use peregrine_domain::{SafePath, SessionToken};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeFile;
use tower_http::set_header::SetResponseHeaderLayer;
use tracing::{debug, info, warn};

use crate::error::StreamError;
use crate::thumbnail::{ThumbnailError, ThumbnailSink};

/// The route a session's file is served at.
///
/// The token is the whole of the variable part, which is what makes the route and the credential the
/// same thing: there is no path this server accepts that is not "here is the token".
const MEDIA_ROUTE: &str = "/media/{token}";

/// The prefix [`MEDIA_ROUTE`] matches, for the guard to strip the token out of a request path.
const MEDIA_PREFIX: &str = "/media/";

/// A boxed future that stays sendable across the server's tasks.
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A reader over one range of a torrent-backed file, opened fresh for each HTTP request.
///
/// The engine's reader has no seek — a different offset is a new stream — so each range request is
/// answered by opening a reader at the range's start rather than moving one. `Ok(None)` is the end
/// of the file; a read that has bytes answers `Ok(Some(..))`.
pub trait MediaReader: Send {
    /// Reads up to `max` bytes from the current position.
    fn read(&mut self, max: usize) -> BoxFuture<'_, io::Result<Option<Vec<u8>>>>;
}

/// A torrent-backed file the server can open readers over, and the four facts serving one needs.
///
/// Implemented in `peregrine-app` over the engine, so this crate keeps no torrent dependency: the
/// server needs a length, a content type, and a way to open a reader, and nothing else (`AGENTS R8`).
pub trait TorrentMedia: Send + Sync + 'static {
    /// The file's length in bytes, as the torrent's metadata reports it.
    fn len(&self) -> u64;

    /// Whether the file holds no bytes. A torrent with no selected file never reaches the server.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The content type a media element is told the bytes are.
    fn content_type(&self) -> &str;

    /// Opens a reader starting at `offset`.
    fn open(&self, offset: u64) -> BoxFuture<'_, io::Result<Box<dyn MediaReader>>>;
}

/// The route a captured poster frame is posted to, by the renderer that captured it.
///
/// The same token as [`MEDIA_ROUTE`], and deliberately its own route rather than a body on the media
/// route: a `GET` of a film and a `POST` of a still are different verbs on different things, and one
/// route that meant both would be a route whose method decided what the body was.
const THUMBNAIL_ROUTE: &str = "/thumbnail/{token}";

/// The prefix [`THUMBNAIL_ROUTE`] matches, and the token's other home.
const THUMBNAIL_PREFIX: &str = "/thumbnail/";

/// The route one sidecar subtitle is served at.
///
/// The session token comes first, exactly as it does on the media and capture routes, so every byte
/// this listener hands out is behind the same per-session credential. The second segment is the
/// `subtitle_track` row id the session was opened with — **and only that**: the route never takes a
/// path, so a request cannot name a file the session did not register.
const SUBTITLE_ROUTE: &str = "/subtitle/{token}/{track}";

/// The prefix [`SUBTITLE_ROUTE`] matches, for the guard to strip the token out of a request path.
const SUBTITLE_PREFIX: &str = "/subtitle/";

/// The largest sidecar subtitle this build will serve in one response.
///
/// A text sidecar is kilobytes, and a full ASS track with styling can be a few megabytes; eight is an
/// order of magnitude over that and still a ceiling that stops a mislabelled library file from asking
/// this process to hold a whole film's worth of bytes. The route reads the file itself rather than
/// streaming it, so the bound is what makes that safe.
const MAX_SUBTITLE_BYTES: u64 = 8 * 1024 * 1024;

/// The largest frame this build will read from a capture.
///
/// A 640-wide JPEG at quality 0.8 is around 60 KB, so this is an order of magnitude of headroom and
/// still a bound: a body limit is the difference between a route that stores a poster and a route a
/// page can be made to fill this process's memory through. `axum`'s own default is 2 MB; naming it
/// here is what makes the number the build's decision rather than the framework's.
const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;

/// How long one engine read may block before the response is aborted.
///
/// A torrent whose peers have gone silent would otherwise hold the HTTP body open forever, and a
/// player cannot retry what looks like a slow read. Thirty seconds is longer than one piece takes on a
/// healthy swarm and short enough that a dead one becomes a network error the player can act on.
const TORRENT_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// A JPEG's own first bytes: the start-of-image marker and the first segment marker.
const JPEG_START: [u8; 3] = [0xff, 0xd8, 0xff];

/// A JPEG's last two bytes, the end-of-image marker.
///
/// **Both ends are checked, and the tail is the half that catches the real failure.** A capture whose
/// element died mid-encode posts a truncated JPEG, which a start-marker-only check would store happily —
/// a file that renders as a broken image and reports nothing. `FF D9` cannot appear at the end of any
/// other segment, so requiring it is a structural check rather than a guess.
///
/// **Still not a decode, and deliberately so.** Sniffing is what `PG-E11-01` does for a download, and a
/// full decode here would need an image crate this crate has no business holding. What this refuses is
/// anything that is not a whole JPEG by its own framing; it does not promise the pixels are valid, which
/// is the same guarantee the provider path has for a poster it fetched.
const JPEG_END: [u8; 2] = [0xff, 0xd9];

/// One sidecar subtitle a session can serve.
///
/// The path arrives as a [`SafePath`], which only `PathGuard` can produce, so a session cannot be
/// handed a sidecar that has not already been shown to be inside a configured root — the same rule the
/// media file follows. The id is the row id the track DTO carries, which is what the route serves and
/// what makes an unregistered request a `404` rather than a path read.
#[derive(Clone)]
pub struct SidecarSubtitle {
    /// The `subtitle_track` row id, which is the route's only variable segment.
    pub track_id: i64,
    /// The sidecar file.
    pub path: SafePath,
    /// The content type the bytes are, from the row's stored format.
    pub content_type: &'static str,
}

/// The frame capture a session offers, when its caller can store one.
#[derive(Clone)]
pub struct Capture {
    /// Which title a captured frame is the poster of.
    ///
    /// The title rather than the file, because that is what the reads that draw a poster key on
    /// (`ImageKey::for_local_title`), and because a title with two copies has one poster.
    pub title_id: i64,
    /// Where the frame goes.
    pub sink: Arc<dyn ThumbnailSink>,
}

/// The host names a request may carry, in the forms a client on this machine spells them.
///
/// `[::1]` is here although the listener is IPv4: the check is about what a *browser* would send, and
/// a document served from `http://[::1]:port/` reaching an IPv4 socket is not an attack.
const LOOPBACK_HOSTS: &[&str] = &["127.0.0.1", "localhost", "[::1]"];

/// What every request is checked against, and what a capture needs.
#[derive(Clone)]
struct Guard {
    token: SessionToken,
    /// The port this listener is bound to, so a `Host` that states a different one is refused.
    port: u16,
    /// The capture this session offers, or `None` when its caller has nowhere to put a frame.
    ///
    /// `None` is why the route answers `404` rather than existing in a broken state: a session opened
    /// by something that cannot store a poster does not offer one, and a route that said so with a
    /// different status would be telling a caller about a capability it does not have.
    capture: Option<Capture>,
    /// The sidecar subtitles this session serves, by `subtitle_track` row id.
    ///
    /// Empty for a session opened without any, and for a torrent-backed session, which has no library
    /// file row for sidecars to hang from. An id that is not here is a route this session does not offer.
    subtitles: Arc<HashMap<i64, SidecarSubtitle>>,
    /// The torrent-backed media this session serves, or `None` when it serves a local file.
    ///
    /// `None` is the file case rather than a broken one: a file session's bytes come from
    /// `ServeFile`, which owns the whole route, and the handler below is only reached when this is
    /// `Some`.
    media: Option<Arc<dyn TorrentMedia>>,
    /// Whether the first request of this session has been logged yet.
    ///
    /// **The one line a black rectangle needs.** A renderer that fails to play reports a code and not a
    /// reason, and the first question is always whether the bytes were asked for at all — a request that
    /// arrived and was refused, and a request that never arrived, look identical on screen. Logged once
    /// rather than per request because a film is a stream of range requests and the answer does not change
    /// between them.
    asked: Arc<AtomicBool>,
}

/// What a session serves: one local file, or one torrent-backed file.
///
/// The bytes' origin is the only difference the rest of the type cares about — the token, the
/// listener, the capture, and the lifetime are the same — so it is one field rather than two types.
enum Served {
    File(PathBuf),
    Torrent,
}

/// One file, served over loopback HTTP for as long as one playback session lasts.
///
/// The socket is bound when this is built and closed when it is dropped or shut down, whichever comes
/// first: a session that is replaced cannot leave a listener behind, and a session the runtime forgets
/// to shut down still cannot outlive its own value.
pub struct MediaServer {
    served: Served,
    token: SessionToken,
    /// Whether a `Capture` was supplied, kept here because the router takes ownership of the guard that
    /// holds it: a caller asking "can this session keep a poster" must not have to ask the router.
    capture: bool,
    address: SocketAddr,
    // Both are `Option` because `Drop` is implemented: a type with a destructor cannot be taken apart
    // field by field, and `shutdown` needs to move the handle out to await it.
    stop: Option<oneshot::Sender<()>>,
    running: Option<tokio::task::JoinHandle<Result<(), io::Error>>>,
}

impl MediaServer {
    /// Binds a loopback listener that serves one file to whoever holds `token`.
    ///
    /// The file arrives as a [`SafePath`], which only `PathGuard` can produce, so this cannot be
    /// handed a path that has not already been shown to be inside a configured root. It is checked
    /// for existence here as well, because `media_file.missing_since` records what a *scan* saw and
    /// not what is true now.
    ///
    /// # Errors
    ///
    /// [`StreamError::Missing`] when the file is gone, [`StreamError::NotAFile`] when the path is a
    /// directory, and [`StreamError::Open`] or [`StreamError::Bind`] when the operating system
    /// refuses.
    ///
    /// `capture` is what makes the session able to keep a poster: `None` serves the file and offers no
    /// way to store a frame, which is what a caller with no image cache gets rather than a route that
    /// fails when it is used.
    ///
    /// `subtitles` are the sidecars this session may serve. Each one is already a [`SafePath`], and the
    /// route serves only the ids in this list: an id the caller did not register is a `404`, which is
    /// what keeps a request from naming a file the session was not opened for.
    pub async fn start(
        file: &SafePath,
        token: SessionToken,
        subtitles: Vec<SidecarSubtitle>,
        capture: Option<Capture>,
    ) -> Result<Self, StreamError> {
        let path = file.as_path().to_path_buf();
        match tokio::fs::metadata(&path).await {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => return Err(StreamError::NotAFile { path }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(StreamError::Missing { path });
            }
            Err(source) => return Err(StreamError::Open { path, source }),
        }

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|source| StreamError::Bind { source })?;
        let address = listener
            .local_addr()
            .map_err(|source| StreamError::Bind { source })?;

        let has_capture = capture.is_some();
        let subtitle_count = subtitles.len();
        let guard = Guard {
            token: token.clone(),
            port: address.port(),
            capture,
            subtitles: Arc::new(
                subtitles
                    .into_iter()
                    .map(|subtitle| (subtitle.track_id, subtitle))
                    .collect(),
            ),
            media: None,
            asked: Arc::new(AtomicBool::new(false)),
        };
        let app = Router::new()
            // `route_service` rather than a handler that calls the service: the route table is then the
            // only thing that decides which requests reach the file, and a path this build did not
            // write is a path that matches nothing and falls through to a `404` with no body.
            .route_service(MEDIA_ROUTE, ServeFile::new(&path))
            .route(THUMBNAIL_ROUTE, post(capture_frame))
            .route(SUBTITLE_ROUTE, get(serve_subtitle))
            .with_state(guard.clone())
            // **`no-store` on the *media* route, and deliberately not on the capture.** They are
            // different kinds of response: a film must never be written to a cache on the way to an
            // element, and the acknowledgement of a stored poster carries nothing to cache either, so
            // both get it — the layer below is applied to the whole router for that reason.
            .layer(SetResponseHeaderLayer::overriding(
                header::CACHE_CONTROL,
                HeaderValue::from_static("no-store"),
            ))
            // A media file served from a route a token authorises is the one place where a sniffed
            // content type could matter, because the client is an element that acts on what it is told
            // the bytes are.
            .layer(SetResponseHeaderLayer::overriding(
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ))
            .layer(DefaultBodyLimit::max(MAX_FRAME_BYTES))
            .layer(middleware::from_fn_with_state(guard, authorize))
            // **CORS, outermost, and for exactly one reason recorded in the ADR:** a frame drawn from a
            // cross-origin `<video>` taints the canvas it is drawn into, and a tainted canvas refuses
            // `toBlob` — so without a CORS-readable media response the renderer cannot turn a frame of
            // the film into the film's poster at all. `Any` origin is safe here for the reason the token
            // is: the request already has to carry a per-session credential in its path, and a page
            // without one is refused by the guard before CORS is consulted. It is outermost so a
            // preflight is answered rather than refused as a method the route does not have.
            .layer(
                CorsLayer::new()
                    .allow_origin(Any)
                    .allow_methods([Method::GET, Method::HEAD, Method::POST])
                    // Range makes a cross-origin video seekable; the element sets crossorigin for its
                    // text tracks, so the media request is under CORS and needs Range allowed and the
                    // range headers exposed, or it paints an error over a file the server can serve.
                    .allow_headers([header::CONTENT_TYPE, header::RANGE])
                    .expose_headers([
                        header::CONTENT_RANGE,
                        header::ACCEPT_RANGES,
                        header::CONTENT_LENGTH,
                    ]),
            );

        let (stop, stopped) = oneshot::channel();
        let running = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    // The sender being dropped is the same instruction as the message: nothing else
                    // holds it, so a runtime that forgets to call `shutdown` still stops the task when
                    // it drops the server.
                    let _ = stopped.await;
                })
                .await
        });

        // The port and nothing else: the file and the token are both values a log must not carry, and
        // the port is what anyone debugging a session actually needs to know.
        debug!(
            port = address.port(),
            subtitles = subtitle_count,
            "a media session is listening on loopback"
        );

        Ok(Self {
            served: Served::File(path),
            token,
            capture: has_capture,
            address,
            stop: Some(stop),
            running: Some(running),
        })
    }

    /// Binds a loopback listener that serves one torrent-backed file to whoever holds `token`.
    ///
    /// The bytes come from the engine rather than the filesystem, so a file that is still downloading
    /// can be played: every request — including each range a media element asks for — opens a fresh
    /// engine reader at that offset, because the engine's reader has no seek.
    ///
    /// # Errors
    ///
    /// [`StreamError::Bind`] when the operating system refuses the listener.
    pub async fn start_torrent(
        media: Arc<dyn TorrentMedia>,
        token: SessionToken,
        capture: Option<Capture>,
    ) -> Result<Self, StreamError> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|source| StreamError::Bind { source })?;
        let address = listener
            .local_addr()
            .map_err(|source| StreamError::Bind { source })?;

        let has_capture = capture.is_some();
        let guard = Guard {
            token: token.clone(),
            port: address.port(),
            capture,
            // A torrent has no library file row for a sidecar to belong to, so this session offers no
            // subtitle route.
            subtitles: Arc::new(HashMap::new()),
            media: Some(media),
            asked: Arc::new(AtomicBool::new(false)),
        };
        let app = Router::new()
            .route(MEDIA_ROUTE, get(serve_torrent))
            .route(THUMBNAIL_ROUTE, post(capture_frame))
            .with_state(guard.clone())
            .layer(SetResponseHeaderLayer::overriding(
                header::CACHE_CONTROL,
                HeaderValue::from_static("no-store"),
            ))
            .layer(SetResponseHeaderLayer::overriding(
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ))
            .layer(DefaultBodyLimit::max(MAX_FRAME_BYTES))
            .layer(middleware::from_fn_with_state(guard, authorize))
            .layer(
                CorsLayer::new()
                    .allow_origin(Any)
                    .allow_methods([Method::GET, Method::HEAD, Method::POST])
                    // Range makes a cross-origin video seekable; the element sets crossorigin for its
                    // text tracks, so the media request is under CORS and needs Range allowed and the
                    // range headers exposed, or it paints an error over a file the server can serve.
                    .allow_headers([header::CONTENT_TYPE, header::RANGE])
                    .expose_headers([
                        header::CONTENT_RANGE,
                        header::ACCEPT_RANGES,
                        header::CONTENT_LENGTH,
                    ]),
            );

        let (stop, stopped) = oneshot::channel();
        let running = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = stopped.await;
                })
                .await
        });

        debug!(
            port = address.port(),
            "a torrent media session is listening on loopback"
        );

        Ok(Self {
            served: Served::Torrent,
            token,
            capture: has_capture,
            address,
            stop: Some(stop),
            running: Some(running),
        })
    }

    /// The URL the renderer points its media element at.
    ///
    /// The token is a path segment, so this is the one place a URL is built and the one place to audit
    /// when the route changes.
    #[must_use]
    pub fn url(&self) -> String {
        format!(
            "http://{}{}{}",
            self.address,
            MEDIA_PREFIX,
            self.token.as_str()
        )
    }

    /// The URL a captured frame is posted to.
    ///
    /// Built here rather than assembled by the renderer from [`MediaServer::url`], because a renderer
    /// that replaced one path segment with another would be a second place the route is spelled — and
    /// the token, which is the part that must not be duplicated by hand, is in both.
    #[must_use]
    pub fn thumbnail_url(&self) -> String {
        format!(
            "http://{}{}{}",
            self.address,
            THUMBNAIL_PREFIX,
            self.token.as_str()
        )
    }

    /// The URL one sidecar subtitle track is served at.
    ///
    /// Answers a URL for any id: the session decides what it serves, and the handler refuses an id that
    /// was not registered with a `404`. Building it here rather than in a renderer keeps the token, and
    /// the route, in the one place that already spells both.
    #[must_use]
    pub fn subtitle_url(&self, track_id: i64) -> String {
        format!(
            "http://{}{}{}/{}",
            self.address,
            SUBTITLE_PREFIX,
            self.token.as_str(),
            track_id
        )
    }

    /// Whether this session offers a way to keep a captured poster.
    #[must_use]
    pub const fn has_capture(&self) -> bool {
        self.capture
    }

    /// The port the listener is bound to.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.address.port()
    }

    /// The session token a request must carry.
    #[must_use]
    pub const fn token(&self) -> &SessionToken {
        &self.token
    }

    /// The file being served, or `None` for a torrent-backed session, which has no path.
    #[must_use]
    pub fn file(&self) -> Option<&std::path::Path> {
        match &self.served {
            Served::File(path) => Some(path),
            Served::Torrent => None,
        }
    }

    /// Stops the listener and waits for the task to finish.
    ///
    /// Returns whether the listener stopped cleanly. A failure is reported rather than returned
    /// because there is nothing a caller can do about it: the session is over either way, and the
    /// socket is closed by the task ending whether or not it ended well.
    pub async fn shutdown(mut self) -> bool {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        match self.running.take() {
            Some(handle) => match handle.await {
                Ok(Ok(())) => true,
                Ok(Err(source)) => {
                    warn!(port = self.address.port(), %source, "the media listener stopped with an error");
                    false
                }
                Err(error) => {
                    warn!(port = self.address.port(), %error, "the media listener's task did not finish");
                    false
                }
            },
            None => true,
        }
    }
}

impl Drop for MediaServer {
    fn drop(&mut self) {
        // Fire and forget: nothing is waiting, but the listener must not outlive the session that owns
        // it even when a caller drops this instead of shutting it down.
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

impl fmt::Debug for MediaServer {
    /// Deliberately does not print the file: a path in a log line is SECURITY.md T13's business, and
    /// the port and the session's existence are what anyone debugging this needs.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MediaServer")
            .field("port", &self.address.port())
            .field("token", &self.token)
            .finish_non_exhaustive()
    }
}

/// The guard: a loopback `Host`, then the session's token, then the route's own work.
async fn authorize(State(guard): State<Guard>, request: Request, next: Next) -> Response {
    // **Every request is logged at debug, and nothing in the line is a secret.** A renderer that fails to
    // play is a black rectangle with no explanation — the element reports a code, not a reason — so the
    // first question is always whether the bytes were ever asked for. The path is logged without the token
    // (see `route_of`), the query is dropped, and the `Host` is logged as a yes/no rather than a value.
    let route = route_of(request.uri().path());
    if !host_is_loopback(request.headers(), guard.port) {
        warn!(%route, method = %request.method(), "a session request was refused: the Host is not loopback");
        return refusal(StatusCode::FORBIDDEN, "loopback Host required");
    }
    if !guard.token.matches(offered_token(request.uri().path())) {
        warn!(%route, method = %request.method(), "a session request was refused: no session token");
        return refusal(StatusCode::UNAUTHORIZED, "session token required");
    }
    // Once per session at `info`, and every later request at `debug`: "the renderer asked for the bytes"
    // is the fact that separates a refused stream from one that was never requested, and a film is a
    // stream of range requests after it.
    if guard.asked.swap(true, Ordering::Relaxed) {
        debug!(%route, method = %request.method(), "a session request was authorised");
    } else {
        info!(%route, method = %request.method(), "the renderer asked this session for its file");
    }
    next.run(request).await
}

/// The route a path names, with the token left out.
///
/// The token is a bearer credential and a log line is the last place it belongs (SECURITY.md T13), so the
/// diagnostic names the route and never the value in it. A path this build did not write is reported as
/// itself, which is what makes "the renderer asked for something unexpected" visible.
fn route_of(path: &str) -> &'static str {
    if path.starts_with(MEDIA_PREFIX) {
        return MEDIA_ROUTE;
    }
    if path.starts_with(THUMBNAIL_PREFIX) {
        return THUMBNAIL_ROUTE;
    }
    if path.starts_with(SUBTITLE_PREFIX) {
        return SUBTITLE_ROUTE;
    }
    "unrecognised"
}

/// The credential a request path carries, or the empty string when it carries none.
///
/// Every route puts the token immediately after its own prefix, so every route is read here rather than
/// in three `strip_prefix` calls that could disagree — and a path this build did not write answers the
/// empty string, which no minted token can be, so it is refused by the same comparison as a wrong one.
///
/// The token is the **first** segment after the prefix, not the whole remainder: the subtitle route puts
/// the track id after it. A route whose remainder has further segments still has to match its route
/// pattern to be served at all, which is the router's job and not this comparison's.
fn offered_token(path: &str) -> &str {
    for prefix in [MEDIA_PREFIX, THUMBNAIL_PREFIX, SUBTITLE_PREFIX] {
        if let Some(offered) = path.strip_prefix(prefix) {
            return offered.split('/').next().unwrap_or_default();
        }
    }
    ""
}

/// Stores a poster frame the renderer captured from the film it is playing.
///
/// **The body is bounded and sniffed before it reaches the sink.** The sink writes into the directory
/// the asset protocol is scoped to — `docs/ARCHITECTURE.md` §17 — so a blob that lands there is a blob
/// the webview can be told to render, and "the renderer sent it" is not a reason to skip the check
/// (`AGENTS R21`: downloaded content, and this is content the renderer produced from a file, is not
/// trusted because of where it came from).
///
/// The stored answer is deliberately bodyless: the renderer does not need the path — the core resolves
/// that on the next read — and a path over the wire would be a filesystem path in a response, which is
/// the same rule `SECURITY.md` T13 keeps for logs.
async fn capture_frame(State(guard): State<Guard>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(capture) = guard.capture.clone() else {
        // A session whose caller has nowhere to put a frame does not offer this, and says so the way a
        // path it did not write says so.
        return refusal(StatusCode::NOT_FOUND, "no such route");
    };
    if !is_jpeg_content_type(&headers) {
        return refusal(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "a poster frame must be a JPEG",
        );
    }
    if body.is_empty() || body.len() > MAX_FRAME_BYTES {
        return refusal(StatusCode::PAYLOAD_TOO_LARGE, "that frame is not a frame");
    }
    if !body.starts_with(&JPEG_START) || !body.ends_with(&JPEG_END) {
        return refusal(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "a poster frame must be a whole JPEG",
        );
    }

    // Blocking work — a file write and a database row — off the runtime, which is the rule for every
    // filesystem call that is not already async (`AGENTS §5`).
    let stored = tokio::task::spawn_blocking(move || capture.sink.store(capture.title_id, &body))
        .await
        .map_err(|_| ThumbnailError::Record)
        .and_then(std::convert::identity);

    match stored {
        Ok(path) => {
            // The path is logged and not returned: a log line is where a maintainer looks when a poster
            // does not appear, and a response body is where a leak would be.
            debug!(title = capture.title_id, file = %path.display(), "a poster frame was stored");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => {
            warn!(title = capture.title_id, %error, "a poster frame was refused");
            // A refusal the renderer cannot act on is answered as one: it retries once per visit at
            // most, and a body would only be a string it does not read.
            refusal(StatusCode::BAD_REQUEST, "that frame could not be stored")
        }
    }
}

/// Serves one sidecar subtitle the session registered.
///
/// **The request names an id, never a path.** The path was fixed when the session opened, from a
/// [`SafePath`] `PathGuard` produced, so there is no file name on the wire to traverse with — and an id
/// the session did not register is the same answer as a route that does not exist.
///
/// The read is bounded by [`MAX_SUBTITLE_BYTES`] rather than streamed: a text track is small, and a
/// ceiling read in one piece is simpler than a second body-streaming path for bytes that are already
/// kilobytes.
async fn serve_subtitle(
    State(guard): State<Guard>,
    Path((_token, track)): Path<(String, String)>,
) -> Response {
    let Some(subtitle) = track
        .parse::<i64>()
        .ok()
        .and_then(|id| guard.subtitles.get(&id))
    else {
        return refusal(StatusCode::NOT_FOUND, "no such subtitle");
    };
    let path = subtitle.path.as_path();
    match tokio::fs::metadata(path).await {
        Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_SUBTITLE_BYTES => {}
        Ok(_) | Err(_) => return refusal(StatusCode::NOT_FOUND, "no such subtitle"),
    }
    match tokio::fs::read(path).await {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, subtitle.content_type)
            .body(Body::from(bytes))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        // A sidecar the scan indexed and something removed since: the track list the renderer already
        // holds is the stale half, and a 404 is what makes the element report a missing resource
        // rather than an empty one.
        Err(_) => refusal(StatusCode::NOT_FOUND, "no such subtitle"),
    }
}

/// Serves one range of a torrent-backed file, opening a fresh engine reader for the request.
///
/// The whole of the range handling the file path gets from `ServeFile` lives here, because a reader
/// of a torrent has no path for `ServeFile` to open and no way to seek one.
async fn serve_torrent(State(guard): State<Guard>, method: Method, headers: HeaderMap) -> Response {
    let Some(media) = guard.media.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let len = media.len();

    let ranged = headers.contains_key(header::RANGE);
    let (start, end) = match headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| parse_range(value, len))
    {
        Some(Ok(range)) => range,
        Some(Err(())) => {
            return Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{len}"))
                .body(Body::empty())
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
        None => (0, len.saturating_sub(1)),
    };

    let length = if len == 0 { 0 } else { end - start + 1 };
    let mut builder = Response::builder()
        .status(if ranged && len > 0 {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_TYPE, media.content_type())
        .header(header::CONTENT_LENGTH, length.to_string());
    if ranged && len > 0 {
        builder = builder.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"));
    }

    let body = if method == Method::HEAD || length == 0 {
        Body::empty()
    } else {
        let Ok(reader) = media.open(start).await else {
            // The engine would not open the file at this offset: a handle that went away, or a piece
            // the swarm cannot supply. A 503 says "try this range again" where a 200 with an empty
            // body would say "the file ended here".
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        };
        Body::from_stream(stream::unfold(
            (reader, length),
            |(mut reader, remaining)| async move {
                if remaining == 0 {
                    return None;
                }
                let want = usize::try_from(remaining.min(64 * 1024)).unwrap_or(64 * 1024);
                match tokio::time::timeout(TORRENT_READ_TIMEOUT, reader.read(want)).await {
                    Ok(Ok(Some(bytes))) if !bytes.is_empty() => {
                        let chunk = Bytes::from(bytes);
                        let served = u64::try_from(chunk.len()).unwrap_or(0);
                        Some((
                            Ok::<_, io::Error>(chunk),
                            (reader, remaining.saturating_sub(served)),
                        ))
                    }
                    // A clean end is the file's end. A read error or a stall is not: yielding it
                    // aborts the body, so the player sees a network failure it can retry instead of a
                    // short file it will treat as complete.
                    Ok(Ok(_)) => None,
                    Ok(Err(error)) => Some((Err(error), (reader, 0))),
                    Err(_) => Some((
                        Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "the torrent did not supply bytes in time",
                        )),
                        (reader, 0),
                    )),
                }
            },
        ))
    };

    builder
        .body(body)
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Parses a single-range `Range` header.
///
/// `None` serves the whole file, which covers a header this build does not understand and a
/// multi-range request a media element does not send. `Some(Err(()))` is a range that cannot be
/// satisfied, which is the one answer that is deliberately not the whole file.
fn parse_range(value: &str, len: u64) -> Option<Result<(u64, u64), ()>> {
    let spec = value.trim().strip_prefix("bytes=")?;
    if spec.contains(',') || len == 0 {
        return None;
    }
    let (start, end) = spec.trim().split_once('-')?;
    if start.is_empty() {
        let suffix: u64 = end.trim().parse().ok()?;
        if suffix == 0 {
            return Some(Err(()));
        }
        return Some(Ok((len.saturating_sub(suffix), len - 1)));
    }
    let start: u64 = start.trim().parse().ok()?;
    let end = if end.trim().is_empty() {
        len - 1
    } else {
        end.trim().parse().ok()?
    };
    if start > end || start >= len {
        return Some(Err(()));
    }
    Some(Ok((start, end.min(len - 1))))
}

/// Whether a capture declared itself as a JPEG.
///
/// The parameter is stripped before comparing, because `image/jpeg; charset=binary` is the same
/// declaration as `image/jpeg` and a check that refused it would be refusing a correct client.
fn is_jpeg_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or_default().trim())
        .is_some_and(|value| value.eq_ignore_ascii_case("image/jpeg"))
}

/// A refusal whose body says what is missing and nothing else: no path, no token, no hint about
/// whether the token was close.
fn refusal(status: StatusCode, message: &'static str) -> Response {
    (status, message).into_response()
}

/// Whether the request's `Host` names this machine and this listener's port.
///
/// Fails closed on every shape this build does not recognise: a missing header, a non-text value, a
/// host that is not a loopback spelling, a port that does not parse, and a port that is not ours.
fn host_is_loopback(headers: &HeaderMap, port: u16) -> bool {
    let Some(host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Some((name, stated)) = split_host(host) else {
        return false;
    };
    if stated.is_some_and(|stated| stated != port) {
        return false;
    }
    LOOPBACK_HOSTS
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(name))
}

/// A `Host` header split into its name and its port, or `None` when the port is present and unreadable.
///
/// An IPv6 literal keeps its colons inside brackets, so only a colon after the closing bracket can be
/// a port separator — which is why this does not simply split on the last `:`.
fn split_host(host: &str) -> Option<(&str, Option<u16>)> {
    let (name, tail) = match host.rfind(']') {
        Some(index) => host.split_at(index + 1),
        None => match host.rfind(':') {
            Some(index) => host.split_at(index),
            None => return Some((host, None)),
        },
    };
    match tail.strip_prefix(':') {
        None | Some("") => Some((name, None)),
        // `Host: 127.0.0.1:notaport` is a malformed request rather than a request without a port, and
        // treating it as the latter would accept a header this build cannot read.
        Some(text) => text.parse::<u16>().ok().map(|port| (name, Some(port))),
    }
}

#[cfg(test)]
mod tests;
