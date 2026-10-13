//! The provider HTTP client: timeouts, retries, and the policy between them.
//!
//! Three things live here and nothing else. The **client** is a `reqwest` one built from a policy —
//! rustls, HTTP/2, and the compression features a provider's CDN answers best with, with a
//! descriptive `User-Agent` because some providers refuse requests without one and an honest one is
//! required rather than spoofed (AGENTS R24). The **retry curve** is [`Backoff`], which is a rule and
//! is therefore pure: it answers with the delay before an attempt or `None` to stop, and a test drives
//! it at a fixed instant rather than by sleeping. And the **classification** is which of a provider's
//! answers are worth another attempt — a `429` with `Retry-After`, a `5xx`, a timeout — and which are
//! final.
//!
//! **A response's bytes are not decoded here.** What arrives is bytes and a content type; turning them
//! into a value is `crate::hygiene`'s job, because a response is untrusted input whatever carried it
//! (`PG-E5-05`), and the adapter that knows the provider is the one that can name it.

use std::time::Duration;

use peregrine_domain::Provider;
use reqwest::header::{CONTENT_TYPE, RETRY_AFTER};
use reqwest::{Client, StatusCode};
use tokio::time::sleep;

use crate::error::{ProviderError, TransportFailure};

/// The longest a provider's own `Retry-After` may hold a request back.
///
/// A provider is entitled to say "come back in an hour" and the client should respect it, but a
/// header is untrusted input like any other and a wrong or hostile value would strand the request
/// for a day. Five minutes is longer than any provider's documented backoff and short enough that a
/// wrong value is a delay rather than a hang.
pub const MAXIMUM_RETRY_AFTER: Duration = Duration::from_secs(300);

/// The retry curve: how many attempts, and how long between them.
///
/// Doubling from `base`, capped at `max`, and a provider's `Retry-After` wins over the curve when it
/// names one — capped by [`MAXIMUM_RETRY_AFTER`], because honouring it exactly is honouring an
/// untrusted number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    /// How many times a request may be attempted in total, including the first.
    pub attempts: u32,
    /// How long to wait before the second attempt.
    pub base: Duration,
    /// The longest the curve itself may grow to.
    pub max: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            attempts: 3,
            base: Duration::from_millis(500),
            max: Duration::from_secs(30),
        }
    }
}

impl Backoff {
    /// How long to wait before attempt `attempt` (1-based), or `None` to stop trying.
    ///
    /// `attempt` is the attempt that just *failed*: `delay(1, _)` is the wait before the second try.
    /// `retry_after` is what the provider asked for, when it asked.
    #[must_use]
    pub fn delay(&self, attempt: u32, retry_after: Option<Duration>) -> Option<Duration> {
        if attempt >= self.attempts {
            return None;
        }
        if let Some(asked) = retry_after {
            return Some(asked.min(MAXIMUM_RETRY_AFTER));
        }
        // `2^(attempt-1)`, saturating: a large attempt count must not overflow into a zero delay,
        // which would turn a backoff into a hot loop against a provider that is already refusing.
        let factor = 1u32
            .checked_shl(attempt.saturating_sub(1))
            .unwrap_or(u32::MAX);
        Some(self.base.saturating_mul(factor).min(self.max))
    }
}

/// What the client is allowed to spend on one request, and who it says it is.
#[derive(Debug, Clone)]
pub struct HttpPolicy {
    /// How long the whole request may take, including the body.
    pub timeout: Duration,
    /// How long a connection may take to open.
    pub connect_timeout: Duration,
    /// The `User-Agent`, which identifies the application (`AGENTS R24`).
    pub user_agent: String,
    /// The retry curve.
    pub backoff: Backoff,
}

/// One provider's answer: the bytes, and what the provider said they are.
///
/// Handed to `crate::hygiene` by the adapter that knows which provider it asked, because a body
/// cannot be validated without knowing whose it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    /// The response body.
    pub bytes: Vec<u8>,
    /// The `Content-Type`, when the provider sent one.
    pub content_type: Option<String>,
}

impl Fetched {
    /// The content type without its parameters, which is what a check compares (`PG-E5-05`).
    #[must_use]
    pub fn media_type(&self) -> Option<&str> {
        self.content_type
            .as_deref()
            .and_then(|value| value.split(';').next())
            .map(str::trim)
    }
}

/// The provider HTTP client.
#[derive(Debug, Clone)]
pub struct HttpClient {
    client: Client,
    backoff: Backoff,
}

impl HttpClient {
    /// Builds a client from a policy.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Transport`] with [`TransportFailure::Connect`] when the client cannot be built,
    /// which happens for a policy the TLS backend cannot satisfy rather than for anything a provider
    /// said.
    pub fn new(provider: Provider, policy: &HttpPolicy) -> Result<Self, ProviderError> {
        let client = Client::builder()
            .timeout(policy.timeout)
            .connect_timeout(policy.connect_timeout)
            // A redirect to another host would carry the request to a place the caller did not ask
            // about, and a provider that moved answers with a status rather than a redirect.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(policy.user_agent.clone())
            .build()
            .map_err(|_| ProviderError::Transport {
                provider,
                kind: TransportFailure::Connect,
            })?;
        Ok(Self {
            client,
            backoff: policy.backoff,
        })
    }

    /// Fetches one URL, retrying what the policy says is worth retrying.
    ///
    /// # Errors
    ///
    /// [`ProviderError`]: `RateLimited` when the provider refused with `429` and the attempts ran
    /// out, `NotFound` and `Unauthorized` for the two statuses that are final and named, and
    /// `Transport` for a timeout, a connection failure, or any other status.
    pub async fn get(&self, provider: Provider, url: &str) -> Result<Fetched, ProviderError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let outcome = self.attempt(provider, url).await;
            match outcome {
                Ok(fetched) => return Ok(fetched),
                Err((error, retry_after)) => {
                    // `is_retryable` first, and its absence was this loop's first bug: it retried
                    // whatever the curve allowed, so a `404` was asked for again. The rule is tested on
                    // its own *and* it has to be consulted here, which is the difference between a
                    // tested function and a used one.
                    if !is_retryable(&error) {
                        return Err(error);
                    }
                    let Some(delay) = self.backoff.delay(attempt, retry_after) else {
                        return Err(error);
                    };
                    sleep(delay).await;
                }
            }
        }
    }

    /// One attempt, with the delay the caller should wait and the error to report if it stops.
    async fn attempt(
        &self,
        provider: Provider,
        url: &str,
    ) -> Result<Fetched, (ProviderError, Option<Duration>)> {
        let response = match self.client.get(url).send().await {
            Ok(response) => response,
            Err(error) => {
                // A timeout and a refused connection are both worth another attempt: neither says
                // anything about the request itself. A body that failed to arrive is the same shape.
                // The error carries no detail on purpose: a URL can hold an api key, and this is the
                // value that reaches a log (`crates/peregrine-providers/src/error.rs`).
                let kind = if error.is_timeout() {
                    TransportFailure::Timeout
                } else {
                    TransportFailure::Connect
                };
                return Err((ProviderError::Transport { provider, kind }, None));
            }
        };

        let status = response.status();
        if status.is_success() {
            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            return match response.bytes().await {
                Ok(bytes) => Ok(Fetched {
                    bytes: bytes.to_vec(),
                    content_type,
                }),
                Err(_) => Err((
                    ProviderError::Transport {
                        provider,
                        kind: TransportFailure::Interrupted,
                    },
                    None,
                )),
            };
        }

        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(Duration::from_secs);

        Err((classify(provider, status, retry_after), retry_after))
    }
}

/// The error a status is, and whether it is worth another attempt.
///
/// A `429` and a `5xx` are the provider saying "not now" — the first explicitly, the second by
/// failing — and both are retried. `404` and `401`/`403` are answers about the request and are final:
/// retrying a request for something that does not exist spends a provider's rate limit to be told the
/// same thing again.
fn classify(
    provider: Provider,
    status: StatusCode,
    retry_after: Option<Duration>,
) -> ProviderError {
    match status {
        StatusCode::TOO_MANY_REQUESTS => ProviderError::RateLimited {
            provider,
            retry_after,
        },
        StatusCode::NOT_FOUND => ProviderError::NotFound { provider },
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            ProviderError::Unauthorized { provider }
        }
        _ => ProviderError::Transport {
            provider,
            kind: if status.is_server_error() {
                TransportFailure::Interrupted
            } else {
                TransportFailure::Decode
            },
        },
    }
}

/// Whether an error is worth another attempt, which is what the caller's retry loop asks.
///
/// A function rather than a `match` inside the loop so the policy is testable without a server: the
/// loop asks this, and this answers from the error's own shape.
#[must_use]
pub fn is_retryable(error: &ProviderError) -> bool {
    match error {
        ProviderError::RateLimited { .. } => true,
        ProviderError::Transport { kind, .. } => !matches!(kind, TransportFailure::Decode),
        ProviderError::NotFound { .. }
        | ProviderError::Unauthorized { .. }
        | ProviderError::Unsupported { .. }
        | ProviderError::SchemaDrift { .. }
        | ProviderError::TooLarge { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use peregrine_domain::Provider;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{Backoff, HttpClient, HttpPolicy, MAXIMUM_RETRY_AFTER};
    use crate::error::{ProviderError, TransportFailure};

    fn policy(backoff: Backoff) -> HttpPolicy {
        HttpPolicy {
            timeout: Duration::from_secs(5),
            connect_timeout: Duration::from_secs(1),
            user_agent: "Peregrine/test".to_owned(),
            backoff,
        }
    }

    /// A curve that waits a millisecond, so a retry test is fast rather than timed.
    fn quick(attempts: u32) -> Backoff {
        Backoff {
            attempts,
            base: Duration::from_millis(1),
            max: Duration::from_millis(5),
        }
    }

    fn client(backoff: Backoff) -> HttpClient {
        HttpClient::new(Provider::Tmdb, &policy(backoff)).expect("a client builds")
    }

    #[test]
    fn the_curve_doubles_and_stops_at_the_last_attempt() {
        let backoff = Backoff {
            attempts: 4,
            base: Duration::from_millis(100),
            max: Duration::from_secs(1),
        };

        assert_eq!(backoff.delay(1, None), Some(Duration::from_millis(100)));
        assert_eq!(backoff.delay(2, None), Some(Duration::from_millis(200)));
        assert_eq!(backoff.delay(3, None), Some(Duration::from_millis(400)));
        assert_eq!(
            backoff.delay(4, None),
            None,
            "the fourth attempt is the last, so nothing waits for a fifth"
        );
    }

    #[test]
    fn the_curve_is_capped_and_never_overflows_to_nothing() {
        // A curve that overflowed would answer a delay of zero, which is a hot loop against a
        // provider that is already refusing — the failure mode this rule exists to prevent.
        let backoff = Backoff {
            attempts: 64,
            base: Duration::from_millis(10),
            max: Duration::from_millis(50),
        };

        assert_eq!(backoff.delay(2, None), Some(Duration::from_millis(20)));
        assert_eq!(backoff.delay(4, None), Some(Duration::from_millis(50)));
        assert_eq!(backoff.delay(40, None), Some(Duration::from_millis(50)));
        assert_ne!(backoff.delay(40, None), Some(Duration::ZERO));
    }

    #[test]
    fn a_provider_that_says_when_to_come_back_is_obeyed_within_reason() {
        let backoff = Backoff::default();

        assert_eq!(
            backoff.delay(1, Some(Duration::from_secs(9))),
            Some(Duration::from_secs(9)),
            "the provider's own number wins over the curve"
        );
        assert_eq!(
            backoff.delay(1, Some(Duration::from_hours(24))),
            Some(MAXIMUM_RETRY_AFTER),
            "but it is an untrusted header like any other"
        );
    }

    #[test]
    fn what_is_worth_another_attempt_is_a_rule_of_its_own() {
        assert!(super::is_retryable(&ProviderError::RateLimited {
            provider: Provider::Tmdb,
            retry_after: None,
        }));
        assert!(super::is_retryable(&ProviderError::Transport {
            provider: Provider::Tmdb,
            kind: TransportFailure::Timeout,
        }));
        assert!(
            !super::is_retryable(&ProviderError::Transport {
                provider: Provider::Tmdb,
                kind: TransportFailure::Decode,
            }),
            "an answer that arrived and made no sense will not make sense next time"
        );
        assert!(!super::is_retryable(&ProviderError::NotFound {
            provider: Provider::Tmdb,
        }));
        assert!(!super::is_retryable(&ProviderError::Unauthorized {
            provider: Provider::Tmdb,
        }));
    }

    #[tokio::test]
    async fn a_rate_limited_answer_is_retried_and_then_succeeds() {
        // PG-E5-02's acceptance, first case: a 429 with `Retry-After` behaves per policy.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/title"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/title"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .expect(1)
            .mount(&server)
            .await;

        let fetched = client(quick(3))
            .get(Provider::Tmdb, &format!("{}/title", server.uri()))
            .await
            .expect("the second attempt answers");

        assert_eq!(fetched.bytes, b"{}");
    }

    #[tokio::test]
    async fn a_server_error_is_retried_and_then_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;

        let fetched = client(quick(3))
            .get(Provider::Tmdb, &server.uri())
            .await
            .expect("the second attempt answers");

        assert_eq!(fetched.bytes, b"ok");
    }

    #[tokio::test]
    async fn a_missing_title_is_not_asked_for_again() {
        // The other half of the policy: a 404 is an answer about the request, and retrying it spends a
        // provider's rate limit to be told the same thing again.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;

        let error = client(quick(4))
            .get(Provider::Tmdb, &server.uri())
            .await
            .expect_err("a 404 is final");

        assert!(matches!(error, ProviderError::NotFound { .. }));
    }

    #[tokio::test]
    async fn a_provider_that_keeps_failing_is_given_up_on() {
        // The attempts are a bound, not a hope: a provider answering 500 forever is asked exactly
        // `attempts` times and then reported.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .expect(2)
            .mount(&server)
            .await;

        let error = client(quick(2))
            .get(Provider::Tmdb, &server.uri())
            .await
            .expect_err("two attempts, then the error");

        assert!(matches!(error, ProviderError::Transport { .. }));
    }

    #[tokio::test]
    async fn a_provider_that_never_answers_times_out_under_the_policy() {
        // PG-E5-02's acceptance, third case. The margins are wide on purpose: the point is which
        // failure is reported, not how precisely a clock behaved.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
            .mount(&server)
            .await;
        let mut http = policy(quick(1));
        http.timeout = Duration::from_millis(50);
        let client = HttpClient::new(Provider::Tmdb, &http).expect("a client builds");

        let error = client
            .get(Provider::Tmdb, &server.uri())
            .await
            .expect_err("the request times out");

        assert!(matches!(
            error,
            ProviderError::Transport {
                kind: TransportFailure::Timeout,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn the_client_says_who_it_is() {
        // AGENTS R24: a descriptive User-Agent is required identification rather than spoofing, and
        // some providers refuse a request without one.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(header("user-agent", "Peregrine/test"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        client(quick(1))
            .get(Provider::Tmdb, &server.uri())
            .await
            .expect("the request carries the agent");
    }
}
