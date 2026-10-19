//! The routes the LAN surface answers, and the refusals around them.
//!
//! Every route is one of three kinds: the pairing ceremony before a device holds a token, the
//! read-only API behind authentication, and the media/image delivery whose credential is either the
//! device token (images) or a short-lived playback grant (video). There is no fourth kind, and no
//! route reaches the IPC command surface.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{
    ConnectInfo, DefaultBodyLimit, FromRequestParts, Path as UrlPath, Query, Request, State,
};
use axum::http::StatusCode;
use axum::http::header::{self, HeaderMap, HeaderName, HeaderValue};
use axum::http::request::Parts;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use peregrine_domain::catalogue::TitleKind;
use peregrine_providers::BrowseSort;
use peregrine_storage::PairedDevice;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio_util::io::ReaderStream;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;
use tracing::warn;

use super::{
    BrowseDto, DownloadDto, GenreDto, LibraryRowDto, MediaGrant, PairOffer, PairStatus,
    PlaybackTicket, TOKEN_COOKIE, TileDto, TitleDto, WebError, WebService,
};

/// The policy the client's own page runs under.
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; img-src 'self' data:; media-src 'self' blob:; style-src 'self' 'unsafe-inline'; script-src 'self'; worker-src 'self' blob:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";

/// Builds the router the listener serves.
pub fn router(service: Arc<WebService>, app_dir: Option<std::path::PathBuf>) -> Router {
    let api = Router::new()
        .route("/api/pair", post(pair_request))
        .route("/api/pair/{pair_id}", get(pair_status))
        .route("/api/home", get(home))
        .route("/api/library", get(library))
        .route("/api/watchlist", get(watchlist))
        .route("/api/downloads", get(downloads))
        .route("/api/search", get(search))
        .route("/api/browse", get(browse))
        .route("/api/genres", get(genres))
        .route("/api/title/{id}", get(title))
        .route("/api/playback/stop", post(stop_playback))
        .route("/api/playback/{id}", post(playback))
        .route("/api/progress", post(progress))
        .route("/img/{key}", get(image))
        .route("/media/{token}", get(media))
        .route("/hls/{token}/{file}", get(hls))
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ));

    let router = match app_dir {
        Some(directory) => {
            let index = ServeFile::new(directory.join("index.html"));
            api.fallback_service(ServeDir::new(directory).not_found_service(index))
        }
        None => api.fallback(no_client),
    };

    router
        .layer(middleware::from_fn_with_state(Arc::clone(&service), gate))
        .layer(DefaultBodyLimit::max(4 * 1024))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CONTENT_SECURITY_POLICY),
        ))
        .with_state(service)
}

/// Refuses a request whose Host or Origin is not this listener's own.
///
/// The Host check is what refuses DNS rebinding: a page at another name that resolves to this
/// address reaches the socket, but the browser sets Host from that name, and nothing in this build
/// accepts a name it did not bind. The Origin check is the second half for a request that carries
/// one, and there is no CORS surface at all.
async fn gate(State(service): State<Arc<WebService>>, request: Request, next: Next) -> Response {
    let Some(host) = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return ApiError::bad_request("host").into_response();
    };
    if !service.host_allowed(host) {
        return ApiError::forbidden("host").into_response();
    }
    if let Some(origin) = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        && !service.origin_allowed(origin)
    {
        return ApiError::forbidden("origin").into_response();
    }
    next.run(request).await
}

/// The JSON failure every route answers with.
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
        }
    }

    fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "the token is not valid",
        )
    }

    fn forbidden(what: &'static str) -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            match what {
                "origin" => "the request came from another origin",
                _ => "the request did not name this server",
            },
        )
    }

    fn bad_request(what: &'static str) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "bad_request",
            match what {
                "host" => "the request did not name a host",
                _ => "the request is not readable",
            },
        )
    }

    fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "the item is not here")
    }

    fn range() -> Self {
        Self::new(
            StatusCode::RANGE_NOT_SATISFIABLE,
            "range",
            "the requested range is outside the file",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(serde_json::json!({
            "code": self.code,
            "message": self.message,
        }));
        let mut response = (self.status, body).into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response
    }
}

impl From<WebError> for ApiError {
    fn from(error: WebError) -> Self {
        match error {
            WebError::Unauthorized => Self::unauthorized(),
            WebError::PairUnknown => Self::new(
                StatusCode::NOT_FOUND,
                "pair_unknown",
                "the pairing request is not known",
            ),
            WebError::PairExpired => Self::new(
                StatusCode::CONFLICT,
                "pair_expired",
                "the pairing request has expired",
            ),
            WebError::PairNotPending => Self::new(
                StatusCode::CONFLICT,
                "pair_used",
                "the pairing request is not waiting for approval",
            ),
            WebError::RateLimited => Self::new(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "too many pairing requests; wait a minute",
            ),
            WebError::PairingFull => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "pairing_full",
                "too many pairing requests are waiting",
            ),
            WebError::NoFile => Self::new(
                StatusCode::NOT_FOUND,
                "no_file",
                "the title has no playable file",
            ),
            WebError::UnknownTitle => Self::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "the title is not in the library",
            ),
            WebError::InvalidProgress => Self::new(
                StatusCode::CONFLICT,
                "invalid_progress",
                "the progress report does not match the playable file",
            ),
            WebError::TranscodeUnavailable => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "transcode_unavailable",
                "this host has no encoder available for a transcode",
            ),
            WebError::TranscodeBusy => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "transcode_busy",
                "too many transcodes are running; stop one and try again",
            ),
            WebError::TranscodeRequired => Self::new(
                StatusCode::CONFLICT,
                "transcode_required",
                "this file needs a transcode the browser cannot do",
            ),
            WebError::Unsupported => Self::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported",
                "the container is not playable in a browser",
            ),
            other => {
                warn!(%other, "a LAN request failed");
                Self::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "the core could not answer",
                )
            }
        }
    }
}

/// The device a request authenticated as.
struct Device(PairedDevice);

impl FromRequestParts<Arc<WebService>> for Device {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<WebService>,
    ) -> Result<Self, Self::Rejection> {
        let presented = presented_token(&parts.headers).ok_or_else(ApiError::unauthorized)?;
        state
            .authenticate(&presented)
            .await
            .map(Device)
            .map_err(ApiError::from)
    }
}

/// The token a request carries, from a bearer header or the paired browser's cookie.
fn presented_token(headers: &HeaderMap) -> Option<String> {
    if let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        && !token.trim().is_empty()
    {
        return Some(token.trim().to_owned());
    }
    let cookies = headers.get(header::COOKIE)?.to_str().ok()?;
    let prefix = format!("{TOKEN_COOKIE}=");
    for cookie in cookies.split(';') {
        let cookie = cookie.trim();
        if let Some(value) = cookie.strip_prefix(&prefix)
            && !value.is_empty()
        {
            return Some(value.to_owned());
        }
    }
    None
}

#[derive(Debug, Deserialize)]
struct PairBody {
    name: Option<String>,
    platform: Option<String>,
}

async fn pair_request(
    State(service): State<Arc<WebService>>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Json(body): Json<PairBody>,
) -> Result<Json<PairOffer>, ApiError> {
    let name = clean(body.name.as_deref().unwrap_or(""), 64);
    let name = if name.is_empty() {
        "Unknown device".to_owned()
    } else {
        name
    };
    let platform = body
        .platform
        .as_deref()
        .map(|platform| clean(platform, 32))
        .filter(|platform| !platform.is_empty());
    let offer = service.request_pair(address.ip(), &name, platform.as_deref())?;
    Ok(Json(offer))
}

async fn pair_status(
    State(service): State<Arc<WebService>>,
    UrlPath(pair_id): UrlPath<String>,
) -> Response {
    match service.pair_status(&pair_id) {
        PairStatus::Pending { expires_at_ms } => Json(PairStatusDto {
            state: "pending",
            token: None,
            name: None,
            expires_at_ms: Some(expires_at_ms),
        })
        .into_response(),
        PairStatus::Denied => Json(PairStatusDto {
            state: "denied",
            token: None,
            name: None,
            expires_at_ms: None,
        })
        .into_response(),
        PairStatus::Unknown => Json(PairStatusDto {
            state: "unknown",
            token: None,
            name: None,
            expires_at_ms: None,
        })
        .into_response(),
        PairStatus::Approved { token, name } => {
            let mut response = Json(PairStatusDto {
                state: "approved",
                token: token.clone(),
                name: Some(name),
                expires_at_ms: None,
            })
            .into_response();
            if let Some(token) = token {
                let cookie = format!(
                    "{TOKEN_COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age=31536000"
                );
                if let Ok(value) = HeaderValue::from_str(&cookie) {
                    response.headers_mut().insert(header::SET_COOKIE, value);
                }
            }
            response
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PairStatusDto {
    state: &'static str,
    token: Option<String>,
    name: Option<String>,
    expires_at_ms: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct PageQuery {
    after: Option<i64>,
    limit: Option<u32>,
}

async fn home(
    State(service): State<Arc<WebService>>,
    Device(device): Device,
) -> Result<Json<super::HomeDto>, ApiError> {
    require_read(&device)?;
    Ok(Json(service.home_page().await?))
}

async fn library(
    State(service): State<Arc<WebService>>,
    Device(device): Device,
    Query(query): Query<PageQuery>,
) -> Result<Json<Vec<LibraryRowDto>>, ApiError> {
    require_read(&device)?;
    let limit = query.limit.unwrap_or(60).clamp(1, 100);
    Ok(Json(service.library_page(query.after, limit).await?))
}

async fn watchlist(
    State(service): State<Arc<WebService>>,
    Device(device): Device,
) -> Result<Json<Vec<TileDto>>, ApiError> {
    require_read(&device)?;
    Ok(Json(service.watchlist_page().await?))
}

async fn downloads(
    State(service): State<Arc<WebService>>,
    Device(device): Device,
) -> Result<Json<Vec<DownloadDto>>, ApiError> {
    require_read(&device)?;
    Ok(Json(service.downloads_page().await?))
}

async fn search(
    State(service): State<Arc<WebService>>,
    Device(device): Device,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Vec<TileDto>>, ApiError> {
    require_read(&device)?;
    let kind = parsed_kind(query.kind.as_deref())?;
    Ok(Json(
        service
            .search_page(query.q.as_deref().unwrap_or_default(), kind)
            .await?,
    ))
}

async fn browse(
    State(service): State<Arc<WebService>>,
    Device(device): Device,
    Query(query): Query<BrowseQuery>,
) -> Result<Json<BrowseDto>, ApiError> {
    require_read(&device)?;
    let kind = parsed_kind(query.kind.as_deref())?.unwrap_or(TitleKind::Movie);
    Ok(Json(
        service
            .browse_page(
                kind,
                query.page.unwrap_or(1).clamp(1, 50),
                parsed_sort(query.sort.as_deref()),
                query.genre,
            )
            .await?,
    ))
}

async fn genres(
    State(service): State<Arc<WebService>>,
    Device(device): Device,
    Query(query): Query<KindQuery>,
) -> Result<Json<Vec<GenreDto>>, ApiError> {
    require_read(&device)?;
    let kind = parsed_kind(query.kind.as_deref())?.unwrap_or(TitleKind::Movie);
    Ok(Json(service.genres_page(kind).await))
}

fn parsed_kind(value: Option<&str>) -> Result<Option<TitleKind>, ApiError> {
    match value {
        None | Some("") => Ok(None),
        Some("movie") => Ok(Some(TitleKind::Movie)),
        Some("show") => Ok(Some(TitleKind::Show)),
        Some(_) => Err(ApiError::bad_request("query")),
    }
}

fn parsed_sort(value: Option<&str>) -> BrowseSort {
    match value {
        Some("rating") => BrowseSort::Rating,
        Some("newest") => BrowseSort::Newest,
        Some("oldest") => BrowseSort::Oldest,
        _ => BrowseSort::Popularity,
    }
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: Option<String>,
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BrowseQuery {
    kind: Option<String>,
    page: Option<u32>,
    sort: Option<String>,
    genre: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct KindQuery {
    kind: Option<String>,
}
async fn title(
    State(service): State<Arc<WebService>>,
    Device(device): Device,
    UrlPath(id): UrlPath<i64>,
    Query(query): Query<TitleQuery>,
) -> Result<Json<TitleDto>, ApiError> {
    require_read(&device)?;
    service
        .title_page(id, query.season)
        .await?
        .map(Json)
        .ok_or_else(ApiError::not_found)
}

#[derive(Debug, Deserialize)]
struct TitleQuery {
    season: Option<u32>,
}

async fn stop_playback(
    State(service): State<Arc<WebService>>,
    Device(device): Device,
    Json(body): Json<StopBody>,
) -> Result<StatusCode, ApiError> {
    service.stop_playback(&device, &body.url).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn playback(
    State(service): State<Arc<WebService>>,
    Device(device): Device,
    UrlPath(id): UrlPath<i64>,
    Json(body): Json<PlaybackBody>,
) -> Result<Json<PlaybackTicket>, ApiError> {
    Ok(Json(
        service
            .mint_playback(
                &device,
                id,
                body.season_number,
                body.episode_number,
                body.episode_id,
                body.position_ms.unwrap_or(0),
            )
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
struct StopBody {
    url: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlaybackBody {
    position_ms: Option<i64>,
    season_number: Option<u32>,
    episode_number: Option<u32>,
    episode_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProgressBody {
    title_id: i64,
    episode_id: Option<i64>,
    position_ms: i64,
    duration_ms: Option<i64>,
}

async fn progress(
    State(service): State<Arc<WebService>>,
    Device(device): Device,
    Json(body): Json<ProgressBody>,
) -> Result<StatusCode, ApiError> {
    service
        .report_progress(
            &device,
            body.title_id,
            body.episode_id,
            body.position_ms,
            body.duration_ms,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn image(
    State(service): State<Arc<WebService>>,
    Device(device): Device,
    UrlPath(key): UrlPath<String>,
) -> Result<Response, ApiError> {
    require_read(&device)?;
    if !is_cache_key(&key) {
        return Err(ApiError::not_found());
    }
    let path = service.artwork_root().join(&key);
    let Some(bytes) = read_contained(&path, service.artwork_root()).await else {
        return Err(ApiError::not_found());
    };
    let Some(mime) = image_mime(&bytes) else {
        return Err(ApiError::not_found());
    };
    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=86400, immutable"),
    );
    Ok(response)
}

async fn hls(
    State(service): State<Arc<WebService>>,
    UrlPath((token, file)): UrlPath<(String, String)>,
) -> Result<Response, ApiError> {
    if !super::transcode::valid_hls_file(&file) {
        return Err(ApiError::not_found());
    }
    let Some(grant) = service.hls_grant(&token).await else {
        return Err(ApiError::not_found());
    };
    if !service.device_active(grant.device_id).await? {
        return Err(ApiError::unauthorized());
    }
    let path = grant.dir.join(&file);
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| ApiError::not_found())?;
    let (mime, cache) = if std::path::Path::new(&file)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("m3u8"))
    {
        ("application/vnd.apple.mpegurl", "no-store")
    } else {
        ("video/mp4", "private, max-age=3600")
    };
    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    Ok(response)
}

async fn media(
    State(service): State<Arc<WebService>>,
    UrlPath(token): UrlPath<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let grant = service
        .media_grant(&token)
        .ok_or_else(ApiError::not_found)?;
    if !service.device_active(grant.device_id).await? {
        return Err(ApiError::unauthorized());
    }
    serve_range(grant, &headers).await
}

async fn serve_range(grant: MediaGrant, headers: &HeaderMap) -> Result<Response, ApiError> {
    let total = grant.size_bytes;
    if total == 0 {
        return Err(ApiError::not_found());
    }
    let requested = match headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
    {
        None => None,
        Some(value) => parse_range(value, total),
    };
    let (status, start, end) = match requested {
        None => (StatusCode::OK, 0, total - 1),
        Some(Err(())) => return Ok(range_refused(total)),
        Some(Ok((start, end))) => (StatusCode::PARTIAL_CONTENT, start, end),
    };
    let length = end - start + 1;
    let mut file = tokio::fs::File::open(&grant.path)
        .await
        .map_err(|_| ApiError::not_found())?;
    file.seek(SeekFrom::Start(start))
        .await
        .map_err(|_| ApiError::not_found())?;
    let stream = ReaderStream::new(file.take(length));
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;
    let outgoing = response.headers_mut();
    outgoing.insert(header::CONTENT_TYPE, HeaderValue::from_static(grant.mime));
    outgoing.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    outgoing.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(value) = HeaderValue::from_str(&length.to_string()) {
        outgoing.insert(header::CONTENT_LENGTH, value);
    }
    if status == StatusCode::PARTIAL_CONTENT
        && let Ok(value) = HeaderValue::from_str(&format!("bytes {start}-{end}/{total}"))
    {
        outgoing.insert(header::CONTENT_RANGE, value);
    }
    Ok(response)
}

fn range_refused(total: u64) -> Response {
    let mut response = ApiError::range().into_response();
    if let Ok(value) = HeaderValue::from_str(&format!("bytes */{total}")) {
        response.headers_mut().insert(header::CONTENT_RANGE, value);
    }
    response
}

/// Parses one range header.
///
/// `None` means the header was not a single byte range this build understands, and RFC 9110 says
/// to ignore it and answer the whole file. `Some(Err(()))` is a range that is syntactically fine but
/// unsatisfiable, which is a `416`.
fn parse_range(value: &str, total: u64) -> Option<Result<(u64, u64), ()>> {
    let spec = value.strip_prefix("bytes=")?;
    let first = spec.split(',').next()?.trim();
    let (start, end) = first.split_once('-')?;
    let start = start.trim();
    let end = end.trim();
    if start.is_empty() {
        let suffix: u64 = end.parse().ok()?;
        if suffix == 0 {
            return Some(Err(()));
        }
        let start = total.saturating_sub(suffix);
        return Some(Ok((start, total - 1)));
    }
    let start: u64 = start.parse().ok()?;
    if start >= total {
        return Some(Err(()));
    }
    let end = if end.is_empty() {
        total - 1
    } else {
        end.parse::<u64>().ok()?.min(total - 1)
    };
    if start > end {
        return Some(Err(()));
    }
    Some(Ok((start, end)))
}

/// A read needs the `read` scope; a stream-only token may not walk the catalogue.
fn require_read(device: &super::PairedDevice) -> Result<(), ApiError> {
    if super::scope_allows(&device.scopes, "read") {
        Ok(())
    } else {
        Err(super::WebError::Unauthorized.into())
    }
}

fn is_cache_key(key: &str) -> bool {
    key.len() == 64
        && key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

async fn read_contained(path: &Path, root: &Path) -> Option<Vec<u8>> {
    let canonical = tokio::fs::canonicalize(path).await.ok()?;
    let root = tokio::fs::canonicalize(root).await.ok()?;
    if !canonical.starts_with(&root) {
        return None;
    }
    let metadata = tokio::fs::metadata(&canonical).await.ok()?;
    if !metadata.is_file() || metadata.len() > 16 * 1024 * 1024 {
        return None;
    }
    tokio::fs::read(&canonical).await.ok()
}

fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        return Some("image/png");
    }
    if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

fn clean(text: &str, max: usize) -> String {
    text.chars()
        .filter(|character| !character.is_control())
        .take(max)
        .collect::<String>()
        .trim()
        .to_owned()
}

async fn no_client() -> Response {
    (
        StatusCode::NOT_FOUND,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "The Peregrine web client is not installed on this machine.",
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::{is_cache_key, parse_range, presented_token};
    use axum::http::{HeaderMap, HeaderValue, header};

    #[test]
    fn a_range_is_parsed_in_the_three_shapes_a_browser_sends() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some(Ok((0, 99))));
        assert_eq!(parse_range("bytes=900-", 1000), Some(Ok((900, 999))));
        assert_eq!(parse_range("bytes=-100", 1000), Some(Ok((900, 999))));
        assert_eq!(parse_range("bytes=0-5000", 1000), Some(Ok((0, 999))));
    }

    #[test]
    fn a_range_outside_the_file_is_unsatisfiable_and_junk_is_ignored() {
        assert_eq!(parse_range("bytes=1000-", 1000), Some(Err(())));
        assert_eq!(parse_range("bytes=5-1", 1000), Some(Err(())));
        assert_eq!(parse_range("items=0-1", 1000), None);
        assert_eq!(parse_range("bytes=one-two", 1000), None);
    }

    #[test]
    fn an_image_key_is_exactly_a_lowercase_sha256() {
        assert!(is_cache_key(&"a".repeat(64)));
        assert!(!is_cache_key(&"A".repeat(64)));
        assert!(!is_cache_key(&"a".repeat(63)));
        assert!(!is_cache_key(&format!("{}_x", "a".repeat(64))));
    }

    #[test]
    fn a_token_arrives_as_a_bearer_or_a_cookie() {
        let mut bearer = HeaderMap::new();
        bearer.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer abc"),
        );
        assert_eq!(presented_token(&bearer).as_deref(), Some("abc"));

        let mut cookie = HeaderMap::new();
        cookie.insert(
            header::COOKIE,
            HeaderValue::from_static("theme=dark; peregrine_token=def; other=1"),
        );
        assert_eq!(presented_token(&cookie).as_deref(), Some("def"));
    }
}
