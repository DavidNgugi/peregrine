//! PG-E2-10: the IPC contract as a suite generated from the command registry.
//!
//! docs/ARCHITECTURE.md §7 is a promise about the whole command surface, not about the
//! commands that happen to exist today, so the suite is driven from the surface itself rather
//! than from a list beside it. [`registered_commands`] reads the command names back out of the bindings
//! the crate's own [`builder`](crate::builder) exports, which is the same `collect_commands!`
//! the desktop shell installs: registering a seventh command without a case here fails
//! `every_registered_command_has_a_case`, naming it.
//!
//! [`cases`] is the single source of truth for what each command does when it fails. A case
//! drives the command through a mock application in the state that makes it fail — a runtime
//! that never started, a step count outside the documented range, a job id nothing is running
//! — and records the code and retryability §7 rule 3 promises, so nothing here asserts on a
//! hand-built [`AppError`].

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use peregrine_app::events::{Event, EventSink, EventSinkError};
use peregrine_app::{Observability, Runtime, RuntimeConfig};
use tauri::test::MockRuntime;
use tauri::{AppHandle, Manager as _, State};
use tempfile::TempDir;

use crate::error::{AppError, ErrorCode};
use crate::guard::MAX_RESPONSE_BYTES;
use crate::runtime::RuntimeState;
use crate::{jobs, system};

/// A job id that is well formed and belongs to no scheduler in this process, so cancelling it
/// is the second half of `cancel`'s failure path: the request is understood, the job is not
/// there.
const UNKNOWN_JOB: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

/// The marker the exporter writes for every registered command. Its argument is the wire name
/// the registry registers.
const INVOKE: &str = "__TAURI_INVOKE(\"";

/// The DTO a command answers with when it has started a job, which is what makes it job-scoped
/// (ARCHITECTURE §7 rule 2).
const JOB_HANDLE: &str = "JobStarted";

/// A sink that discards every event: this suite is about the command surface, and the bus's
/// own delivery is tested in the crate that owns it.
struct SilentSink;

impl EventSink for SilentSink {
    fn emit(&self, _event: &Event) -> Result<(), EventSinkError> {
        Ok(())
    }
}

/// The two applications a case can be handed.
///
/// Which failure a command has depends on the state it was given, so the harness holds both
/// shapes the contract has to cover: an application whose runtime never started — the state a
/// command sees when startup lost, built through PG-E2-02's `RuntimeState::failed` — and one
/// whose runtime is running against a temporary data directory.
struct Harness {
    dead: tauri::App<MockRuntime>,
    served: tauri::App<MockRuntime>,
}

impl Harness {
    /// Builds both applications, starting a runtime into the second.
    async fn new(directory: &TempDir) -> Self {
        let dead = tauri::test::mock_app();
        dead.manage(RuntimeState::failed(
            "the library database could not be opened",
        ));

        let served = tauri::test::mock_app();
        let config = RuntimeConfig::new(
            directory.path(),
            directory.path().join("artwork"),
            Arc::new(SilentSink),
        )
        // A command test never reaches the developer's keychain (`AGENTS R14`): a real read from a
        // freshly built binary raises a macOS access prompt and waits on a person.
        .with_secrets(Arc::new(crate::test_support::NoCredentials));
        let (subscriber, logging) = Observability::build(&config.log_config()).unwrap();
        drop(subscriber);
        let runtime = Runtime::start(config, logging).await.unwrap();
        served.manage(RuntimeState::for_test(Arc::new(runtime)));

        Self { dead, served }
    }

    /// The state of the application whose runtime never started.
    fn dead_state(&self) -> State<'_, RuntimeState> {
        self.dead.state::<RuntimeState>()
    }

    /// The state of the application whose runtime is running.
    fn served_state(&self) -> State<'_, RuntimeState> {
        self.served.state::<RuntimeState>()
    }

    /// The handle of the application whose runtime never started, which is all `build_info`
    /// is given.
    fn dead_handle(&self) -> AppHandle<MockRuntime> {
        self.dead.handle().clone()
    }

    /// Drains both runtimes. The dead one has nothing to drain.
    async fn shutdown(self) {
        crate::runtime::shutdown(self.dead.handle()).await;
        crate::runtime::shutdown(self.served.handle()).await;
    }
}

/// A case's invocation: the command, driven down its failing path, with the success value
/// dropped so that commands with different return types share one shape.
type Invocation<'a> = Pin<Box<dyn Future<Output = Result<(), AppError>> + 'a>>;

/// How a case reaches the harness. Boxed futures have one type per command, so the table holds
/// the function that builds one and the harness arrives when the case runs.
type Drive = for<'a> fn(&'a Harness) -> Invocation<'a>;

/// `build_info` reads the bundle's package metadata, which cannot fail.
///
/// The case drives it against the application whose runtime never started, which is the
/// strongest failure this suite can offer it: a command that reached for runtime state would
/// answer with `INTERNAL` here.
fn build_info_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move { system::build_info(harness.dead_handle()).map(drop) })
}

/// `health` reports the running process's own counters, so a runtime that is not running is
/// the only failure it has.
/// `provider_attribution` reproduces fixed strings, so it has nothing to fail at — and that is the
/// point of it: an installation with no runtime and no configured provider still has to be able to
/// show what this build owes the providers it offers.
fn provider_attribution_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    let _ = harness;
    Box::pin(async move { system::provider_attribution().map(drop) })
}

fn health_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move { system::health(harness.dead_state()).await.map(drop) })
}

/// `log_tail` reads the redacted log through the runtime, so it fails the same way.
fn log_tail_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        system::log_tail(harness.dead_state(), Some(10))
            .await
            .map(drop)
    })
}

/// `start_hello` refuses a step count outside the documented range before it accepts any work.
fn start_hello_with_no_steps(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move { jobs::start_hello(harness.served_state(), 0).await.map(drop) })
}

/// `cancel` parses the id before it looks anything up.
fn cancel_with_a_malformed_id(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        jobs::cancel(harness.served_state(), "not-a-job".to_owned())
            .await
            .map(drop)
    })
}

/// `cancel` of a job that is not running, which is what the second attempt always is.
fn cancel_with_an_unknown_id(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        jobs::cancel(harness.served_state(), UNKNOWN_JOB.to_owned())
            .await
            .map(drop)
    })
}

/// `list_shortcuts` reads a registry that is a constant of this build, and its one failure is a
/// conflict inside that constant — a build defect its own tests catch, not a state any
/// application can be in. Driven against the application whose runtime never started, which is
/// the strongest state this suite can offer it.
fn list_shortcuts_anywhere(_harness: &Harness) -> Invocation<'_> {
    Box::pin(async move { crate::shortcuts::list_shortcuts().map(drop) })
}

/// `autostart_enabled` reads the login item through the adapter the shell registers, and a mock
/// application registers nothing: that is the failure this suite can reach.
fn autostart_enabled_without_the_adapter(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move { crate::autostart::autostart_enabled(harness.dead_handle()).map(drop) })
}

/// `set_autostart` fails the same way when there is no adapter to write through.
fn set_autostart_without_the_adapter(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move { crate::autostart::set_autostart(harness.dead_handle(), true).map(drop) })
}

/// `list` reads the journal through the runtime, so it fails the same way.
fn list_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move { jobs::list(harness.dead_state(), None).await.map(drop) })
}

/// `list_roots` reads the roots through the runtime, so it fails the same way.
fn list_roots_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::library::list_roots(harness.dead_state(), None)
            .await
            .map(drop)
    })
}

/// `add_root` refuses a path that cannot be a root before it writes anything.
///
/// This is the branch a user reaches with a real hand: the shell's folder picker hands the path
/// over as text it has not validated (PG-E1-12), so a relative path is the input the service has to
/// refuse rather than the scanner discovering it later.
fn add_root_with_an_unusable_path(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::library::add_root(
            harness.served_state(),
            "Movies".to_owned(),
            crate::library::RootKind::Movies,
            crate::library::RootScanMode::Incremental,
        )
        .await
        .map(drop)
    })
}

/// `remove_root` reaches the runtime like the other two, and a dead runtime is its only failure
/// that is not a plain "the row was not there".
fn remove_root_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::library::remove_root(harness.dead_state(), crate::library::RootRef { id: 1 })
            .await
            .map(drop)
    })
}

/// `scan_now` reaches the runtime like the root commands, and a dead runtime is the failure this
/// suite can reach without starting a scan it would then have to wait for.
fn scan_now_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::library::scan_now(harness.dead_state())
            .await
            .map(drop)
    })
}

/// `list_titles` reaches the runtime like the other library reads.
fn list_titles_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::library::list_titles(harness.dead_state(), None)
            .await
            .map(drop)
    })
}

/// `list_unmatched` reads the queue through the runtime, so it fails the same way.
fn list_unmatched_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::library::list_unmatched(harness.dead_state(), None)
            .await
            .map(drop)
    })
}

/// `match_unmatched` writes through the runtime, so a dead runtime is the failure it has.
fn match_unmatched_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::library::match_unmatched(
            harness.dead_state(),
            crate::library::UnmatchedRef {
                id: 1,
                as_title: crate::library::MatchAsDto::Parsed,
                title_id: None,
            },
        )
        .await
        .map(drop)
    })
}

/// `list_match_rules` reads through the runtime, so it fails the same way.
fn list_match_rules_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::library::list_match_rules(harness.dead_state(), None)
            .await
            .map(drop)
    })
}

/// `forget_match_rule` writes through the runtime, so a dead runtime is the failure it has.
fn forget_match_rule_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::library::forget_match_rule(
            harness.dead_state(),
            crate::library::MatchRuleRef { id: 1 },
        )
        .await
        .map(drop)
    })
}

/// `start_playback` resolves the title, the file, and the listener through the runtime, so a dead
/// runtime is the failure it has — and the one refusal that matters here is that no listener was
/// opened, which is why the contract asserts the command *fails* rather than what it returns.
fn start_playback_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::library::start_playback(harness.dead_state(), crate::library::TitleRef { id: 1 })
            .await
            .map(drop)
    })
}

/// `stop_playback` needs the runtime too, and reports the same failure rather than `false`: "nothing is
/// playing" and "I could not be asked" are different answers, and a screen that read the second as the
/// first would show an idle player over a session it never closed.
fn stop_playback_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::library::stop_playback(harness.dead_state(), crate::library::SessionRef { id: 0 })
            .await
            .map(drop)
    })
}

/// `start_poster_capture` resolves the title and opens a listener through the runtime, so it fails the
/// same way — and the failure that matters is that no socket was opened.
fn start_poster_capture_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::library::start_poster_capture(
            harness.dead_state(),
            crate::library::TitleRef { id: 1 },
        )
        .await
        .map(drop)
    })
}

/// `stop_poster_capture` answers the runtime's failure rather than `false`, for the reason
/// `stop_playback` does: a backfill that read "I could not be asked" as "there was nothing open" would
/// leave a listener behind and report that it had cleaned up.
fn stop_poster_capture_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::library::stop_poster_capture(
            harness.dead_state(),
            crate::library::SessionRef { id: 0 },
        )
        .await
        .map(drop)
    })
}

/// `get_title` reads through the runtime, so it fails the same way.
fn get_title_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::library::get_title(harness.dead_state(), crate::library::TitleRef { id: 1 })
            .await
            .map(drop)
    })
}

/// `list_episodes` reads through the runtime, so it fails the same way.
fn list_episodes_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::library::list_episodes(
            harness.dead_state(),
            crate::library::TitleRef { id: 1 },
            None,
            None,
        )
        .await
        .map(drop)
    })
}

/// `list_seasons` reads through the runtime, so it fails the same way.
fn list_seasons_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::library::list_seasons(
            harness.dead_state(),
            crate::library::TitleRef { id: 1 },
            None,
        )
        .await
        .map(drop)
    })
}

/// `search_titles` reads through the runtime, so it fails the same way.
fn search_providers_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::discover::search_providers(harness.dead_state(), "alien".to_owned(), None, None)
            .await
            .map(drop)
    })
}
fn refresh_title_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::library::refresh_title(
            harness.dead_state(),
            crate::library::TitleRef { id: 1 },
            crate::library::RootKind::Movies,
        )
        .await
        .map(drop)
    })
}

/// `home_rails` reads through the runtime and touches nothing else, so a runtime that never
/// started is the only failure a warm read has — which is the point of the command: it is a cache
/// read, and a read that could fail on a provider would be a screen that could not paint. A cold
/// read has one failure more, the journal the fetch is queued in, and that is still not a provider.
fn home_rails_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::discover::home_rails(harness.dead_state(), None)
            .await
            .map(drop)
    })
}

/// `refresh_rails` journals a row and submits a job, both of which need a running runtime.
fn refresh_rails_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::discover::refresh_rails(harness.dead_state())
            .await
            .map(drop)
    })
}

/// `browse` reads through the runtime before it can reach a provider, so a runtime that never
/// started is the failure — and it is the same `Internal` the search read reports, because the
/// two ask the same question of the same core.
fn browse_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::discover::browse(
            harness.dead_state(),
            crate::discover::RailKindDto::Movie,
            1,
            None,
            None,
            None,
        )
        .await
        .map(drop)
    })
}

/// `genres` answers from the runtime's own session cache or asks a provider, both of which need a
/// runtime to exist first.
fn genres_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::discover::genres(harness.dead_state(), crate::discover::RailKindDto::Movie)
            .await
            .map(drop)
    })
}

/// `recommendations` resolves the title through the runtime and then asks a provider, so a runtime
/// that never started fails before either.
fn recommendations_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::discover::recommendations(harness.dead_state(), crate::library::TitleRef { id: 1 })
            .await
            .map(drop)
    })
}

fn search_titles_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::library::search_titles(harness.dead_state(), "alien".to_owned(), None)
            .await
            .map(drop)
    })
}

/// `status` reads the cached provider answer through the runtime, so it fails the same way.
fn providers_status_without_a_runtime(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::providers::status(harness.dead_state())
            .await
            .map(drop)
    })
}

/// `set_key` refuses a provider this build does not know before it writes anything, and the
/// candidate is never built into an adapter, so no request leaves the machine (AGENTS R14).
fn set_key_for_an_unknown_provider(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::providers::set_key(
            harness.served_state(),
            "nobody".to_owned(),
            "a-key".to_owned(),
        )
        .await
        .map(drop)
    })
}

/// `set_key` refuses a key that is empty once trimmed, before the runtime is asked anything.
fn set_key_with_an_empty_key(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::providers::set_key(harness.served_state(), "tmdb".to_owned(), "   ".to_owned())
            .await
            .map(drop)
    })
}

/// `test_connection` names the provider to the runtime, and a name this build does not know is
/// refused there.
fn test_connection_for_an_unknown_provider(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::providers::test_connection(harness.served_state(), "nobody".to_owned())
            .await
            .map(drop)
    })
}

/// `clear_key` refuses an unknown provider the same way, before a job is written.
fn clear_key_for_an_unknown_provider(harness: &Harness) -> Invocation<'_> {
    Box::pin(async move {
        crate::providers::clear_key(harness.served_state(), "nobody".to_owned())
            .await
            .map(drop)
    })
}

/// What a case must observe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The invocation must fail with this code, and with the retryability the code carries,
    /// which is what ARCHITECTURE §7 rule 3 promises for it.
    Fails {
        /// The stable code the UI branches on.
        code: ErrorCode,
        /// Whether retrying the same call unchanged could succeed.
        retryable: bool,
    },
    /// The command has no failing path.
    ///
    /// §7 rule 3 still requires its `Result` — the UI handles one response shape — so the case
    /// proves the absence instead of asserting it, by driving the command against the
    /// application whose runtime never started. `reason` records why the exemption is
    /// justified, because a reviewer has to accept it rather than discover it.
    CannotFail {
        /// Why this command cannot fail.
        reason: &'static str,
    },
}

/// One entry in the contract: a command, the failure it must report, and how to reach it.
struct ContractCase {
    /// The wire name the registry registers, which is what `collect_commands!` exports.
    command: &'static str,
    /// What the invocation below must observe.
    expected: Expected,
    /// Drives the command down that path.
    drive: Drive,
}

/// The single source of truth for the IPC contract: every registered command, the failure it
/// must report, and how that failure is reached.
///
/// It is kept in step with `collect_commands!` by `every_registered_command_has_a_case`, which
/// reads the registered names out of the builder's own export. A command with no case here
/// fails that test by name; a case naming a command that is not registered fails it too, so
/// the table cannot keep a dead entry either.
fn cases() -> Vec<ContractCase> {
    let mut all = cases_by_namespace();
    all.extend(provider_cases());
    all.extend(credential_cases());
    all
}

/// The `providers` namespace's cases: the three write commands and the read they start from.
///
/// The unknown-provider and empty-key paths are the two a settings screen can reach with a real
/// hand, and each is refused before a job is written or a request leaves the machine (AGENTS R14).
fn credential_cases() -> Vec<ContractCase> {
    vec![
        ContractCase {
            command: "status",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: providers_status_without_a_runtime,
        },
        ContractCase {
            command: "set_key",
            expected: Expected::Fails {
                code: ErrorCode::InvalidInput,
                retryable: false,
            },
            drive: set_key_for_an_unknown_provider,
        },
        ContractCase {
            command: "set_key",
            expected: Expected::Fails {
                code: ErrorCode::InvalidInput,
                retryable: false,
            },
            drive: set_key_with_an_empty_key,
        },
        ContractCase {
            command: "test_connection",
            expected: Expected::Fails {
                code: ErrorCode::InvalidInput,
                retryable: false,
            },
            drive: test_connection_for_an_unknown_provider,
        },
        ContractCase {
            command: "clear_key",
            expected: Expected::Fails {
                code: ErrorCode::InvalidInput,
                retryable: false,
            },
            drive: clear_key_for_an_unknown_provider,
        },
    ]
}

/// The discover namespace's cases, kept apart so each namespace stays readable as the list grows.
fn provider_cases() -> Vec<ContractCase> {
    vec![
        ContractCase {
            command: "refresh_title",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: refresh_title_without_a_runtime,
        },
        ContractCase {
            command: "search_providers",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: search_providers_without_a_runtime,
        },
        ContractCase {
            command: "home_rails",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: home_rails_without_a_runtime,
        },
        ContractCase {
            command: "refresh_rails",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: refresh_rails_without_a_runtime,
        },
        ContractCase {
            command: "browse",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: browse_without_a_runtime,
        },
        ContractCase {
            command: "genres",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: genres_without_a_runtime,
        },
        ContractCase {
            command: "recommendations",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: recommendations_without_a_runtime,
        },
    ]
}

fn cases_by_namespace() -> Vec<ContractCase> {
    let mut all = shell_cases();
    all.extend(job_cases());
    all.extend(library_cases());
    all.extend(autostart_cases());
    all
}

/// The commands that answer about the process and the shell rather than about work.
fn shell_cases() -> Vec<ContractCase> {
    vec![
        ContractCase {
            command: "provider_attribution",
            expected: Expected::CannotFail {
                reason: "the notices are fixed strings this build ships, not a read",
            },
            drive: provider_attribution_without_a_runtime,
        },
        ContractCase {
            command: "build_info",
            expected: Expected::CannotFail {
                reason: "the bundle's name, version, and target come from infallible lookups",
            },
            drive: build_info_without_a_runtime,
        },
        ContractCase {
            command: "health",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: health_without_a_runtime,
        },
        ContractCase {
            command: "log_tail",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: log_tail_without_a_runtime,
        },
    ]
}

/// The commands that start, watch, and cancel work.
fn job_cases() -> Vec<ContractCase> {
    vec![
        ContractCase {
            command: "start_hello",
            expected: Expected::Fails {
                code: ErrorCode::InvalidInput,
                retryable: false,
            },
            drive: start_hello_with_no_steps,
        },
        ContractCase {
            command: "cancel",
            expected: Expected::Fails {
                code: ErrorCode::InvalidInput,
                retryable: false,
            },
            drive: cancel_with_a_malformed_id,
        },
        ContractCase {
            command: "cancel",
            expected: Expected::Fails {
                code: ErrorCode::NotFound,
                retryable: false,
            },
            drive: cancel_with_an_unknown_id,
        },
        ContractCase {
            command: "list",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: list_without_a_runtime,
        },
    ]
}

/// The commands that manage the library roots.
fn library_cases() -> Vec<ContractCase> {
    vec![
        ContractCase {
            command: "list_roots",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: list_roots_without_a_runtime,
        },
        ContractCase {
            command: "add_root",
            expected: Expected::Fails {
                code: ErrorCode::InvalidInput,
                retryable: false,
            },
            drive: add_root_with_an_unusable_path,
        },
        ContractCase {
            command: "remove_root",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: remove_root_without_a_runtime,
        },
    ]
}

/// The commands the shell's own chrome reaches.
// One case per command, tabulated rather than written as logic: the length is the point, and splitting
// the table would separate a command from the expectation beside it.
#[expect(
    clippy::too_many_lines,
    reason = "an inline table of contract cases, not logic"
)]
fn autostart_cases() -> Vec<ContractCase> {
    vec![
        ContractCase {
            command: "scan_now",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: scan_now_without_a_runtime,
        },
        ContractCase {
            command: "list_titles",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: list_titles_without_a_runtime,
        },
        ContractCase {
            command: "list_unmatched",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: list_unmatched_without_a_runtime,
        },
        ContractCase {
            command: "search_titles",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: search_titles_without_a_runtime,
        },
        ContractCase {
            command: "get_title",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: get_title_without_a_runtime,
        },
        ContractCase {
            command: "list_episodes",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: list_episodes_without_a_runtime,
        },
        ContractCase {
            command: "list_seasons",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: list_seasons_without_a_runtime,
        },
        ContractCase {
            command: "list_match_rules",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: list_match_rules_without_a_runtime,
        },
        ContractCase {
            command: "forget_match_rule",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: forget_match_rule_without_a_runtime,
        },
        ContractCase {
            command: "start_playback",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: start_playback_without_a_runtime,
        },
        ContractCase {
            command: "stop_playback",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: stop_playback_without_a_runtime,
        },
        ContractCase {
            command: "start_poster_capture",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: start_poster_capture_without_a_runtime,
        },
        ContractCase {
            command: "stop_poster_capture",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: stop_poster_capture_without_a_runtime,
        },
        ContractCase {
            command: "match_unmatched",
            expected: Expected::Fails {
                code: ErrorCode::Internal,
                retryable: false,
            },
            drive: match_unmatched_without_a_runtime,
        },
        ContractCase {
            command: "list_shortcuts",
            expected: Expected::CannotFail {
                reason: "the registry is a constant of this build; its conflict check runs in the \
                         crate's own tests, so no application state reaches the failure branch",
            },
            drive: list_shortcuts_anywhere,
        },
        ContractCase {
            command: "autostart_enabled",
            expected: Expected::Fails {
                code: ErrorCode::Unsupported,
                retryable: false,
            },
            drive: autostart_enabled_without_the_adapter,
        },
        ContractCase {
            command: "set_autostart",
            expected: Expected::Fails {
                code: ErrorCode::Unsupported,
                retryable: false,
            },
            drive: set_autostart_without_the_adapter,
        },
    ]
}

/// The TypeScript the builder generates for the whole command surface.
///
/// `collect_commands!` seals the command list inside a `Commands<R>` whose only fields are
/// private to `tauri-specta`, so there is no runtime accessor to ask for it. The export is the
/// next best thing and is not a second copy of the list: it is an artifact the same `builder()`
/// produces, and `cargo xtask gen-ipc` plus its drift gate already depend on it containing
/// exactly the registered commands.
fn exported_bindings() -> String {
    let directory = TempDir::new().expect("a directory for the export");
    let path = directory.path().join("bindings.ts");
    crate::builder()
        .export(specta_typescript::Typescript::default(), &path)
        .expect("the registry must export its bindings");
    std::fs::read_to_string(&path).expect("the exported bindings are readable")
}

/// The wire name the exporter passed to `invoke` on one line of the export.
fn invoked_name(line: &str) -> Option<&str> {
    let start = line.find(INVOKE)?;
    let rest = &line[start + INVOKE.len()..];
    let end = rest.find('"').expect("the invoke name is a quoted string");
    Some(&rest[..end])
}

/// Every command name the registry registers, read out of that export.
fn registered_commands() -> Vec<String> {
    let bindings = exported_bindings();
    let mut names: Vec<String> = bindings
        .lines()
        .filter_map(invoked_name)
        .map(str::to_owned)
        .collect();
    names.sort_unstable();
    names.dedup();
    // An export this parser cannot read would make the coverage test below vacuous, and a
    // vacuous test is worse than none, so an unreadable export is its own failure.
    assert!(
        !names.is_empty(),
        "no command was found in the exported bindings (looking for `{INVOKE}`): tauri-specta's \
         export format changed and this parser has to change with it"
    );
    names
}

/// The registered commands whose exported answer is a job handle.
///
/// ARCHITECTURE §7 rule 2 makes a job handle the mark of job-scoped work, and `JobStarted` is
/// the DTO for one. The set is read from the registry's own export rather than listed here, so
/// a job-scoped command joins it without anyone remembering to say so — and the check below
/// fails rather than passing if the export stops being readable this way.
fn job_scoped_commands() -> Vec<String> {
    let bindings = exported_bindings();
    let mut names: Vec<String> = bindings
        .lines()
        .filter(|line| line.contains(&format!("<{JOB_HANDLE},")))
        .filter_map(invoked_name)
        .map(str::to_owned)
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// PG-E2-10's acceptance: every command the registry registers has a case, and every case
/// names a command the registry registers.
///
/// This is the test that fails when a seventh command is added to `collect_commands!` without
/// its failing path being written down. The registry is read, not restated, so it cannot pass
/// by comparing a hand-written list with itself.
#[test]
fn every_registered_command_has_a_case() {
    let registered = registered_commands();
    let declared = cases();

    for command in &registered {
        assert!(
            declared.iter().any(|case| case.command == command),
            "`{command}` is registered in `collect_commands!` but has no contract case: add it \
             to `cases()` with the failure it must report (PG-E2-10, ARCHITECTURE §7 rule 3)"
        );
    }
    for case in &declared {
        assert!(
            registered.iter().any(|command| command == case.command),
            "the contract case for `{}` names a command the registry does not register: remove \
             it, or register the command (PG-E2-10)",
            case.command
        );
    }
}

/// Every case drives its command through a mock application and finds the failure
/// ARCHITECTURE §7 rule 3 promises: a stable code, the message catalogue key that code
/// carries, and a retryability that cannot disagree with it.
#[tokio::test]
async fn every_case_drives_its_command_to_the_declared_failure() {
    let directory = TempDir::new().unwrap();
    let harness = Harness::new(&directory).await;

    for case in cases() {
        let outcome = (case.drive)(&harness).await;
        match case.expected {
            Expected::Fails { code, retryable } => {
                let error = match outcome {
                    Ok(()) => panic!(
                        "`{}` must fail on the path this case drives it down",
                        case.command
                    ),
                    Err(error) => error,
                };
                assert_eq!(error.code, code, "`{}`: {}", case.command, error.message);
                assert_eq!(
                    error.retryable, retryable,
                    "`{}`: {}",
                    case.command, error.message
                );
                assert_eq!(
                    error.retryable,
                    code.metadata().retryable,
                    "`{}` reports a retryability its code does not carry",
                    case.command
                );
                assert_eq!(
                    error.message,
                    code.metadata().message,
                    "`{}` reports a message its code does not carry",
                    case.command
                );
                assert!(
                    error.message_key().starts_with("error."),
                    "`{}` produced a message key outside the catalogue: {}",
                    case.command,
                    error.message_key()
                );
            }
            Expected::CannotFail { reason } => {
                assert!(
                    !reason.trim().is_empty(),
                    "`{}` is exempted from the failing path without saying why",
                    case.command
                );
                if let Err(error) = outcome {
                    panic!(
                        "`{}` is declared unable to fail, but this path failed: {error:?}",
                        case.command
                    );
                }
            }
        }
    }

    harness.shutdown().await;
}

/// The two payload rules a command can break: an error's `detail` and `cause` carry no
/// absolute path (ARCHITECTURE §7 rule 3), and no response is over the 64 KB budget (rule 1).
///
/// The budget is measured on the failing payload here. The successful payloads are the
/// commands' own to bound, and the tests that cover how they do it already exist beside them:
/// the guard's boundary test in [`crate::guard`], `list`'s page cap, and `log_tail`'s read
/// window.
#[tokio::test]
async fn every_failing_response_is_small_and_path_free() {
    let directory = TempDir::new().unwrap();
    let harness = Harness::new(&directory).await;

    let failing: Vec<ContractCase> = cases()
        .into_iter()
        .filter(|case| matches!(case.expected, Expected::Fails { .. }))
        .collect();
    assert!(
        !failing.is_empty(),
        "no case drives a failure, so this test would pass without looking at anything"
    );

    for case in failing {
        let error = (case.drive)(&harness)
            .await
            .expect_err("the failing cases all fail");
        let what = format!("`{}`", case.command);
        for text in [error.detail.as_deref(), error.cause.as_deref()]
            .into_iter()
            .flatten()
        {
            crate::redaction::assert_no_path(&what, text);
        }
        let json = serde_json::to_string(&error).expect("an error serializes");
        assert!(
            json.len() <= MAX_RESPONSE_BYTES,
            "`{}` answered with {} bytes, over the {MAX_RESPONSE_BYTES} byte budget in \
             ARCHITECTURE §7 rule 1",
            case.command,
            json.len()
        );
    }

    harness.shutdown().await;
}

/// ARCHITECTURE §7 rule 6: every job-scoped command has a `cancel` counterpart.
///
/// Both sides come from the registry's own export — the job-scoped set from the answer each
/// command declares, the counterpart from the registered names — so this cannot pass by
/// comparing a list with itself, and the failure names the command that owes a cancel. The
/// counterpart is asserted against the registry rather than against a namespace because a wire
/// name carries none: `jobs::start_hello` is registered as plain `start_hello`.
#[test]
fn every_job_scoped_command_has_a_cancel_counterpart() {
    let job_scoped = job_scoped_commands();
    assert!(
        !job_scoped.is_empty(),
        "no job-scoped command was found in the export (looking for an answer of `{JOB_HANDLE}`): \
         either the surface lost its job commands or this parser has to change"
    );

    let registered = registered_commands();
    for command in &job_scoped {
        assert!(
            registered.iter().any(|name| name == "cancel"),
            "`{command}` answers with a job handle, so it is job-scoped and the registry owes \
             it a `cancel` counterpart (ARCHITECTURE §7 rule 6), but none is registered"
        );
    }
}
