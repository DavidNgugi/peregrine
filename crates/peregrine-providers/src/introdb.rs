//! The `TheIntroDB` v4 adapter: community timestamps, translated into playback markers.
//!
//! **Nothing `TheIntroDB`-shaped leaves this module.** The JSON types below are private and read a
//! subset of the documented answer; every field that may be absent is optional here rather than
//! assumed, because a provider's answer is untrusted input like any other.
//!
//! **The key is for submissions, not for reads.** Reads work anonymously, so the credential is
//! optional and its presence changes one header and nothing else. It is never put in the URL: this
//! adapter's failures carry no URL at all, for the reason the marker port records (SECURITY.md T13).
//!
//! **The fixtures these tests are written against are synthetic.** The shapes follow the v4
//! documentation; no request has been made to the live service from this repository, because
//! AGENTS R14 forbids it and a test that needed the network would be a test nobody could run.

use std::time::Duration;

use peregrine_domain::{MarkerConfidence, MarkerKind, MarkerSource, PlaybackMarker};
use reqwest::header::RETRY_AFTER;
use reqwest::{Client, Response, StatusCode, Url};
use serde::Deserialize;
use tokio::time::sleep;

use crate::error::TransportFailure;
use crate::marker::{MarkerError, MarkerFuture, MarkerProvider, MarkerRequest};

/// The source every marker and every failure from this adapter names.
const SOURCE: MarkerSource = MarkerSource::IntroDb;

/// The agent that identifies this application (AGENTS R24).
const USER_AGENT: &str = "Peregrine/0.1 (+https://github.com/peregrine-media/peregrine)";

/// The largest answer this adapter reads from one lookup.
///
/// The default shape is four short arrays, so a megabyte is generous for a legitimate answer and
/// small enough that a wrong or hostile one is refused before it is parsed.
const BODY_CAP: u64 = 1_048_576;

/// How long one lookup may take, its body included.
const TIMEOUT: Duration = Duration::from_secs(15);

/// How many times one lookup is attempted in total, the first attempt included.
const ATTEMPTS: u32 = 3;

/// The longest a provider's `Retry-After` may hold a lookup back.
///
/// The header is untrusted input: it is honoured, but capped, so a wrong or hostile value leaves a
/// skip control missing for a moment rather than stranding the lookup for hours.
const RETRY_AFTER_CAP: Duration = Duration::from_secs(5);

/// The wait before a retry the provider named no delay for.
///
/// Short because a marker lookup sits under a person pressing play: a pause they can perceive is
/// worth less than the chance that the next attempt answers.
const RETRY_DELAY: Duration = Duration::from_millis(250);

/// The `TheIntroDB` adapter, built by the integration registry.
///
/// Holds the client, the base URL, and the optional submission key. The key is a parameter rather
/// than something this type goes looking for: an adapter that fetched its own secret would be an
/// adapter that could not be tested without one.
#[derive(Debug, Clone)]
pub struct IntroDb {
    client: Client,
    base: String,
    key: Option<String>,
}

impl IntroDb {
    /// The production base URL.
    pub const BASE_URL: &'static str = "https://api.theintrodb.org/v4";

    /// Builds an adapter against `base`, with an optional submission key.
    ///
    /// `base` is a parameter rather than a constant so the adapter can be pointed at a `wiremock`
    /// server, which is the only way its behaviour can be tested at all (AGENTS R14).
    ///
    /// # Errors
    ///
    /// [`MarkerError::Transport`] when the client cannot be constructed, which is a failure of the
    /// policy rather than of anything a provider said.
    pub fn new(base: &str, key: Option<String>) -> Result<Self, MarkerError> {
        let client = Client::builder()
            .timeout(TIMEOUT)
            // A redirect would carry the request to a host the caller did not ask about, and a
            // provider that moved answers with a status rather than a redirect.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(USER_AGENT)
            .build()
            .map_err(|_| MarkerError::Transport {
                marker_source: SOURCE,
                kind: TransportFailure::Connect,
            })?;
        Ok(Self {
            client,
            base: base.trim_end_matches('/').to_owned(),
            key,
        })
    }

    /// The `/media` URL for one request, or `None` when the request carries no identity this
    /// adapter can ask about.
    ///
    /// TVDB first, TMDB second, `IMDb` last: that is the provider's own order of preference, and
    /// an `IMDb` id is the slowest of the three for it to resolve. `details` is always sent
    /// because it is what asks for the confidence a marker is scored with.
    fn media_url(&self, request: &MarkerRequest) -> Result<Option<Url>, MarkerError> {
        let identity = if let Some(id) = request.tvdb_id {
            ("tvdb_id", id.to_string())
        } else if let Some(id) = request.tmdb_id {
            ("tmdb_id", id.to_string())
        } else if let Some(id) = request.imdb_id.as_deref() {
            ("imdb_id", id.to_owned())
        } else {
            return Ok(None);
        };

        let mut url =
            Url::parse(&format!("{}/media", self.base)).map_err(|_| MarkerError::Transport {
                marker_source: SOURCE,
                kind: TransportFailure::Connect,
            })?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair(identity.0, &identity.1);
            if let (Some(season), Some(episode)) = (request.season, request.episode) {
                query.append_pair("season", &season.to_string());
                query.append_pair("episode", &episode.to_string());
            }
            if let Some(duration) = request.duration_ms.filter(|duration| *duration > 0) {
                query.append_pair("duration_ms", &duration.to_string());
            }
            query.append_pair("details", "true");
        }
        Ok(Some(url))
    }

    /// Asks for `url`, retrying what the provider's answer says is worth another attempt.
    async fn get(&self, url: &Url) -> Result<Vec<u8>, MarkerError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            match self.attempt(url).await {
                Ok(body) => return Ok(body),
                Err(error) => {
                    let Some(delay) = retry_delay(attempt, &error) else {
                        return Err(error);
                    };
                    sleep(delay).await;
                }
            }
        }
    }

    /// One attempt: one status, one capped read of the body.
    async fn attempt(&self, url: &Url) -> Result<Vec<u8>, MarkerError> {
        let mut builder = self.client.get(url.clone());
        if let Some(key) = &self.key {
            // A bearer header rather than a query parameter: a key in a URL reaches every log and
            // every error, and this module's errors deliberately carry no URL at all.
            builder = builder.bearer_auth(key);
        }

        let response = builder
            .send()
            .await
            .map_err(|error| MarkerError::Transport {
                marker_source: SOURCE,
                kind: if error.is_timeout() {
                    TransportFailure::Timeout
                } else {
                    TransportFailure::Connect
                },
            })?;

        let status = response.status();
        if !status.is_success() {
            return Err(status_error(status, retry_after(&response)));
        }
        read_capped(response).await
    }
}

impl MarkerProvider for IntroDb {
    fn source(&self) -> MarkerSource {
        MarkerSource::IntroDb
    }

    fn requires_credential(&self) -> bool {
        false
    }

    fn fetch(&self, request: MarkerRequest) -> MarkerFuture<'_, Vec<PlaybackMarker>> {
        Box::pin(async move {
            // No identity means no question: a lookup with nothing to match on could only answer
            // with another title's markers, and the port's empty answer is the true one.
            let Some(url) = self.media_url(&request)? else {
                return Ok(Vec::new());
            };
            let body = self.get(&url).await?;
            Ok(markers(decode(&body)?, request.duration_ms))
        })
    }
}

/// The error one status is, in the port's vocabulary.
///
/// A server error is reported as an interrupted exchange because that is what it is to the caller:
/// the answer never arrived. A `4xx` this adapter has no name for is a decode failure of the same
/// kind, because the bytes that arrived were not the answer the request asked for.
fn status_error(status: StatusCode, retry_after: Option<Duration>) -> MarkerError {
    match status {
        StatusCode::TOO_MANY_REQUESTS => MarkerError::RateLimited {
            marker_source: SOURCE,
            retry_after,
        },
        StatusCode::NOT_FOUND => MarkerError::NotFound {
            marker_source: SOURCE,
        },
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => MarkerError::Unauthorized {
            marker_source: SOURCE,
        },
        _ => MarkerError::Transport {
            marker_source: SOURCE,
            kind: if status.is_server_error() {
                TransportFailure::Interrupted
            } else {
                TransportFailure::Decode
            },
        },
    }
}

/// The delay one failed attempt asked for, or `None` when the failure is final.
///
/// `attempt` is the attempt that just failed, 1-based. A provider's own `Retry-After` wins over
/// this adapter's pause when it named one, within [`RETRY_AFTER_CAP`].
fn retry_delay(attempt: u32, error: &MarkerError) -> Option<Duration> {
    if attempt >= ATTEMPTS {
        return None;
    }
    match error {
        MarkerError::RateLimited { retry_after, .. } => {
            Some(retry_after.unwrap_or(RETRY_DELAY).min(RETRY_AFTER_CAP))
        }
        // A body that arrived and made no sense will not make sense next time; a timeout, a
        // refused connection, or a `5xx` might.
        MarkerError::Transport { kind, .. } if !matches!(kind, TransportFailure::Decode) => {
            Some(RETRY_DELAY)
        }
        _ => None,
    }
}

/// The provider's own `Retry-After`, when it sent a delay in seconds.
fn retry_after(response: &Response) -> Option<Duration> {
    response
        .headers()
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

/// Reads one answer, refusing it beyond [`BODY_CAP`] rather than after buffering it whole.
///
/// The declared length is checked first, because that refuses an oversized answer before a byte of
/// it is buffered; a chunked answer carries no length, which is why the loop checks as well.
async fn read_capped(mut response: Response) -> Result<Vec<u8>, MarkerError> {
    if let Some(length) = response
        .content_length()
        .filter(|length| *length > BODY_CAP)
    {
        return Err(MarkerError::TooLarge {
            marker_source: SOURCE,
            bytes: length,
            cap: BODY_CAP,
        });
    }

    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| MarkerError::Transport {
        marker_source: SOURCE,
        kind: TransportFailure::Interrupted,
    })? {
        let read = body.len() as u64 + chunk.len() as u64;
        if read > BODY_CAP {
            return Err(MarkerError::TooLarge {
                marker_source: SOURCE,
                bytes: read,
                cap: BODY_CAP,
            });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// The answer, or the error that says which kind of unreadable it was.
///
/// `crate::hygiene::Body` is the crate's usual gate, but it reports through `ProviderError`, which
/// names a domain `Provider` — and `TheIntroDB` has no variant there because a marker source is not a
/// metadata provider. What the gate would check that this adapter still owes is the split between
/// "not JSON at all" and "not this shape"; the cap is enforced while reading.
fn decode(body: &[u8]) -> Result<MediaAnswer, MarkerError> {
    serde_json::from_slice(body).map_err(|error| match error.classify() {
        serde_json::error::Category::Data => MarkerError::SchemaDrift {
            marker_source: SOURCE,
        },
        serde_json::error::Category::Io
        | serde_json::error::Category::Syntax
        | serde_json::error::Category::Eof => MarkerError::Transport {
            marker_source: SOURCE,
            kind: TransportFailure::Decode,
        },
    })
}

/// Every marker in one answer, in the order `TheIntroDB` lists the kinds.
fn markers(answer: MediaAnswer, duration_ms: Option<i64>) -> Vec<PlaybackMarker> {
    let listed = [
        (MarkerKind::Intro, answer.intro),
        (MarkerKind::Recap, answer.recap),
        (MarkerKind::Credits, answer.credits),
        (MarkerKind::Preview, answer.preview),
    ];

    let mut markers = Vec::new();
    for (kind, segments) in listed {
        markers.extend(
            segments
                .into_iter()
                .filter_map(|segment| segment.into_marker(kind, duration_ms)),
        );
    }
    markers
}

/// The default `/media` shape, as much of it as this adapter reads.
///
/// The `versions` and `episodes` shapes carry no markers, so they are deliberately not modelled:
/// they decode to four empty lists, which is the same evidence as a provider with nothing to say.
#[derive(Debug, Deserialize)]
struct MediaAnswer {
    #[serde(default)]
    intro: Vec<Segment>,
    #[serde(default)]
    recap: Vec<Segment>,
    #[serde(default)]
    credits: Vec<Segment>,
    #[serde(default)]
    preview: Vec<Segment>,
}

/// One window, as `TheIntroDB` publishes it.
///
/// A null start is the beginning of the media and a null end is its end: both are the provider's way
/// of saying "the rest of it", not a missing value, so they stay options here until the request's
/// own runtime can resolve them.
#[derive(Debug, Deserialize)]
struct Segment {
    #[serde(default)]
    start_ms: Option<i64>,
    #[serde(default)]
    end_ms: Option<i64>,
    #[serde(default)]
    confidence: Option<f64>,
}

impl Segment {
    /// This window as a marker, or `None` when it cannot be one.
    ///
    /// Only credits may run to the end of the media without an end of their own. Any other kind
    /// gets the file's runtime when it is known and is dropped when it is not, because an invented
    /// end is a skip button that moves the film somewhere the person did not ask for.
    fn into_marker(self, kind: MarkerKind, duration_ms: Option<i64>) -> Option<PlaybackMarker> {
        let start = self.start_ms.unwrap_or(0);
        let end = match (kind, self.end_ms) {
            (_, Some(end)) => Some(end),
            (MarkerKind::Credits, None) => None,
            (_, None) => Some(duration_ms?),
        };

        let marker = PlaybackMarker::new(
            kind,
            start,
            end,
            SOURCE,
            MarkerConfidence::from_provider(self.confidence),
        )
        .ok()?;

        match duration_ms {
            Some(duration) => marker.bounded(duration),
            None => Some(marker),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use peregrine_domain::{MarkerKind, MarkerSource, TitleKind};
    use wiremock::matchers::{header, method, path, query_param, query_param_is_missing};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{ATTEMPTS, BODY_CAP, IntroDb};
    use crate::error::TransportFailure;
    use crate::marker::{MarkerError, MarkerProvider, MarkerRequest};

    fn adapter(base: &str, key: Option<&str>) -> IntroDb {
        IntroDb::new(base, key.map(str::to_owned)).expect("a client builds")
    }

    fn movie(tvdb_id: Option<u64>, duration_ms: Option<i64>) -> MarkerRequest {
        MarkerRequest {
            kind: TitleKind::Movie,
            season: None,
            episode: None,
            tvdb_id,
            tmdb_id: None,
            imdb_id: None,
            mal_id: None,
            duration_ms,
            is_anime: false,
        }
    }

    fn episode(season: u32, number: u32, duration_ms: Option<i64>) -> MarkerRequest {
        MarkerRequest {
            kind: TitleKind::Show,
            season: Some(season),
            episode: Some(number),
            tvdb_id: Some(555),
            tmdb_id: None,
            imdb_id: None,
            mal_id: None,
            duration_ms,
            is_anime: false,
        }
    }

    #[tokio::test]
    async fn a_movie_lookup_sends_its_identity_and_maps_intro_and_credits() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/media"))
            .and(query_param("tvdb_id", "1234"))
            .and(query_param("details", "true"))
            .and(query_param("duration_ms", "7200000"))
            .and(query_param_is_missing("tmdb_id"))
            .and(query_param_is_missing("imdb_id"))
            .and(query_param_is_missing("season"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"tvdb_id":1234,"type":"movie",
                    "intro":[{"start_ms":null,"end_ms":42000,"confidence":0.9}],
                    "credits":[{"start_ms":7000000,"end_ms":null,"confidence":95}]}"#,
            ))
            .expect(1)
            .mount(&server)
            .await;

        // Both alternates are present, so the request also proves the preference order.
        let mut request = movie(Some(1234), Some(7_200_000));
        request.tmdb_id = Some(42);
        request.imdb_id = Some("tt123".to_owned());

        let markers = adapter(&server.uri(), None)
            .fetch(request)
            .await
            .expect("the answer is read");

        assert_eq!(markers.len(), 2);
        assert_eq!(markers[0].kind(), MarkerKind::Intro);
        assert_eq!(markers[0].start_ms(), 0, "a null start is the beginning");
        assert_eq!(markers[0].end_ms(), Some(42_000));
        assert_eq!(markers[0].confidence().percent(), 90);
        assert_eq!(markers[1].kind(), MarkerKind::Credits);
        assert_eq!(markers[1].start_ms(), 7_000_000);
        assert_eq!(markers[1].end_ms(), None, "credits may run to the end");
        assert_eq!(markers[1].confidence().percent(), 95);
    }

    #[tokio::test]
    async fn a_series_lookup_sends_the_episode_and_maps_recap_and_preview() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/media"))
            .and(query_param("tvdb_id", "555"))
            .and(query_param("season", "1"))
            .and(query_param("episode", "7"))
            .and(query_param("duration_ms", "1440000"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"tvdb_id":555,"type":"tv","season":1,"episode":7,
                    "recap":[{"start_ms":0,"end_ms":30000,"confidence":1.0}],
                    "preview":[{"start_ms":1400000,"end_ms":1430000}]}"#,
            ))
            .expect(1)
            .mount(&server)
            .await;

        let markers = adapter(&server.uri(), None)
            .fetch(episode(1, 7, Some(1_440_000)))
            .await
            .expect("the answer is read");

        assert_eq!(markers.len(), 2);
        assert_eq!(markers[0].kind(), MarkerKind::Recap);
        assert_eq!(markers[0].start_ms(), 0);
        assert_eq!(markers[0].end_ms(), Some(30_000));
        assert_eq!(markers[1].kind(), MarkerKind::Preview);
        assert_eq!(markers[1].start_ms(), 1_400_000);
        assert_eq!(markers[1].end_ms(), Some(1_430_000));
        assert_eq!(
            markers[1].confidence().percent(),
            60,
            "a segment with no score is trusted with the port's community default"
        );
    }

    #[tokio::test]
    async fn a_request_with_no_identity_asks_nothing() {
        let server = MockServer::start().await;

        let markers = adapter(&server.uri(), None)
            .fetch(movie(None, Some(1_440_000)))
            .await
            .expect("nothing to ask is not a failure");

        assert!(markers.is_empty());
        assert!(
            server
                .received_requests()
                .await
                .expect("the server records requests")
                .is_empty(),
            "a lookup with no id would answer with another title's markers"
        );
    }

    #[tokio::test]
    async fn the_identity_preference_falls_back_to_tmdb_and_then_imdb() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(query_param("tmdb_id", "42"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"intro":[{"start_ms":1000,"end_ms":2000}]}"#),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(query_param("imdb_id", "tt9"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"intro":[{"start_ms":3000,"end_ms":4000}]}"#),
            )
            .expect(1)
            .mount(&server)
            .await;

        let mut tmdb = movie(None, None);
        tmdb.tmdb_id = Some(42);
        let mut imdb = movie(None, None);
        imdb.imdb_id = Some("tt9".to_owned());

        let from_tmdb = adapter(&server.uri(), None)
            .fetch(tmdb)
            .await
            .expect("TMDB is an accepted identity");
        let from_imdb = adapter(&server.uri(), None)
            .fetch(imdb)
            .await
            .expect("IMDb is an accepted identity");

        assert_eq!(from_tmdb[0].start_ms(), 1_000);
        assert_eq!(from_imdb[0].start_ms(), 3_000);
    }

    #[tokio::test]
    async fn a_refused_credential_is_final_and_reported_as_one() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .expect(1)
            .mount(&server)
            .await;

        let error = adapter(&server.uri(), Some("submission-key"))
            .fetch(movie(Some(1), None))
            .await
            .expect_err("a refused credential is not an answer");

        assert_eq!(
            error,
            MarkerError::Unauthorized {
                marker_source: MarkerSource::IntroDb
            }
        );
        assert!(!error.retryable());
    }

    #[tokio::test]
    async fn a_rate_limit_is_reported_with_the_delay_the_provider_named() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
            .expect(u64::from(ATTEMPTS))
            .mount(&server)
            .await;

        let error = adapter(&server.uri(), None)
            .fetch(movie(Some(1), None))
            .await
            .expect_err("a 429 is not an answer");

        assert_eq!(
            error,
            MarkerError::RateLimited {
                marker_source: MarkerSource::IntroDb,
                retry_after: Some(Duration::ZERO),
            }
        );
        assert!(error.retryable());
    }

    #[tokio::test]
    async fn json_that_is_not_the_expected_shape_is_drift_and_bytes_that_are_not_json_are_a_decode()
    {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(query_param("tvdb_id", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"intro":"nope"}"#))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(query_param("tvdb_id", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>maintenance</html>"))
            .expect(1)
            .mount(&server)
            .await;

        let drift = adapter(&server.uri(), None)
            .fetch(movie(Some(1), None))
            .await
            .expect_err("the shape drifted");
        let undecodable = adapter(&server.uri(), None)
            .fetch(movie(Some(2), None))
            .await
            .expect_err("the body is not JSON");

        assert_eq!(
            drift,
            MarkerError::SchemaDrift {
                marker_source: MarkerSource::IntroDb
            }
        );
        assert_eq!(
            undecodable,
            MarkerError::Transport {
                marker_source: MarkerSource::IntroDb,
                kind: TransportFailure::Decode,
            }
        );
    }

    #[tokio::test]
    async fn an_answer_over_the_cap_is_refused_with_its_size() {
        let server = MockServer::start().await;
        let padding = " ".repeat(BODY_CAP as usize);
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(format!(r#"{{"intro":[]}}{padding}"#)),
            )
            .expect(1)
            .mount(&server)
            .await;

        let error = adapter(&server.uri(), None)
            .fetch(movie(Some(1), None))
            .await
            .expect_err("the answer is over the cap");

        match error {
            MarkerError::TooLarge {
                marker_source,
                bytes,
                cap,
            } => {
                assert_eq!(marker_source, MarkerSource::IntroDb);
                assert_eq!(cap, BODY_CAP);
                assert!(bytes > cap, "{bytes} must be over {cap}");
            }
            other => panic!("expected a cap refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_preview_with_no_end_needs_a_runtime() {
        let server = MockServer::start().await;
        let body = r#"{"preview":[{"start_ms":1300000,"end_ms":null}]}"#;
        Mock::given(method("GET"))
            .and(query_param("tvdb_id", "11"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(query_param("tvdb_id", "12"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .expect(1)
            .mount(&server)
            .await;

        let unknown = adapter(&server.uri(), None)
            .fetch(movie(Some(11), None))
            .await
            .expect("the answer is read");
        let known = adapter(&server.uri(), None)
            .fetch(movie(Some(12), Some(1_440_000)))
            .await
            .expect("the answer is read");

        assert!(
            unknown.is_empty(),
            "an end this build does not know must not be invented"
        );
        assert_eq!(known.len(), 1);
        assert_eq!(known[0].kind(), MarkerKind::Preview);
        assert_eq!(known[0].end_ms(), Some(1_440_000));
    }

    #[tokio::test]
    async fn a_segment_that_closes_before_it_opens_is_dropped_and_a_long_one_is_clamped() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"intro":[
                    {"start_ms":60000,"end_ms":60000},
                    {"start_ms":70000,"end_ms":60000},
                    {"start_ms":61000,"end_ms":62000},
                    {"start_ms":1000,"end_ms":2000},
                    {"start_ms":50000,"end_ms":900000}
                ]}"#,
            ))
            .expect(1)
            .mount(&server)
            .await;

        let markers = adapter(&server.uri(), None)
            .fetch(movie(Some(1), Some(60_000)))
            .await
            .expect("the answer is read");

        assert_eq!(markers.len(), 2, "a window with no interior is no window");
        assert_eq!(markers[0].start_ms(), 1_000);
        assert_eq!(markers[0].end_ms(), Some(2_000));
        assert_eq!(markers[1].start_ms(), 50_000);
        assert_eq!(
            markers[1].end_ms(),
            Some(60_000),
            "an end past the runtime is clamped rather than dropped"
        );
    }

    #[tokio::test]
    async fn an_answer_without_marker_arrays_has_no_markers() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(query_param("tvdb_id", "21"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"tvdb_id":21,"type":"tv","season":1,"episode":2}"#),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(query_param("tvdb_id", "22"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"tvdb_id":22,"versions":[{"tvdb_id":22,"intro":[{"start_ms":1,"end_ms":2}]}]}"#,
            ))
            .expect(1)
            .mount(&server)
            .await;

        let bare = adapter(&server.uri(), None)
            .fetch(movie(Some(21), None))
            .await
            .expect("no markers is not a failure");
        let versions = adapter(&server.uri(), None)
            .fetch(movie(Some(22), None))
            .await
            .expect("no markers is not a failure");

        assert!(bare.is_empty());
        assert!(
            versions.is_empty(),
            "the versions shape carries no markers for this subject"
        );
    }

    #[tokio::test]
    async fn the_key_is_sent_only_when_one_was_supplied() {
        let keyed = MockServer::start().await;
        Mock::given(method("GET"))
            .and(header("authorization", "Bearer submission-key"))
            .and(header("user-agent", super::USER_AGENT))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"intro":[]}"#))
            .expect(1)
            .mount(&keyed)
            .await;

        adapter(&keyed.uri(), Some("submission-key"))
            .fetch(movie(Some(1), None))
            .await
            .expect("the keyed lookup answers");

        let anonymous = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"intro":[]}"#))
            .expect(1)
            .mount(&anonymous)
            .await;

        adapter(&anonymous.uri(), None)
            .fetch(movie(Some(1), None))
            .await
            .expect("the anonymous lookup answers");

        let requests = anonymous
            .received_requests()
            .await
            .expect("the server records requests");
        assert_eq!(requests.len(), 1);
        assert!(
            !requests[0].headers.contains_key("authorization"),
            "reads work anonymously, so no credential means no header at all"
        );
    }

    #[test]
    fn only_the_retryable_answers_are_retried_and_the_wait_is_bounded() {
        let rate_limited = MarkerError::RateLimited {
            marker_source: MarkerSource::IntroDb,
            retry_after: Some(Duration::from_hours(1)),
        };

        assert_eq!(
            super::retry_delay(1, &rate_limited),
            Some(super::RETRY_AFTER_CAP),
            "a provider's delay is honoured within the cap"
        );
        assert_eq!(
            super::retry_delay(ATTEMPTS, &rate_limited),
            None,
            "the attempts are a bound, not a hope"
        );
        assert_eq!(
            super::retry_delay(
                1,
                &MarkerError::Unauthorized {
                    marker_source: MarkerSource::IntroDb
                }
            ),
            None
        );
        assert_eq!(
            super::retry_delay(
                1,
                &MarkerError::Transport {
                    marker_source: MarkerSource::IntroDb,
                    kind: TransportFailure::Decode,
                }
            ),
            None,
            "a body that made no sense will not make sense next time"
        );
    }
}
