//! Serving the runtime over Tauri: the managed state, the event sink, and the shutdown.
//!
//! The runtime itself is `peregrine-app`'s; this module is the only place that knows it is
//! driven by a Tauri application. Two things are wired here and nowhere else:
//!
//! * the event sink, which turns a bus delivery into `window.emit` of the topic name, and
//! * the process ordering, so the log subscriber is installed before the runtime starts
//!   and the runtime is drained before the process exits.
//!
//! A startup failure is recorded rather than thrown at the event loop: Tauri panics when a
//! `setup` hook returns an error, and a panic is not a report. The state carries why the
//! runtime is missing, and every command answers with a structured error instead of
//! reaching for state that was never installed.

use std::path::PathBuf;
use std::sync::Arc;

use peregrine_app::events::{Event, EventSink, EventSinkError};
use peregrine_app::{Observability, ObservabilityError, Runtime, RuntimeConfig, RuntimeError};
use serde_json::json;
use tauri::{AppHandle, Emitter as _, Manager as _};

use crate::error::{AppError, ErrorCode};

/// The application state every command reaches through.
///
/// Holds either the running application or the reason it is not running, because the shell
/// starts it from `setup` and a command can be invoked before that succeeded.
pub struct RuntimeState {
    runtime: Option<Arc<Runtime>>,
    /// The LAN surface over the same runtime (ADR 0028), built with it and never listening until
    /// a command starts it.
    web: Option<Arc<peregrine_app::WebService>>,
    /// Where the built TV client lives, when this installation has one.
    web_app_dir: Option<PathBuf>,
    /// Why there is no runtime, in words that are safe to show (`ARCHITECTURE` §7 rule 3).
    unavailable: Option<&'static str>,
}

impl RuntimeState {
    /// The running application.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::Internal`] when startup failed, with the kind of failure in `cause`
    /// and no filesystem path in either field.
    pub fn runtime(&self) -> Result<&Runtime, AppError> {
        self.runtime.as_deref().ok_or_else(|| {
            AppError::new(ErrorCode::Internal)
                .with_detail("Peregrine did not finish starting, so it cannot do that.")
                .with_cause(
                    self.unavailable
                        .unwrap_or("the application runtime is not running"),
                )
        })
    }

    /// The LAN surface, when the runtime started.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::Internal`] when startup failed, for the same reason as [`Self::runtime`].
    pub fn web(&self) -> Result<&Arc<peregrine_app::WebService>, AppError> {
        self.web.as_ref().ok_or_else(|| {
            AppError::new(ErrorCode::Internal)
                .with_detail(
                    "Peregrine did not finish starting, so the LAN surface is unavailable.",
                )
                .with_cause(
                    self.unavailable
                        .unwrap_or("the application runtime is not running"),
                )
        })
    }

    /// Where the built client lives, when this installation has one.
    pub fn web_app_dir(&self) -> Option<&PathBuf> {
        self.web_app_dir.as_ref()
    }

    /// Wraps a runtime for the tests in this crate, which build one directly rather than
    /// through [`serve`] so that none of them installs a process-wide log subscriber.
    #[cfg(test)]
    pub(crate) fn for_test(runtime: Arc<Runtime>) -> Self {
        let web = Some(Arc::new(runtime.web_service()));
        Self {
            runtime: Some(runtime),
            web,
            web_app_dir: None,
            unavailable: None,
        }
    }

    /// A state with a runtime that never started.
    #[cfg(test)]
    pub(crate) fn failed(reason: &'static str) -> Self {
        Self {
            runtime: None,
            web: None,
            web_app_dir: None,
            unavailable: Some(reason),
        }
    }
}

/// Starts the runtime and installs it as the application's managed state.
///
/// Called once from the shell's `setup`. The order is the design: the log subscriber is
/// installed first, because every later failure has to be reportable, and the runtime is
/// started second, so a failed database migration stops the app before a window is usable.
///
/// `artwork_directory` is the directory the shell scoped the asset protocol to, passed through so
/// that the image cache writes where the webview can read (PG-E2-08). It is a parameter rather than
/// a derivation because the shell is the only layer that knows what the protocol serves.
///
/// # Errors
///
/// [`StartupError::Logging`] when the subscriber cannot be installed — another one already
/// exists, or the log directory cannot be opened — and [`StartupError::Runtime`] when the
/// data directory or the database cannot be opened.
pub fn serve<R: tauri::Runtime>(
    app: &AppHandle<R>,
    data_directory: PathBuf,
    artwork_directory: PathBuf,
) -> Result<(), StartupError> {
    let outcome = start(app, data_directory, artwork_directory);
    let web_app_dir = web_client_dir(app);
    let state = match &outcome {
        Ok(runtime) => RuntimeState {
            runtime: Some(Arc::clone(runtime)),
            web: Some(Arc::new(runtime.web_service())),
            web_app_dir,
            unavailable: None,
        },
        Err(error) => RuntimeState {
            runtime: None,
            web: None,
            web_app_dir,
            unavailable: Some(reason(error)),
        },
    };
    // The listener follows the machine: a Wi-Fi switch assigns a new address while the old socket
    // stays bound to the one it was opened on, so the surface must be moved rather than restarted.
    let watched = state.web.clone();
    app.manage(state);
    if let Some(web) = watched {
        watch_lan_address(app.clone(), web);
    }
    // The two periodic pieces of core work: schedules whose date has arrived, and scanned titles
    // waiting to be matched to a provider. One thread, a minute apart, and one pass immediately so
    // work that came due while the app was closed is taken on the next start rather than later.
    if let Ok(runtime) = &outcome {
        // A dedicated thread rather than a task: both passes are short blocking reads, and this
        // crate does not otherwise depend on the async runtime's timer surface.
        let runtime = Arc::clone(runtime);
        std::thread::spawn(move || {
            loop {
                if let Err(error) = tauri::async_runtime::block_on(runtime.run_due_schedules()) {
                    tracing::warn!(%error, "scheduled downloads could not run");
                }
                if let Err(error) = tauri::async_runtime::block_on(runtime.match_stub_titles()) {
                    tracing::warn!(%error, "scanned titles could not be queued for matching");
                }
                std::thread::sleep(std::time::Duration::from_secs(60));
            }
        });
    }
    outcome.map(|_| ())
}

/// Moves the LAN surface when the address it is bound to is no longer the machine's.
///
/// Five seconds is short enough that a Wi-Fi switch is not something a person has to notice, and
/// the probe is a route lookup plus a bind attempt rather than a packet. A listener that is not
/// running, or that is still bound to an address this machine still owns, is left alone: the
/// watcher exists for the address that vanished, not to second-guess an interface the person chose.
fn watch_lan_address<R: tauri::Runtime>(app: AppHandle<R>, web: Arc<peregrine_app::WebService>) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            let status = tauri::async_runtime::block_on(web.status());
            let Some(bound) = status.address else {
                continue;
            };
            if crate::web::address_is_local(bound.ip()) {
                continue;
            }
            let Some(detected) = crate::web::detected_lan_address() else {
                continue;
            };
            match tauri::async_runtime::block_on(web.rebind(detected)) {
                Ok(status) => {
                    let payload = crate::web::status_dto(&status);
                    if let Err(error) = app.emit(crate::web::WEB_STATUS_EVENT, payload) {
                        tracing::warn!(%error, "the moved LAN address could not reach the window");
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "the LAN surface could not move to the new address");
                }
            }
        }
    });
}

/// The mpv binary shipped inside the bundle, when this build has one.
///
/// A bundled player is the reason the installer does not ask for mpv; `PATH` remains the fallback
/// for a development build, where no sidecar has been staged.
fn bundled_mpv<R: tauri::Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    let directory = app.path().resource_dir().ok()?;
    let sidecars = directory.join("sidecars");
    #[cfg(target_os = "macos")]
    let binary = sidecars.join("mpv.app/Contents/MacOS/mpv");
    #[cfg(target_os = "windows")]
    let binary = sidecars.join("mpv").join("mpv.exe");
    #[cfg(all(unix, not(target_os = "macos")))]
    let binary = sidecars.join("mpv");
    binary.is_file().then_some(binary)
}

/// Installs logging and starts the runtime.
fn start<R: tauri::Runtime>(
    app: &AppHandle<R>,
    data_directory: PathBuf,
    artwork_directory: PathBuf,
) -> Result<Arc<Runtime>, StartupError> {
    // A debug build keeps provider secrets in a 0600 file under the app's data directory: macOS
    // binds a keychain item to the binary's code signature, and a dev rebuild is a new signature,
    // so three providers meant three login-password prompts on every launch. The store migrates a
    // value from the keyring once, then answers from the file; release builds keep the keyring.
    #[cfg(debug_assertions)]
    let dev_secrets = data_directory.join("secrets.dev.json");
    let config = RuntimeConfig::new(
        data_directory,
        artwork_directory,
        Arc::new(TauriSink { app: app.clone() }),
    );
    let config = match bundled_mpv(app) {
        Some(binary) => config.with_player_binary(binary),
        None => config,
    };
    #[cfg(debug_assertions)]
    let config = config.with_secrets(Arc::new(peregrine_app::DevFileStore::new(dev_secrets)));
    let logging = Observability::install(&config.log_config())?;
    let runtime = tauri::async_runtime::block_on(Runtime::start(config, logging))?;
    Ok(Arc::new(runtime))
}

/// Why the runtime is missing, in words that carry no path and no secret.
fn reason(error: &StartupError) -> &'static str {
    match error {
        StartupError::Logging(_) => "logging could not be started",
        StartupError::Runtime(RuntimeError::DataDirectory { .. }) => {
            "the data folder could not be created"
        }
        StartupError::Runtime(RuntimeError::Storage(_)) => {
            "the library database could not be opened"
        }
        StartupError::Runtime(RuntimeError::Scheduler(_)) => "the job queue could not be started",
        StartupError::Runtime(_) => "the application runtime could not be started",
    }
}

/// Drains the runtime.
///
/// Called from the shell's exit handler. Nothing here waits for a job that ignores its
/// cancellation token, because the scheduler does not either; what this guarantees is that
/// the journal rows of live jobs are closed, the event bus has flushed, and the log writer
/// has stopped before the process goes away.
pub async fn shutdown<R: tauri::Runtime>(app: &AppHandle<R>) {
    let Some(state) = app.try_state::<RuntimeState>() else {
        // Nothing was ever installed, so there is nothing to drain and no second failure
        // to report on the way out.
        return;
    };
    if let Ok(web) = state.web() {
        web.stop().await;
    }
    if let Ok(runtime) = state.runtime() {
        runtime.shutdown().await;
    }
}

/// Where the built TV client lives, when this installation has one.
///
/// A directory is not a bundle: the first slice serves whatever is at one of three places rather
/// than embedding the assets, which is a packaging task (ADR 0028 §6). The environment variable is
/// the development path, `web/` beside the binary is the packaged path, and the checkout's
/// `apps/web/dist` is the same-machine path a debug build uses so the TV can be opened while the
/// client is iterated on.
fn web_client_dir<R: tauri::Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("PEREGRINE_WEB_DIR") {
        let path = PathBuf::from(explicit);
        if path.join("index.html").is_file() {
            return Some(path);
        }
    }
    if let Ok(resources) = app.path().resource_dir() {
        let path = resources.join("web");
        if path.join("index.html").is_file() {
            return Some(path);
        }
    }
    #[cfg(debug_assertions)]
    {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/web/dist");
        if path.join("index.html").is_file() {
            return Some(path);
        }
    }
    None
}

/// The counts the shell's tray presents, read from the running runtime (PG-E1-09).
///
/// `None` when the runtime never started. The tray then shows no numbers rather than
/// zeros, because "no runtime" and "nothing running" are different claims and only one of
/// them is true.
pub async fn shell_status<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Option<peregrine_app::ShellStatus> {
    let state = app.try_state::<RuntimeState>()?;
    let runtime = state.runtime().ok()?;
    Some(runtime.shell_status().await)
}

/// Delivers a bus event to the webview.
struct TauriSink<R: tauri::Runtime> {
    app: AppHandle<R>,
}

impl<R: tauri::Runtime> EventSink for TauriSink<R> {
    fn emit(&self, event: &Event) -> Result<(), EventSinkError> {
        // The topic name is the event name and the version travels in the body, so a
        // listener never has to parse the name to learn the payload's shape.
        let payload = json!({
            "topic": event.topic.name(),
            "schemaVersion": event.topic.version(),
            "payload": event.payload,
        });
        self.app
            .emit(event.topic.name(), payload)
            .map_err(|error| EventSinkError::new(error.to_string()))
    }
}

/// Why the command surface could not start.
#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    /// The log subscriber could not be installed.
    #[error("logging could not be started: {0}")]
    Logging(#[from] ObservabilityError),

    /// The runtime could not be started.
    #[error("{0}")]
    Runtime(#[from] RuntimeError),
}
