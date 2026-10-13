//! The TMDB adapter: what TMDB says, translated into this application's own vocabulary.
//!
//! **Nothing TMDB-shaped leaves this module** (ADR 0007 rule 2). The JSON types below are private,
//! they deserialize a *subset* of what the public API documents, and every field that could be absent
//! is optional here rather than assumed — an answer is untrusted input like any other (`PG-E5-05`).
//!
//! This is the adapter's first operation: `search`. The rest of `PG-E5-06` — configuration, genres,
//! trending, discover with the full filter set — and `PG-E5-07`/`PG-E5-08`'s details, credits,
//! videos, seasons, and episodes are what the port will grow to ask for. What is here is the
//! operation the port actually has.
//!
//! **The fixtures these tests are written against are synthetic.** The response shape follows TMDB's
//! public documentation; no request has been made to the live service from this repository, because
//! `AGENTS R14` forbids it and a test that needed a key would be a test nobody could run. When the
//! first live call happens (`PG-E5-20`'s "test connection"), the shapes here are what to check.

use std::time::Duration;

use peregrine_domain::catalogue::{TitleDetails, TitleKind};
use peregrine_domain::{ExternalId, NonEmptyText, Provider};
use reqwest::Url;
use serde::Deserialize;

use crate::capability::{Capabilities, Capability};
use crate::descriptor::{Attribution, CachePolicy, ProviderDescriptor, RatePolicy};
use crate::error::ProviderError;
use crate::http::{HttpClient, HttpPolicy, Request};
use crate::hygiene::{Body, BodyCap};
use crate::provider::{MetadataProvider, ProviderFuture, SearchCandidate, SearchQuery};

/// The largest search answer this adapter will read.
///
/// TMDB's search pages are small — twenty results of ids and names — so a megabyte is generous for a
/// legitimate answer and small enough that a wrong or hostile one is refused before it is parsed.
pub const SEARCH_BODY_CAP: BodyCap = BodyCap::new(std::num::NonZeroU64::new(1_048_576).unwrap());

/// The attribution TMDB's terms require (`PG-E5-19` renders it).
const ATTRIBUTION_NOTICE: &str =
    "This product uses the TMDB API but is not endorsed or certified by TMDB.";

/// The TMDB adapter.
///
/// Holds the descriptor, the client, the base URL, and the credential. It is built by the registry
/// from a descriptor and a key the keyring provided (`PG-E2`), which is why the credential is a
/// parameter rather than something this type goes looking for: an adapter that fetched its own secret
/// would be an adapter that could not be tested without one.
pub struct Tmdb {
    descriptor: ProviderDescriptor,
    client: HttpClient,
    base: String,
    image_base: String,
    credential: String,
}

impl Tmdb {
    /// The production base URL.
    pub const BASE_URL: &'static str = "https://api.themoviedb.org/3";

    /// Builds an adapter against `base` — the live service, or a test server.
    ///
    /// `base` is a parameter rather than a constant so the adapter can be pointed at a `wiremock`
    /// server, which is the only way its behaviour can be tested at all (`AGENTS R14`).
    ///
    /// # Errors
    ///
    /// [`ProviderError`] when the descriptor cannot be built or the HTTP client cannot be constructed,
    /// which is a failure of the policy rather than of anything a provider said.
    pub fn new(base: &str, image_base: &str, credential: String) -> Result<Self, ProviderError> {
        let descriptor = ProviderDescriptor::new(
            Provider::Tmdb,
            "TMDB",
            Capabilities::EMPTY
                .with(Capability::Search)
                .with(Capability::Title)
                .with(Capability::Images),
            // TMDB's documented limit is fifty requests a second; this is far below it, because the
            // cost of being wrong in this direction is a slower sync and in the other is a ban.
            RatePolicy::new(nonzero(20), Duration::from_secs(1), nonzero(10), true).map_err(
                |_| ProviderError::Unsupported {
                    provider: Provider::Tmdb,
                    capability: "a rate policy",
                },
            )?,
            CachePolicy::new(Duration::from_hours(24), None, true).map_err(|_| {
                ProviderError::Unsupported {
                    provider: Provider::Tmdb,
                    capability: "a cache policy",
                }
            })?,
            Some(Attribution::new(ATTRIBUTION_NOTICE, true).map_err(|_| {
                ProviderError::Unsupported {
                    provider: Provider::Tmdb,
                    capability: "an attribution",
                }
            })?),
        )
        .map_err(|_| ProviderError::Unsupported {
            provider: Provider::Tmdb,
            capability: "a descriptor",
        })?;

        let policy = HttpPolicy {
            timeout: Duration::from_secs(15),
            connect_timeout: Duration::from_secs(5),
            user_agent: "Peregrine/0.1 (+https://github.com/peregrine-media/peregrine)".to_owned(),
            backoff: crate::http::Backoff::default(),
        };

        Ok(Self {
            descriptor,
            client: HttpClient::new(Provider::Tmdb, &policy)?,
            base: base.trim_end_matches('/').to_owned(),
            // A parameter for the same reason the API base is: an adapter that could only talk to the
            // live CDN could not be tested at all (`AGENTS R14`).
            image_base: image_base.trim_end_matches('/').to_owned(),
            credential,
        })
    }

    /// The URL one search goes to.
    ///
    /// A `Url` rather than a formatted string, because the query is the user's own text and a title
    /// with an `&` or a space in it must be encoded rather than glued into a path. `include_adult` is
    /// false: a provider-side filter is a courtesy to a household's search box, and the *scope* is what
    /// decides what an account may see (`AGENTS R34`) — the two are different questions and neither
    /// substitutes for the other.
    fn search_url(&self, query: &SearchQuery) -> Result<Url, ProviderError> {
        let endpoint = match query.kind() {
            Some(TitleKind::Movie) => "search/movie",
            Some(TitleKind::Show) => "search/tv",
            None => "search/multi",
        };
        let mut url = Url::parse(&format!("{}/{endpoint}", self.base)).map_err(|_| {
            ProviderError::Transport {
                provider: Provider::Tmdb,
                kind: crate::error::TransportFailure::Connect,
            }
        })?;
        url.query_pairs_mut()
            .append_pair("query", query.text().as_str())
            .append_pair("include_adult", "false")
            .append_pair("page", "1");
        Ok(url)
    }
}

impl MetadataProvider for Tmdb {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn search(&self, query: &SearchQuery) -> ProviderFuture<'_, Vec<SearchCandidate>> {
        // Everything borrowed is read *before* the async block and moved in as owned data, so the
        // returned future borrows only `self` — which is the lifetime the port's elided signature
        // gives it. A block that captured `query` would demand both live as long, and an impl may not
        // be narrower than its trait.
        if !self.descriptor.capabilities().contains(Capability::Search) {
            // The port's own rule: a provider either answers or refuses, and it never spends a request
            // finding out which (`crate::provider`).
            let unsupported = ProviderError::Unsupported {
                provider: Provider::Tmdb,
                capability: "search",
            };
            return Box::pin(async move { Err(unsupported) });
        }
        let limit = query.limit().get() as usize;
        let url = match self.search_url(query) {
            Ok(url) => url.to_string(),
            Err(error) => return Box::pin(async move { Err(error) }),
        };

        Box::pin(async move {
            let fetched = self
                .client
                .fetch(
                    Provider::Tmdb,
                    Request {
                        url: &url,
                        credential: Some(&self.credential),
                    },
                )
                .await?;

            let answer: SearchAnswer =
                Body::new(Provider::Tmdb, &fetched.bytes, fetched.media_type())
                    .decode(SEARCH_BODY_CAP)?;

            let mut candidates = Vec::new();
            for result in answer.results.into_iter().take(limit) {
                if let Some(candidate) = result.into_candidate() {
                    candidates.push(candidate);
                }
            }
            Ok(candidates)
        })
    }

    fn image(
        &self,
        path: &str,
        kind: peregrine_domain::catalogue::ImageKind,
    ) -> ProviderFuture<'_, Vec<u8>> {
        if !self.descriptor.capabilities().contains(Capability::Images) {
            let unsupported = ProviderError::Unsupported {
                provider: Provider::Tmdb,
                capability: "images",
            };
            return Box::pin(async move { Err(unsupported) });
        }
        // The size is this adapter's choice for the kind: a poster is shown in a grid, a backdrop behind
        // a title, and one width each is what those two places ask for.
        let size = match kind {
            peregrine_domain::catalogue::ImageKind::Poster => "w342",
            peregrine_domain::catalogue::ImageKind::Backdrop => "w780",
        };
        // A path from a provider is untrusted input on its way into a URL. Cleaning it a segment at a
        // time is what makes it safe *and* correct: `..` would climb out of the size directory, `.` and
        // an empty segment would produce a `//` that some servers serve and others refuse, and a path
        // beginning with a scheme never gets the chance to replace the host.
        let cleaned = path
            .split('/')
            .filter(|segment| !segment.is_empty() && *segment != ".." && *segment != ".")
            .collect::<Vec<_>>()
            .join("/");
        let url = format!("{}/{size}/{cleaned}", self.image_base);

        Box::pin(async move {
            let fetched = self
                .client
                .fetch(
                    Provider::Tmdb,
                    Request {
                        url: &url,
                        // The image CDN is public: sending the credential to a second host would hand
                        // it to whoever runs that host.
                        credential: None,
                    },
                )
                .await?;
            Ok(fetched.bytes)
        })
    }

    fn title(&self, id: &ExternalId, kind: TitleKind) -> ProviderFuture<'_, TitleDetails> {
        if !self.descriptor.capabilities().contains(Capability::Title) {
            let unsupported = ProviderError::Unsupported {
                provider: Provider::Tmdb,
                capability: "title",
            };
            return Box::pin(async move { Err(unsupported) });
        }
        let endpoint = match kind {
            TitleKind::Movie => "movie",
            TitleKind::Show => "tv",
        };
        let url = format!("{}/{endpoint}/{}", self.base, id.value());
        let credential = self.credential.clone();

        Box::pin(async move {
            let fetched = self
                .client
                .fetch(
                    Provider::Tmdb,
                    Request {
                        url: &url,
                        credential: Some(&credential),
                    },
                )
                .await?;
            let answer: DetailsAnswer =
                Body::new(Provider::Tmdb, &fetched.bytes, fetched.media_type())
                    .decode(SEARCH_BODY_CAP)?;

            let name = answer
                .title
                .or(answer.name)
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty())
                .and_then(|name| NonEmptyText::new(&name).ok())
                .ok_or(ProviderError::SchemaDrift {
                    provider: Provider::Tmdb,
                })?;
            let released = answer
                .release_date
                .or(answer.first_air_date)
                .filter(|date| !date.is_empty())
                .and_then(|date| date.parse().ok());
            let own_name = match kind {
                TitleKind::Movie => answer.original_title,
                TitleKind::Show => answer.original_name,
            };

            Ok(TitleDetails {
                kind,
                name,
                original_name: own_name.filter(|name| !name.is_empty()),
                overview: answer.overview.filter(|text| !text.is_empty()),
                tagline: answer.tagline.filter(|text| !text.is_empty()),
                released,
                runtime_minutes: answer.runtime.or_else(|| {
                    answer
                        .episode_run_time
                        .and_then(|times| times.first().copied())
                }),
                original_language: answer.original_language.filter(|code| !code.is_empty()),
                origin_country: answer.origin_country.filter(|codes| !codes.is_empty()),
                rating: answer.vote_average,
                votes: answer.vote_count,
                popularity: answer.popularity,
                adult: answer.adult,
                in_production: answer.in_production,
                season_count: answer.number_of_seasons,
                episode_count: answer.number_of_episodes,
                homepage: answer.homepage.filter(|url| !url.is_empty()),
                poster_path: answer.poster_path.filter(|path| !path.is_empty()),
                backdrop_path: answer.backdrop_path.filter(|path| !path.is_empty()),
            })
        })
    }
}

/// One title's details, as much of the documented object as this adapter reads.
///
/// One struct for films and series, because the two objects differ only in which of `title`/`name` and
/// `release_date`/`first_air_date` are filled. Everything is optional: a provider omits freely, and an
/// absent field must never be read as "clear this".
#[derive(Debug, Deserialize)]
struct DetailsAnswer {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    original_title: Option<String>,
    #[serde(default)]
    original_name: Option<String>,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    tagline: Option<String>,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    first_air_date: Option<String>,
    #[serde(default)]
    runtime: Option<u32>,
    #[serde(default)]
    episode_run_time: Option<Vec<u32>>,
    #[serde(default)]
    original_language: Option<String>,
    /// Documented for series. Left absent for films rather than guessed at a shape this build has not
    /// seen, because a provider's answer is not the place to find out what its documentation says.
    #[serde(default)]
    origin_country: Option<Vec<String>>,
    #[serde(default)]
    vote_average: Option<f64>,
    #[serde(default)]
    vote_count: Option<u64>,
    #[serde(default)]
    popularity: Option<f64>,
    #[serde(default)]
    adult: Option<bool>,
    #[serde(default)]
    in_production: Option<bool>,
    #[serde(default)]
    number_of_seasons: Option<u32>,
    #[serde(default)]
    number_of_episodes: Option<u32>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    poster_path: Option<String>,
    #[serde(default)]
    backdrop_path: Option<String>,
}

/// Where TMDB serves images, and the two widths this build asks for.
///
/// A second host, which is why it is a constant here rather than something a caller supplies: the image
/// CDN is public, needs no credential, and — per `AGENTS R24`'s documented exception — cannot
/// revalidate, so an image request carries no conditional header.
pub const IMAGE_BASE: &str = "https://image.tmdb.org/t/p";

/// What TMDB answers a search with, as much of it as this adapter reads.
///
/// No `deny_unknown_fields`: a provider that adds a field is not drifting, and refusing an answer
/// because it carries more than this build knows would break on the provider's next deploy.
#[derive(Debug, Deserialize)]
struct SearchAnswer {
    #[serde(default)]
    results: Vec<SearchResult>,
}

/// One search result.
///
/// Everything that could be absent is: a person result carries no title, an unreleased film carries no
/// date, and a name that has not been translated yet can be empty. Each of those is a field this
/// adapter skips rather than a reason to fail the whole answer.
#[derive(Debug, Deserialize)]
struct SearchResult {
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    media_type: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    first_air_date: Option<String>,
}

impl SearchResult {
    /// This result as a candidate, or `None` when it is not a title this build can use.
    ///
    /// A person result, an id TMDB did not send, a name that is empty after trimming, or a kind this
    /// build does not have are all skipped: a search that answered four titles and a person is a
    /// search that found four titles.
    fn into_candidate(self) -> Option<SearchCandidate> {
        let kind = match self.media_type.as_deref() {
            Some("movie") => TitleKind::Movie,
            Some("tv") => TitleKind::Show,
            _ => return None,
        };
        let name = self
            .title
            .or(self.name)
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())?;
        let value = self.id?.to_string();
        let external_id = ExternalId::new(Provider::Tmdb, &value).ok()?;
        let name = NonEmptyText::new(&name).ok()?;
        let released = self
            .release_date
            .or(self.first_air_date)
            .filter(|date| !date.is_empty())
            .and_then(|date| date.parse().ok());
        Some(SearchCandidate::new(external_id, name, kind, released))
    }
}

/// A `NonZeroU32` for the policies above, which are built rather than parsed.
fn nonzero(value: u32) -> std::num::NonZeroU32 {
    std::num::NonZeroU32::new(value).unwrap_or(std::num::NonZeroU32::MIN)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use peregrine_domain::{NonEmptyText, Provider};
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::Tmdb;
    use crate::error::ProviderError;
    use crate::provider::{MetadataProvider, SearchQuery};

    /// A multi-search answer with the three shapes that matter: a film, a series, and a person.
    const MULTI: &str = r#"{
      "page": 1,
      "results": [
        {"media_type": "movie", "id": 603, "title": "The Matrix", "release_date": "1999-03-31"},
        {"media_type": "tv", "id": 1396, "name": "Breaking Bad", "first_air_date": "2008-01-20"},
        {"media_type": "person", "id": 6384, "name": "Keanu Reeves"}
      ],
      "total_results": 3
    }"#;

    fn query(text: &str, limit: u32) -> SearchQuery {
        SearchQuery::new(
            NonEmptyText::new(text).expect("a non-empty query"),
            NonZeroU32::new(limit).expect("a non-zero limit"),
            None,
        )
    }

    /// A JSON answer, with the content type `hygiene` requires.
    ///
    /// Written as a helper because three fixtures first forgot it and every one of them failed with
    /// `Decode` — which is the adapter being right: `PG-E5-05` refuses an answer whose type is not the
    /// one the endpoint documents, and a test that omitted the header was testing that refusal by
    /// accident.
    fn json(body: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_raw(body, "application/json")
    }

    fn adapter(server: &MockServer) -> Tmdb {
        Tmdb::new(&server.uri(), &server.uri(), "test-token".to_owned())
            .expect("the adapter builds")
    }

    #[tokio::test]
    async fn a_search_maps_the_providers_answer_into_domain_candidates() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/multi"))
            .and(query_param("query", "matrix"))
            .and(query_param("include_adult", "false"))
            .respond_with(json(MULTI))
            .mount(&server)
            .await;

        let found = adapter(&server)
            .search(&query("matrix", 20))
            .await
            .expect("a search answers");

        assert_eq!(found.len(), 2, "the person result is not a title");
        assert_eq!(found[0].name().as_str(), "The Matrix");
        assert_eq!(found[0].external_id().provider(), Provider::Tmdb);
        assert_eq!(found[0].external_id().value(), "603");
        assert_eq!(found[1].name().as_str(), "Breaking Bad");
        assert!(found[1].released().is_some(), "the first air date is read");
    }

    #[tokio::test]
    async fn the_request_carries_the_credential_as_a_bearer_header() {
        // Not as a query parameter: a key in a URL reaches every log and every error path, which is
        // why this crate's own error carries no URL either.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(json(MULTI))
            .expect(1)
            .mount(&server)
            .await;

        adapter(&server)
            .search(&query("matrix", 5))
            .await
            .expect("the credential is sent");
    }

    #[tokio::test]
    async fn a_kind_restricts_the_endpoint_rather_than_filtering_the_answer() {
        // Asking the wrong endpoint and dropping half the results would spend a request to learn
        // something the query already said.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/movie"))
            .respond_with(json(r#"{"results":[]}"#))
            .expect(1)
            .mount(&server)
            .await;

        let mut restricted = query("matrix", 5);
        restricted = SearchQuery::new(
            restricted.text().clone(),
            restricted.limit(),
            Some(peregrine_domain::catalogue::TitleKind::Movie),
        );

        adapter(&server)
            .search(&restricted)
            .await
            .expect("the restricted endpoint answers");
    }

    #[tokio::test]
    async fn the_limit_is_respected_and_a_result_with_nothing_to_name_it_is_skipped() {
        // The port's conformance suite checks the limit; this checks what happens to a result that
        // cannot become a candidate at all, which is the malformed case §4 asks about.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/multi"))
            .respond_with(json(
                r#"{"results": [
                    {"media_type": "movie", "id": 1, "title": "   "},
                    {"media_type": "movie", "id": 2, "title": "Dune", "release_date": ""},
                    {"media_type": "unknown", "id": 3, "title": "Nothing"},
                    {"media_type": "movie", "title": "No id"},
                    {"media_type": "movie", "id": 5, "title": "Arrival", "release_date": "2016-11-11"}
                ]}"#,
            ))
            .mount(&server)
            .await;

        let found = adapter(&server)
            .search(&query("anything", 20))
            .await
            .expect("a search answers");

        assert_eq!(found.len(), 2, "Dune and Arrival: {found:?}");
        assert_eq!(found[0].name().as_str(), "Dune");
        assert!(
            found[0].released().is_none(),
            "an empty date is an absent date"
        );
        assert_eq!(found[1].name().as_str(), "Arrival");
    }

    #[tokio::test]
    async fn the_limit_truncates_the_answer() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/multi"))
            .and(query_param("query", "matrix"))
            .respond_with(json(MULTI))
            .mount(&server)
            .await;

        let found = adapter(&server)
            .search(&query("matrix", 1))
            .await
            .expect("a search answers");

        assert_eq!(found.len(), 1);
    }

    #[tokio::test]
    async fn the_statuses_that_are_final_are_reported_as_themselves() {
        for (status, expected) in [
            (404, "not found"),
            (401, "unauthorized"),
            (403, "unauthorized"),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(status))
                .mount(&server)
                .await;

            let error = adapter(&server)
                .search(&query("matrix", 5))
                .await
                .expect_err("the status is final");

            let named = match error {
                ProviderError::NotFound { .. } => "not found",
                ProviderError::Unauthorized { .. } => "unauthorized",
                other => panic!("{status} became {other:?}"),
            };
            assert_eq!(named, expected, "status {status}");
        }
    }

    #[tokio::test]
    async fn an_answer_that_is_not_what_the_provider_documents_is_drift() {
        // A stale row keeps rendering from cache and is marked for a later sync (ADR 0007 rule 7), so
        // the shape has to be a reported state rather than a panic or an empty list.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(json("{\"results\": \"not a list\"}"))
            .mount(&server)
            .await;

        let error = adapter(&server)
            .search(&query("matrix", 5))
            .await
            .expect_err("the shape drifted");

        assert!(
            matches!(error, ProviderError::SchemaDrift { .. }),
            "got {error:?}"
        );
    }

    #[tokio::test]
    async fn a_film_s_details_are_mapped_into_domain_fields() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/movie/603"))
            .respond_with(json(
                r#"{
                    "title": "The Matrix",
                    "original_title": "The Matrix",
                    "overview": "A hacker learns the truth.",
                    "tagline": "Welcome to the real world.",
                    "release_date": "1999-03-31",
                    "runtime": 136,
                    "original_language": "en",
                    "vote_average": 8.2,
                    "vote_count": 24000,
                    "popularity": 42.5,
                    "adult": false,
                    "homepage": "https://thematrix.com",
                    "poster_path": "/poster.jpg",
                    "backdrop_path": "/backdrop.jpg"
                }"#,
            ))
            .mount(&server)
            .await;

        let details = adapter(&server)
            .title(
                &peregrine_domain::ExternalId::new(Provider::Tmdb, "603").expect("an id"),
                peregrine_domain::catalogue::TitleKind::Movie,
            )
            .await
            .expect("the details answer");

        assert_eq!(details.name.as_str(), "The Matrix");
        assert_eq!(
            details.overview.as_deref(),
            Some("A hacker learns the truth.")
        );
        assert_eq!(details.runtime_minutes, Some(136));
        assert_eq!(details.rating, Some(8.2));
        assert_eq!(details.votes, Some(24_000));
        assert_eq!(
            details.released,
            Some("1999-03-31".parse().expect("a date"))
        );
        assert_eq!(details.poster_path.as_deref(), Some("/poster.jpg"));
    }

    #[tokio::test]
    async fn an_image_is_fetched_without_the_credential_and_cannot_climb_out_of_its_directory() {
        // Two things at once, because they are the same request: the public CDN gets **no** credential —
        // sending it to a second host would hand it to whoever runs that host — and a path from a
        // provider cannot escape the size directory it is appended to.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/w342/poster.jpg"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"jpeg".to_vec()))
            .mount(&server)
            .await;

        let adapter = Tmdb::new(
            &format!("{}/3", server.uri()),
            &server.uri(),
            "test-token".to_owned(),
        )
        .expect("the adapter builds");
        let bytes = adapter
            .image(
                // A path a compromised or mistaken provider could send: cleaned, it addresses a file
                // in this size's directory rather than one above it.
                "/../poster.jpg",
                peregrine_domain::catalogue::ImageKind::Poster,
            )
            .await
            .expect("the image answers");
        assert_eq!(bytes, b"jpeg");
    }

    #[tokio::test]
    async fn the_descriptor_declares_what_the_adapter_can_actually_do() {
        // The port's rule: a provider either answers or refuses, and it never spends a request
        // finding out which. This is the declaration the conformance suite reads.
        let server = MockServer::start().await;
        let adapter = adapter(&server);
        let descriptor = adapter.descriptor();

        assert_eq!(descriptor.provider(), Provider::Tmdb);
        assert_eq!(descriptor.display_name(), "TMDB");
        assert!(descriptor.can(crate::capability::Capability::Search));
    }
}
