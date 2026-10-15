//! The `providers` namespace of the command surface (docs/ARCHITECTURE.md §7).
//!
//! The four commands a settings screen needs to configure a metadata provider: what this
//! installation has, and the three things a person does about it — save a key, test the
//! stored one, and remove it.
//!
//! **No command here touches a keyring.** Reading one is blocking and, on macOS, the first
//! read from a new binary raises an OS prompt that measured 11.7 s (PG-E5-12), so the read
//! happens once off the command path and [`status`] answers from that cache. Saving and
//! clearing are jobs for the same reason (`ARCHITECTURE §7` rule 2), and the candidate key
//! lives in the job's memory and in the keyring: no command logs it, returns it, or puts it
//! in an error (SECURITY.md T13).

use peregrine_app::providers::ProviderStatus;
use peregrine_app::{CredentialRequest, Secret};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;

use crate::error::{AppError, ErrorCode};
use crate::guard::guard_response;
use crate::jobs::JobStarted;
use crate::runtime::RuntimeState;

/// One provider this build knows how to talk to, as a screen renders it.
///
/// The display name and the endpoint come from the manifest rather than from the screen, so a
/// second provider is a manifest entry and not a table in the interface (AGENTS R7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDto {
    /// The provider's name, which is what the three write commands take.
    pub provider: String,
    /// What to call it in the interface.
    pub display_name: String,
    /// Where its API lives by default.
    pub endpoint: String,
    /// Whether it holds a credential and can therefore be asked.
    pub configured: bool,
}

/// What the last read of the credential store answered, as the settings screen spells it.
///
/// **A refused read is not an absent key.** The screen that read the two as one thing would tell a
/// user whose key is still in their keychain to paste it again, which is the shape of "reinstalling
/// deleted my key". `peregrine_app::providers::CredentialStore` carries the reason; this is its wire
/// spelling, and the mapping between them is total so a new reason is a compile error here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum CredentialStoreDto {
    /// Nobody has read the store yet, so nothing is known about what it holds.
    Unread,
    /// The store answered, so a provider's `configured` flag is the whole truth.
    Answered,
    /// The store refused access: the key may well still be stored.
    Denied,
    /// There is no store this system can use.
    Unavailable,
}

impl From<peregrine_app::providers::CredentialStore> for CredentialStoreDto {
    fn from(store: peregrine_app::providers::CredentialStore) -> Self {
        use peregrine_app::providers::CredentialStore;
        match store {
            CredentialStore::Unread => Self::Unread,
            CredentialStore::Answered => Self::Answered,
            CredentialStore::Denied => Self::Denied,
            CredentialStore::Unavailable => Self::Unavailable,
        }
    }
}

/// What this installation has configured, as the settings screen reads it.
///
/// One list rather than a known set and a configured set: a provider is known by being here
/// and configured by its flag, and two lists of the same entries would be two things to keep
/// in step. A provider that needs a credential and has none is here and not configured, which
/// is the state every installation starts in — **and `store` is the one fact that tells that state
/// apart from a keychain the app was not allowed to read.**
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatusDto {
    /// The providers this build knows, whether or not they can be used.
    pub known: Vec<ProviderDto>,
    /// What the last read of the credential store answered.
    pub store: CredentialStoreDto,
}

impl From<ProviderStatus> for ProviderStatusDto {
    fn from(status: ProviderStatus) -> Self {
        // **The known set is the manifest's and only the configured flag is the cache's.** The
        // refresh that fills the cache runs off the startup and command paths — a keyring read is
        // blocking — so a status read can arrive before it, and an answer built from the cache's
        // copy would then say this build knows no provider at all. What a build can talk to is a
        // constant, and what it holds a credential for is the cache's answer.
        let known = peregrine_app::providers::MANIFEST
            .iter()
            .map(|entry| ProviderDto {
                provider: entry.provider.to_owned(),
                display_name: entry.display_name.to_owned(),
                endpoint: entry.endpoint.to_owned(),
                configured: status.configured.contains(&entry.provider),
            })
            .collect();
        Self {
            known,
            store: CredentialStoreDto::from(status.store),
        }
    }
}

/// What this installation has configured.
///
/// The cached answer: neither this command nor a poll of it ever reads a keyring, which is
/// what keeps a locked keychain from stalling the settings screen.
///
/// # Errors
///
/// [`ErrorCode::Internal`] when the application runtime is not running.
#[tauri::command]
#[specta::specta]
pub async fn status(state: State<'_, RuntimeState>) -> Result<ProviderStatusDto, AppError> {
    let status = state.runtime()?.provider_status();
    Ok(guard_response(
        "providers::status",
        ProviderStatusDto::from(status),
    ))
}

/// Stores a provider's API key, proving it against the provider first.
///
/// The key is trimmed, so pasting one with a trailing newline works; a key that is empty once
/// trimmed is refused here rather than stored, because a job whose probe can only fail is a
/// job worth not starting. The candidate is moved into the job and never logged, returned, or
/// put in an error (SECURITY.md T13).
///
/// # Errors
///
/// [`ErrorCode::InvalidInput`] when the key is empty or whitespace only, or when this build
/// knows no provider by that name. [`AppError`] when the journal or the queue refuses the job.
#[tauri::command]
#[specta::specta]
pub async fn set_key(
    state: State<'_, RuntimeState>,
    provider: String,
    key: String,
) -> Result<JobStarted, AppError> {
    let key = key.trim();
    if key.is_empty() {
        return Err(AppError::new(ErrorCode::InvalidInput)
            .with_detail("Paste your API key before saving. The field is empty."));
    }
    let started = state
        .runtime()?
        .start_provider_credential(&provider, CredentialRequest::Replace(Secret::new(key)))
        .await?;
    Ok(guard_response(
        "providers::set_key",
        JobStarted::from(started),
    ))
}

/// Asks the provider whether the stored key still works.
///
/// # Errors
///
/// [`ErrorCode::InvalidInput`] when this build knows no provider by that name, and [`AppError`]
/// when the journal or the queue refuses the job.
#[tauri::command]
#[specta::specta]
pub async fn test_connection(
    state: State<'_, RuntimeState>,
    provider: String,
) -> Result<JobStarted, AppError> {
    let started = state
        .runtime()?
        .start_provider_credential(&provider, CredentialRequest::TestConnection)
        .await?;
    Ok(guard_response(
        "providers::test_connection",
        JobStarted::from(started),
    ))
}

/// Removes whatever credential is stored for a provider.
///
/// Removal is not a deletion of anything else: titles already filled in stay, and the provider
/// is simply no longer asked until a key is stored again.
///
/// # Errors
///
/// [`ErrorCode::InvalidInput`] when this build knows no provider by that name, and [`AppError`]
/// when the journal or the queue refuses the job.
#[tauri::command]
#[specta::specta]
pub async fn clear_key(
    state: State<'_, RuntimeState>,
    provider: String,
) -> Result<JobStarted, AppError> {
    let started = state
        .runtime()?
        .start_provider_credential(&provider, CredentialRequest::Forget)
        .await?;
    Ok(guard_response(
        "providers::clear_key",
        JobStarted::from(started),
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use peregrine_app::events::{Event, EventSink, EventSinkError};
    use peregrine_app::providers::ProviderStatus;
    use peregrine_app::{Observability, Runtime, RuntimeConfig};
    use tauri::Manager as _;
    use tempfile::TempDir;

    use super::*;

    /// The endpoint the manifest holds for TMDB, pinned as a wire value rather than read back
    /// from the same source the answer was built from.
    const TMDB_ENDPOINT: &str = "https://api.themoviedb.org/3";

    /// A sink that discards every event: these tests are about the command surface, and the bus's
    /// own delivery is tested in the crate that owns it.
    struct SilentSink;

    impl EventSink for SilentSink {
        fn emit(&self, _event: &Event) -> Result<(), EventSinkError> {
            Ok(())
        }
    }

    /// Starts a runtime into a mock app, so a command can be called the way Tauri calls it.
    async fn served(directory: &TempDir) -> tauri::App<tauri::test::MockRuntime> {
        let app = tauri::test::mock_app();
        let config = RuntimeConfig::new(
            directory.path(),
            directory.path().join("artwork"),
            Arc::new(SilentSink),
        )
        .with_secrets(Arc::new(crate::test_support::NoCredentials));
        let (subscriber, logging) = Observability::build(&config.log_config()).unwrap();
        drop(subscriber);
        let runtime = Runtime::start(config, logging).await.unwrap();
        app.manage(RuntimeState::for_test(Arc::new(runtime)));
        app
    }

    /// Waits for the startup refresh to *finish*, which runs off the startup and command paths.
    ///
    /// **The read finishing is the thing to wait for, not the known list.** `known` is the build's own
    /// manifest and is in place before any read happens, so a wait on it would pass instantly and test
    /// nothing; the read is what can be late, and it can end in a refusal on a machine with no usable
    /// keychain — which is an answer this test must accept rather than wait past.
    async fn await_cache(runtime: &Runtime) {
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            while runtime.provider_status().store
                == peregrine_app::providers::CredentialStore::Unread
            {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the startup refresh must finish, whatever it answers");
    }

    /// A provider the build knows is named and placed even before the cache is filled.
    ///
    /// What a build can talk to is a constant; the refresh that fills the cache is what can be
    /// late, and an answer built from the cache alone would claim this build knows nothing.
    #[test]
    fn a_known_provider_carries_its_manifest_name_and_endpoint() {
        let answer = ProviderStatusDto::from(ProviderStatus::default());

        assert_eq!(answer.known.len(), peregrine_app::providers::MANIFEST.len());
        let tmdb = &answer.known[0];
        assert_eq!(tmdb.provider, "tmdb");
        assert_eq!(tmdb.display_name, "TMDB");
        assert_eq!(tmdb.endpoint, TMDB_ENDPOINT);
        assert!(
            !tmdb.configured,
            "a provider with no credential is known and not configured"
        );
    }

    /// The two states a screen draws differ by one flag and not by a second entry.
    #[test]
    fn a_configured_provider_is_the_same_entry_with_the_flag_set() {
        let answer = ProviderStatusDto::from(ProviderStatus {
            known: Vec::new(),
            configured: vec!["tmdb"],
            store: peregrine_app::providers::CredentialStore::Answered,
        });

        assert_eq!(answer.known.len(), 1);
        assert!(answer.known[0].configured);
        assert_eq!(answer.store, CredentialStoreDto::Answered);
    }

    /// A keychain the app was not allowed to read crosses as its own state, so the screen can say
    /// "your key is still there" instead of "no key stored".
    #[test]
    fn a_refused_credential_read_is_not_an_absent_key() {
        for (store, expected) in [
            (
                peregrine_app::providers::CredentialStore::Denied,
                CredentialStoreDto::Denied,
            ),
            (
                peregrine_app::providers::CredentialStore::Unavailable,
                CredentialStoreDto::Unavailable,
            ),
            (
                peregrine_app::providers::CredentialStore::Unread,
                CredentialStoreDto::Unread,
            ),
        ] {
            let answer = ProviderStatusDto::from(ProviderStatus {
                known: Vec::new(),
                configured: Vec::new(),
                store,
            });
            assert_eq!(
                answer.store, expected,
                "the reason a read failed must survive the wire"
            );
            assert!(
                answer.known.iter().all(|entry| !entry.configured),
                "a refused read configures nothing"
            );
        }
    }

    /// Every reason the store can give has a spelling the webview can switch on, and the mapping is
    /// total — a fourth reason in the core is a compile error here rather than a silent `unread`.
    #[test]
    fn every_credential_store_state_has_its_own_word() {
        use peregrine_app::providers::CredentialStore;
        let words: Vec<CredentialStoreDto> = CredentialStore::ALL
            .iter()
            .map(|store| CredentialStoreDto::from(*store))
            .collect();
        assert_eq!(
            words,
            vec![
                CredentialStoreDto::Unread,
                CredentialStoreDto::Answered,
                CredentialStoreDto::Denied,
                CredentialStoreDto::Unavailable,
            ]
        );
        for (store, word) in CredentialStore::ALL.iter().zip(words) {
            assert_eq!(
                serde_json::to_value(word).expect("serializes"),
                serde_json::Value::String(
                    match store {
                        CredentialStore::Unread => "unread",
                        CredentialStore::Answered => "answered",
                        CredentialStore::Denied => "denied",
                        CredentialStore::Unavailable => "unavailable",
                    }
                    .to_owned()
                )
            );
        }
    }

    /// A name the build does not know cannot become a row by appearing in the cache.
    #[test]
    fn a_cached_name_the_build_does_not_know_is_not_described() {
        let answer = ProviderStatusDto::from(ProviderStatus {
            known: vec!["nobody"],
            configured: vec!["nobody"],
            store: peregrine_app::providers::CredentialStore::Answered,
        });

        assert_eq!(answer.known.len(), peregrine_app::providers::MANIFEST.len());
        assert!(
            answer.known.iter().all(|entry| entry.provider != "nobody"),
            "a name with no manifest entry has no facts to show: {answer:?}"
        );
    }

    /// The shape every job-starting command answers with, built without starting work.
    #[test]
    fn a_job_handle_names_the_scheduler_job_and_its_journal_row() {
        let job = peregrine_app::jobs::parse_job_id("01ARZ3NDEKTSV4RRFFQ69G5FAV")
            .expect("a well-formed id");
        let handle = JobStarted::from(peregrine_app::StartedJob {
            job,
            journal_id: 7,
            coalesced: false,
        });

        assert_eq!(handle.job_id, "01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_eq!(handle.journal_id, 7);
    }

    /// The answer a screen draws names every provider this build knows, and only the configured
    /// flag is the cache's.
    #[tokio::test]
    async fn status_answers_the_providers_the_build_knows() {
        let directory = TempDir::new().unwrap();
        let app = served(&directory).await;
        let state = app.state::<RuntimeState>();
        await_cache(state.runtime().unwrap()).await;

        let answer = status(state.clone()).await.unwrap();
        let tmdb = answer
            .known
            .iter()
            .find(|entry| entry.provider == "tmdb")
            .expect("TMDB is a provider this build knows");

        assert_eq!(tmdb.display_name, "TMDB");
        assert_eq!(tmdb.endpoint, TMDB_ENDPOINT);
        assert!(
            !tmdb.configured,
            "the fixture's store holds nothing, so the flag is this test's answer and not the machine's"
        );
        assert_eq!(
            answer.store,
            CredentialStoreDto::Answered,
            "the store answered: it holds nothing, which is not the same as being refused"
        );

        crate::runtime::shutdown(app.handle()).await;
    }

    /// An empty field is refused with the instruction to fill it, before any job is written.
    #[tokio::test]
    async fn an_empty_key_is_refused_with_an_instruction_and_no_job() {
        let directory = TempDir::new().unwrap();
        let app = served(&directory).await;
        let state = app.state::<RuntimeState>();

        for key in ["", "   ", "\t\n"] {
            let refused = set_key(state.clone(), "tmdb".to_owned(), key.to_owned())
                .await
                .expect_err("a key that is empty once trimmed cannot be stored");

            assert_eq!(refused.code, ErrorCode::InvalidInput, "{refused:?}");
            let detail = refused.detail.expect("the refusal says what to do");
            assert!(
                detail.contains("Paste your API key"),
                "the refusal must be actionable: {detail}"
            );
        }
        assert!(
            state
                .runtime()
                .unwrap()
                .jobs(None, 10)
                .await
                .unwrap()
                .is_empty(),
            "a refused key must not leave a job behind"
        );

        crate::runtime::shutdown(app.handle()).await;
    }

    /// A padded key is trimmed rather than read as empty.
    ///
    /// The scheduler is stopped first, so the submitted job cannot run its probe and no request
    /// leaves the machine (AGENTS R14). The refusal that comes back is the queue's, which is what
    /// says the key was accepted for storing rather than rejected as blank.
    #[tokio::test]
    async fn a_padded_key_is_trimmed_rather_than_read_as_empty() {
        const KEY: &str = "a-padded-key";
        let directory = TempDir::new().unwrap();
        let app = served(&directory).await;
        let state = app.state::<RuntimeState>();
        state.runtime().unwrap().shutdown().await;

        let refused = set_key(state.clone(), "tmdb".to_owned(), format!("  {KEY}  "))
            .await
            .expect_err("a stopped scheduler refuses work");

        assert_eq!(refused.code, ErrorCode::Internal, "{refused:?}");
        let row = state
            .runtime()
            .unwrap()
            .jobs(None, 10)
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("the row is written before the queue is asked");
        assert!(
            !row.last_error.as_deref().unwrap_or_default().contains(KEY),
            "the row carries no credential: {row:?}"
        );

        crate::runtime::shutdown(app.handle()).await;
    }

    /// A provider this build does not know is refused by name, and nothing is written.
    #[tokio::test]
    async fn an_unknown_provider_is_refused_before_a_job_is_written() {
        let directory = TempDir::new().unwrap();
        let app = served(&directory).await;
        let state = app.state::<RuntimeState>();

        let refused = set_key(state.clone(), "nobody".to_owned(), "a-key".to_owned())
            .await
            .expect_err("an unknown provider cannot be stored against");

        assert_eq!(refused.code, ErrorCode::InvalidInput, "{refused:?}");
        assert!(
            refused.detail.unwrap_or_default().contains("nobody"),
            "the refusal must name what was asked for"
        );
        let rows = state.runtime().unwrap().jobs(None, 10).await.unwrap();
        assert!(
            rows.is_empty(),
            "a refused request must not leave a journal row behind: {rows:?}"
        );

        crate::runtime::shutdown(app.handle()).await;
    }
}
