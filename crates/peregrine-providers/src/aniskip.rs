//! The `AniSkip` adapter: anime skip windows keyed by a `MyAnimeList` id.
//!
//! `AniSkip` answers for one episode of one anime, and the `MAL` id is the whole identity. A
//! title with `TMDB`, `TVDB`, or `IMDb` ids but no `MAL` id is not asked about here, and neither
//! is a film; both absences are normal answers rather than failures, as is the service's 404,
//! which is how it says it has no timestamps for the episode.
//!
//! **No provider type escapes this module.** The JSON below is private and reads only the fields
//! this adapter maps; what leaves is a `Vec<PlaybackMarker>` in the domain's own vocabulary.
//!
//! HTTP is `reqwest` directly rather than the crate's `HttpClient`. That client is attributed to a
//! `Provider`, and `Provider` has no `AniSkip` variant, so reusing it would mean adding a second
//! provider enum to the crate for one adapter (AGENTS R3). The policy is the one the shared client
//! states: an identifying `User-Agent`, a bounded timeout, no redirects, a body cap, and a retry
//! only for the two statuses that mean "not now".

use std::time::Duration;

use peregrine_domain::{MarkerConfidence, MarkerKind, MarkerSource, PlaybackMarker};
use reqwest::header::RETRY_AFTER;
use reqwest::{Client, StatusCode, Url};
use serde::Deserialize;
use tokio::time::sleep;

use crate::error::TransportFailure;
use crate::marker::{MarkerError, MarkerFuture, MarkerProvider, MarkerRequest};

/// The source every failure is attributed to.
const SOURCE: MarkerSource = MarkerSource::AniSkip;

/// Identifies the application to a provider that refuses requests without a name (AGENTS R24).
const USER_AGENT: &str = "Peregrine/0.1 (+https://github.com/peregrine-media/peregrine)";

/// The whole request's budget, body included.
const TIMEOUT: Duration = Duration::from_secs(15);

/// The largest answer this adapter will read.
///
/// A skip-times document is a handful of intervals, so a megabyte is generous for a legitimate
/// answer and small enough that a wrong or hostile one is refused before it is parsed.
const BODY_CAP: u64 = 1_048_576;

/// The longest a provider's own Retry-After may hold a request back.
///
/// The header is untrusted input like any other: a wrong or hostile value would strand the request
/// for an hour, and the adapter's own curve already bounds a legitimate wait.
const MAXIMUM_RETRY_AFTER: Duration = Duration::from_secs(5);

/// How long to wait before the second attempt, before doubling.
const RETRY_BASE: Duration = Duration::from_millis(250);

/// How many times a failed attempt is repeated. Two retries, so at most three requests.
const MAX_RETRIES: u32 = 2;

/// The adapter, built by the integration registry.
#[derive(Debug, Clone)]
pub struct AniSkip {
    client: Client,
    base: String,
}

impl AniSkip {
    /// The production base URL.
    pub const BASE_URL: &'static str = "https://api.aniskip.com";

    /// Builds an adapter against `base`: the live service, or a test server.
    ///
    /// `base` is a parameter rather than a constant so the adapter can be pointed at a `wiremock`
    /// server, which is the only way its behaviour can be tested at all (AGENTS R14).
    ///
    /// # Errors
    ///
    /// [`MarkerError::Transport`] with [`TransportFailure::Connect`] when the HTTP client cannot
    /// be constructed, which is a failure of the policy rather than of anything `AniSkip` said.
    pub fn new(base: &str) -> Result<Self, MarkerError> {
        let client = Client::builder()
            .timeout(TIMEOUT)
            // A redirect to another host would carry the request somewhere the caller did not ask
            // about, and a provider that moved answers with a status rather than a redirect.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(USER_AGENT)
            .build()
            .map_err(|_| transport(TransportFailure::Connect))?;

        Ok(Self {
            client,
            base: base.trim_end_matches('/').to_owned(),
        })
    }

    /// The URL one episode's skip times are read from.
    ///
    /// `episodeLength` is sent only when the player knows the runtime: `AniSkip` uses it to pick
    /// the right release version, and a guessed number would ask about a different cut. The types are
    /// repeated because that is how the documented endpoint takes its filter list.
    fn endpoint(
        &self,
        mal_id: u64,
        episode: u32,
        duration_ms: Option<i64>,
    ) -> Result<Url, MarkerError> {
        let mut url = Url::parse(&format!("{}/v2/skip-times/{mal_id}/{episode}", self.base))
            .map_err(|_| transport(TransportFailure::Connect))?;
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("types", "op")
                .append_pair("types", "ed")
                .append_pair("types", "recap");
            if let Some(seconds) = duration_ms.filter(|ms| *ms > 0).map(|ms| ms / 1000) {
                query.append_pair("episodeLength", &seconds.to_string());
            }
        }
        Ok(url)
    }

    /// Fetches one URL's body, retrying the statuses that mean the provider is busy.
    ///
    /// Ok(None) is the service's 404, which is a statement that the episode has no markers rather
    /// than a failure.
    async fn fetch_body(&self, url: &str) -> Result<Option<Vec<u8>>, MarkerError> {
        let mut retries = 0;
        loop {
            match self.attempt(url).await {
                Ok(body) => return Ok(body),
                Err(failure) => {
                    if !failure.retryable || retries >= MAX_RETRIES {
                        return Err(failure.error);
                    }
                    retries += 1;
                    sleep(retry_delay(retries, failure.retry_after)).await;
                }
            }
        }
    }

    /// One request, with everything the retry loop needs to decide what to do next.
    async fn attempt(&self, url: &str) -> Result<Option<Vec<u8>>, AttemptFailure> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|error| AttemptFailure {
                // Neither a timeout nor a refused connection says anything about the request itself;
                // the caller's retryability flag still lets a later call try again.
                error: transport(if error.is_timeout() {
                    TransportFailure::Timeout
                } else {
                    TransportFailure::Connect
                }),
                retry_after: None,
                retryable: false,
            })?;

        let status = response.status();
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if status.is_success() {
            return read_capped(response).await.map(Some);
        }

        if status == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = retry_after(&response);
            return Err(AttemptFailure {
                error: MarkerError::RateLimited {
                    marker_source: SOURCE,
                    retry_after,
                },
                retry_after,
                retryable: true,
            });
        }
        if status.is_server_error() {
            return Err(AttemptFailure {
                error: transport(TransportFailure::Interrupted),
                retry_after: None,
                retryable: true,
            });
        }

        // A 401 or 403 is a final answer about the credential, and every other status is a
        // response this build cannot read as data. Neither is asked for again.
        let error = match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => MarkerError::Unauthorized {
                marker_source: SOURCE,
            },
            _ => transport(TransportFailure::Decode),
        };
        Err(AttemptFailure {
            error,
            retry_after: None,
            retryable: false,
        })
    }
}

impl MarkerProvider for AniSkip {
    fn source(&self) -> MarkerSource {
        MarkerSource::AniSkip
    }

    fn requires_credential(&self) -> bool {
        false
    }

    fn fetch(&self, request: MarkerRequest) -> MarkerFuture<'_, Vec<PlaybackMarker>> {
        // AniSkip keys on MAL alone. A title without one is not guessed at from another id, and a
        // title that is not anime is never its subject.
        if !request.is_anime {
            return Box::pin(async { Ok(Vec::new()) });
        }
        let Some(mal_id) = request.mal_id else {
            return Box::pin(async { Ok(Vec::new()) });
        };
        let Some(episode) = request.episode else {
            return Box::pin(async { Ok(Vec::new()) });
        };
        let url = match self.endpoint(mal_id, episode, request.duration_ms) {
            Ok(url) => url,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let duration_ms = request.duration_ms;

        Box::pin(async move {
            let Some(body) = self.fetch_body(url.as_str()).await? else {
                return Ok(Vec::new());
            };
            Ok(decode(&body)?.into_markers(duration_ms))
        })
    }
}

/// One failed attempt, with the wait the provider asked for and whether another try is worthwhile.
struct AttemptFailure {
    error: MarkerError,
    retry_after: Option<Duration>,
    retryable: bool,
}

/// How long to wait before retry `retry` (1-based).
///
/// The provider's own `Retry-After` wins when it named one, capped at [`MAXIMUM_RETRY_AFTER`];
/// otherwise the delay doubles from [`RETRY_BASE`]. Both arms are bounded so a hostile header or
/// an unexpected retry count cannot become a hot loop against a provider that is already refusing.
fn retry_delay(retry: u32, retry_after: Option<Duration>) -> Duration {
    retry_after.map_or_else(
        || {
            RETRY_BASE
                .saturating_mul(2u32.saturating_pow(retry.saturating_sub(1)))
                .min(MAXIMUM_RETRY_AFTER)
        },
        |asked| asked.min(MAXIMUM_RETRY_AFTER),
    )
}

/// The delay a response's Retry-After names, when it names a whole number of seconds.
///
/// An HTTP-date is deliberately not parsed: this adapter only needs the form a rate limiter sends,
/// and a date here would be trusted arithmetic over an untrusted clock.
fn retry_after(response: &reqwest::Response) -> Option<Duration> {
    response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

/// Reads a successful body, refusing it once it passes `BODY_CAP`.
///
/// The cap is enforced while reading rather than after, so an answer over it is never held in
/// memory in full.
async fn read_capped(mut response: reqwest::Response) -> Result<Vec<u8>, AttemptFailure> {
    let mut body = Vec::new();
    loop {
        let chunk = response.chunk().await.map_err(|_| AttemptFailure {
            error: transport(TransportFailure::Interrupted),
            retry_after: None,
            retryable: false,
        })?;
        let Some(chunk) = chunk else {
            return Ok(body);
        };
        let received = body.len() as u64 + chunk.len() as u64;
        if received > BODY_CAP {
            return Err(AttemptFailure {
                error: MarkerError::TooLarge {
                    marker_source: SOURCE,
                    bytes: received,
                    cap: BODY_CAP,
                },
                retry_after: None,
                retryable: false,
            });
        }
        body.extend_from_slice(&chunk);
    }
}

/// Reads a success body as `AniSkip`'s skip-times answer.
///
/// A body that is not JSON never reached the provider's documented shape, so it is a transport
/// decode failure; JSON that is the wrong shape is the provider changing its answer, which is
/// schema drift.
fn decode(body: &[u8]) -> Result<SkipTimesAnswer, MarkerError> {
    serde_json::from_slice(body).map_err(|error| match error.classify() {
        serde_json::error::Category::Data => MarkerError::SchemaDrift {
            marker_source: SOURCE,
        },
        serde_json::error::Category::Io
        | serde_json::error::Category::Syntax
        | serde_json::error::Category::Eof => transport(TransportFailure::Decode),
    })
}

/// A transport failure attributed to this adapter.
fn transport(kind: TransportFailure) -> MarkerError {
    MarkerError::Transport {
        marker_source: SOURCE,
        kind,
    }
}

/// The subset of the documented skip-times answer this adapter reads.
///
/// found and results are required: every documented answer carries both, and defaulting a missing
/// key would turn a renamed field into "this episode has no markers".
#[derive(Debug, Deserialize)]
struct SkipTimesAnswer {
    found: bool,
    results: Vec<SkipTime>,
}

impl SkipTimesAnswer {
    /// This answer as markers, bounded by the runtime when the player knows it.
    fn into_markers(self, duration_ms: Option<i64>) -> Vec<PlaybackMarker> {
        if !self.found {
            return Vec::new();
        }
        self.results
            .into_iter()
            .filter_map(|result| result.into_marker(duration_ms))
            .collect()
    }
}

/// One documented skip interval.
#[derive(Debug, Deserialize)]
struct SkipTime {
    interval: Interval,
    #[serde(rename = "skipType")]
    skip_type: SkipType,
}

impl SkipTime {
    /// This interval as one marker, or None when it cannot be shown.
    fn into_marker(self, duration_ms: Option<i64>) -> Option<PlaybackMarker> {
        let kind = self.skip_type.kind()?;
        let start = millis(self.interval.start_time)?;
        let end = millis(self.interval.end_time)?;
        // A submission that closes at or before it opens is a bad interval, not a provider change,
        // and every kind this adapter maps is closed-ended: there is nothing to skip.
        if end <= start {
            return None;
        }
        let marker = PlaybackMarker::new(
            kind,
            start,
            Some(end),
            MarkerSource::AniSkip,
            // AniSkip publishes no score. Its community has voted the interval into the dataset,
            // which is what the domain's community confidence is for.
            MarkerConfidence::COMMUNITY,
        )
        .ok()?;
        match duration_ms {
            Some(duration) => marker.bounded(duration),
            None => Some(marker),
        }
    }
}

/// The documented interval, in seconds.
#[derive(Debug, Deserialize)]
struct Interval {
    #[serde(rename = "startTime")]
    start_time: f64,
    #[serde(rename = "endTime")]
    end_time: f64,
}

/// The documented segment types, with anything else ignored rather than refused.
#[derive(Debug, Clone, Copy, Deserialize)]
enum SkipType {
    #[serde(rename = "op")]
    Op,
    #[serde(rename = "ed")]
    Ed,
    #[serde(rename = "mixed-op")]
    MixedOp,
    #[serde(rename = "mixed-ed")]
    MixedEd,
    #[serde(rename = "recap")]
    Recap,
    /// A type this build has no window for; a provider adding one is not an outage.
    #[serde(other)]
    Unrecognised,
}

impl SkipType {
    /// The kind this segment type describes, or None when this build ignores it.
    const fn kind(self) -> Option<MarkerKind> {
        match self {
            Self::Op | Self::MixedOp => Some(MarkerKind::Intro),
            Self::Ed | Self::MixedEd => Some(MarkerKind::Credits),
            Self::Recap => Some(MarkerKind::Recap),
            Self::Unrecognised => None,
        }
    }
}

/// Seconds rounded to the nearest whole millisecond, or None when it is not a usable instant.
fn millis(seconds: f64) -> Option<i64> {
    // Beyond 2^53 - 1 an `f64` no longer represents every integer, so a larger value is not the
    // millisecond count the provider sent and is refused rather than truncated into a smaller one.
    const MAX_EXACT_MILLIS: f64 = 9_007_199_254_740_991.0;
    if !seconds.is_finite() {
        return None;
    }
    let millis = (seconds * 1000.0).round();
    if !(0.0..=MAX_EXACT_MILLIS).contains(&millis) {
        return None;
    }
    Some(millis as i64)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use peregrine_domain::TitleKind;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const OP_ED_ANSWER: &str = r#"{
        "found": true,
        "results": [
            {"interval": {"startTime": 12.5, "endTime": 92.25}, "skipType": "op", "skipId": "1", "episodeLength": 1440},
            {"interval": {"startTime": 1300.5, "endTime": 1399.75}, "skipType": "ed", "skipId": "2", "episodeLength": 1440}
        ],
        "message": "",
        "statusCode": 200
    }"#;

    /// A request for one anime episode, with the runtime the player would know.
    fn request(mal_id: Option<u64>, episode: Option<u32>, is_anime: bool) -> MarkerRequest {
        MarkerRequest {
            kind: TitleKind::Show,
            season: Some(1),
            episode,
            tvdb_id: None,
            tmdb_id: None,
            imdb_id: None,
            mal_id,
            duration_ms: Some(1_440_000),
            is_anime,
        }
    }

    fn adapter(server: &MockServer) -> AniSkip {
        AniSkip::new(&server.uri()).expect("an adapter builds")
    }

    fn marker(kind: MarkerKind, start: i64, end: i64) -> PlaybackMarker {
        PlaybackMarker::new(
            kind,
            start,
            Some(end),
            MarkerSource::AniSkip,
            MarkerConfidence::COMMUNITY,
        )
        .expect("the tests pass a valid window")
    }

    #[tokio::test]
    async fn an_anime_episode_maps_its_opening_and_ending_to_intro_and_credits() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/skip-times/21/1"))
            .and(query_param("types", "op"))
            .and(query_param("types", "ed"))
            .and(query_param("types", "recap"))
            .and(query_param("episodeLength", "1440"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(OP_ED_ANSWER, "application/json"))
            .expect(1)
            .mount(&server)
            .await;

        let markers = adapter(&server)
            .fetch(request(Some(21), Some(1), true))
            .await
            .expect("the answer is read");

        assert_eq!(
            markers,
            vec![
                marker(MarkerKind::Intro, 12_500, 92_250),
                marker(MarkerKind::Credits, 1_300_500, 1_399_750),
            ]
        );
    }

    #[tokio::test]
    async fn mixed_openings_and_endings_map_to_the_same_kinds_as_op_and_ed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/skip-times/21/1"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "found": true,
                    "results": [
                        {"interval": {"startTime": 10, "endTime": 80}, "skipType": "mixed-op", "skipId": "1", "episodeLength": 1440},
                        {"interval": {"startTime": 1300, "endTime": 1390}, "skipType": "mixed-ed", "skipId": "2", "episodeLength": 1440}
                    ],
                    "message": "",
                    "statusCode": 200
                }"#,
                "application/json",
            ))
            .expect(1)
            .mount(&server)
            .await;

        let markers = adapter(&server)
            .fetch(request(Some(21), Some(1), true))
            .await
            .expect("the answer is read");

        assert_eq!(
            markers,
            vec![
                marker(MarkerKind::Intro, 10_000, 80_000),
                marker(MarkerKind::Credits, 1_300_000, 1_390_000),
            ]
        );
    }

    #[tokio::test]
    async fn a_request_without_a_mal_id_is_never_sent() {
        let server = MockServer::start().await;

        let markers = adapter(&server)
            .fetch(request(None, Some(1), true))
            .await
            .expect("nothing to ask is not a failure");

        assert_eq!(markers, Vec::new());
        assert!(
            server
                .received_requests()
                .await
                .expect("the server records requests")
                .is_empty(),
            "a title with no MAL id must not be guessed at from another identity"
        );
    }

    #[tokio::test]
    async fn a_title_that_is_not_anime_is_never_sent() {
        let server = MockServer::start().await;

        let markers = adapter(&server)
            .fetch(request(Some(21), Some(1), false))
            .await
            .expect("nothing to ask is not a failure");

        assert_eq!(markers, Vec::new());
        assert!(
            server
                .received_requests()
                .await
                .expect("the server records requests")
                .is_empty(),
            "AniSkip is never asked about a title that is not anime"
        );
    }

    #[tokio::test]
    async fn an_episode_without_a_number_is_never_sent() {
        let server = MockServer::start().await;

        let markers = adapter(&server)
            .fetch(request(Some(21), None, true))
            .await
            .expect("nothing to ask is not a failure");

        assert_eq!(markers, Vec::new());
        assert!(
            server
                .received_requests()
                .await
                .expect("the server records requests")
                .is_empty(),
            "there is no endpoint for a whole series' skip times"
        );
    }

    #[tokio::test]
    async fn a_not_found_answer_is_no_markers_rather_than_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/skip-times/21/1"))
            .respond_with(ResponseTemplate::new(404).set_body_raw(
                r#"{"found": false, "results": [], "message": "No skip times found", "statusCode": 404}"#,
                "application/json",
            ))
            .expect(1)
            .mount(&server)
            .await;

        let markers = adapter(&server)
            .fetch(request(Some(21), Some(1), true))
            .await
            .expect("a 404 is an answer");

        assert_eq!(markers, Vec::new());
    }

    #[tokio::test]
    async fn a_server_error_is_retried_twice_and_then_reported_as_transport() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/skip-times/21/1"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let error = adapter(&server)
            .fetch(request(Some(21), Some(1), true))
            .await
            .expect_err("a provider that keeps failing is reported");

        assert_eq!(
            error,
            MarkerError::Transport {
                marker_source: MarkerSource::AniSkip,
                kind: TransportFailure::Interrupted,
            }
        );
        assert!(error.retryable(), "the caller may try again later");
        assert_eq!(
            server
                .received_requests()
                .await
                .expect("the server records requests")
                .len(),
            3,
            "the first attempt and exactly two retries"
        );
    }

    #[tokio::test]
    async fn a_rate_limited_answer_is_asked_again() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/skip-times/21/1"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v2/skip-times/21/1"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(OP_ED_ANSWER, "application/json"))
            .expect(1)
            .mount(&server)
            .await;

        let markers = adapter(&server)
            .fetch(request(Some(21), Some(1), true))
            .await
            .expect("the second attempt answers");

        assert_eq!(markers.len(), 2);
    }

    #[test]
    fn the_retry_wait_is_the_providers_own_number_capped_at_five_seconds() {
        assert_eq!(retry_delay(1, None), RETRY_BASE);
        assert_eq!(retry_delay(2, None), RETRY_BASE * 2);
        assert_eq!(
            retry_delay(1, Some(Duration::from_millis(100))),
            Duration::from_millis(100),
            "a provider may ask for less than the curve"
        );
        assert_eq!(
            retry_delay(1, Some(Duration::from_secs(600))),
            MAXIMUM_RETRY_AFTER,
            "a header is untrusted input, so a wrong value is a delay and not a hang"
        );
    }

    #[tokio::test]
    async fn an_answer_that_is_not_json_is_a_decode_failure() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/skip-times/21/1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw("<html>maintenance</html>", "application/json"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let error = adapter(&server)
            .fetch(request(Some(21), Some(1), true))
            .await
            .expect_err("html is not the documented shape");

        assert_eq!(
            error,
            MarkerError::Transport {
                marker_source: MarkerSource::AniSkip,
                kind: TransportFailure::Decode,
            }
        );
    }

    #[tokio::test]
    async fn json_of_an_unexpected_shape_is_schema_drift() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/skip-times/21/1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(r#"{"found": true, "results": "none"}"#, "application/json"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let error = adapter(&server)
            .fetch(request(Some(21), Some(1), true))
            .await
            .expect_err("the provider changed its answer");

        assert_eq!(
            error,
            MarkerError::SchemaDrift {
                marker_source: MarkerSource::AniSkip,
            }
        );
    }

    #[tokio::test]
    async fn an_answer_over_the_cap_is_refused_with_its_size() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/skip-times/21/1"))
            .respond_with(
                ResponseTemplate::new(200).set_body_bytes(vec![b'x'; (BODY_CAP + 1) as usize]),
            )
            .expect(1)
            .mount(&server)
            .await;

        let error = adapter(&server)
            .fetch(request(Some(21), Some(1), true))
            .await
            .expect_err("an answer over the cap is refused");

        match error {
            MarkerError::TooLarge {
                marker_source,
                bytes,
                cap,
            } => {
                assert_eq!(marker_source, MarkerSource::AniSkip);
                assert_eq!(cap, BODY_CAP);
                assert!(bytes > BODY_CAP, "{bytes} is not over {BODY_CAP}");
            }
            other => panic!("expected a cap refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_interval_that_ends_before_it_starts_is_dropped() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/skip-times/21/1"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{
                    "found": true,
                    "results": [
                        {"interval": {"startTime": 100, "endTime": 90}, "skipType": "op", "skipId": "1", "episodeLength": 1440},
                        {"interval": {"startTime": 50, "endTime": 50}, "skipType": "ed", "skipId": "2", "episodeLength": 1440},
                        {"interval": {"startTime": 1300, "endTime": 1390}, "skipType": "unknown-segment", "skipId": "3", "episodeLength": 1440},
                        {"interval": {"startTime": 0, "endTime": 30}, "skipType": "recap", "skipId": "4", "episodeLength": 1440}
                    ],
                    "message": "",
                    "statusCode": 200
                }"#,
                "application/json",
            ))
            .expect(1)
            .mount(&server)
            .await;

        let markers = adapter(&server)
            .fetch(request(Some(21), Some(1), true))
            .await
            .expect("the answer is read");

        assert_eq!(markers, vec![marker(MarkerKind::Recap, 0, 30_000)]);
    }

    #[tokio::test]
    async fn a_request_identifies_the_application() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/skip-times/21/1"))
            .and(header("user-agent", USER_AGENT))
            .respond_with(ResponseTemplate::new(200).set_body_raw(OP_ED_ANSWER, "application/json"))
            .expect(1)
            .mount(&server)
            .await;

        let markers = adapter(&server)
            .fetch(request(Some(21), Some(1), true))
            .await
            .expect("the request carries the agent");

        assert_eq!(markers.len(), 2);
    }
}
