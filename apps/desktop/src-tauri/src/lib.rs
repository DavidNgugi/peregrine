#![forbid(unsafe_code)]
//! The Tauri application shell: it starts the runtime, registers the command surface,
//! creates the main window, and runs the event loop, and does nothing else.
//!
//! Business logic belongs in `peregrine-app` and reaches the frontend over the
//! IPC contract (docs/ARCHITECTURE.md §7). The command surface itself is owned by
//! `peregrine-ipc`, which is also what `cargo xtask gen-ipc` exports the
//! TypeScript bindings from, so the shell and the generated client can never
//! register different commands.

use tauri::Manager as _;

/// Why the desktop shell could not start or had to stop.
#[derive(Debug, thiserror::Error)]
pub enum ShellError {
    /// The Tauri runtime failed to initialise or exited with an error.
    #[error("the desktop shell could not start: {0}")]
    Runtime(#[from] tauri::Error),
}

/// Starts the runtime and blocks until the last window closes.
///
/// # Errors
///
/// Returns [`ShellError::Runtime`] when the runtime cannot initialise.
pub fn run() -> Result<(), ShellError> {
    // One builder instance: Tauri's invoke handler and the event mount both read
    // from it, so a command cannot be invokable without also being exported.
    let ipc = peregrine_ipc::builder();
    let app = tauri::Builder::default()
        .invoke_handler(ipc.invoke_handler())
        .setup(move |app| {
            ipc.mount_events(app);
            start_runtime(app);
            Ok(())
        })
        .build(tauri::generate_context!())
        .map_err(ShellError::Runtime)?;

    app.run(|app, event| {
        if let tauri::RunEvent::Exit = event {
            // The runtime drains here rather than in `Drop`: the journal rows of live jobs
            // have to be closed and the queued log lines flushed before the process goes
            // away, and neither happens if the process exits with destructors unrun.
            tauri::async_runtime::block_on(peregrine_ipc::runtime::shutdown(app));
        }
    });
    Ok(())
}

/// Starts the application runtime, reporting a failure without crashing the event loop.
///
/// `setup` cannot report a failure without Tauri panicking, so the failure is written to
/// stderr (logging may not be installed yet) and recorded in the managed state: every
/// command then answers with a structured error. That is worth more than a stack trace,
/// because the user can read what went wrong and the window still closes cleanly.
fn start_runtime(app: &mut tauri::App) {
    let directory = match app.path().app_data_dir() {
        Ok(directory) => directory,
        Err(error) => {
            eprintln!("peregrine: cannot resolve the data directory: {error}");
            tracing::error!(%error, "cannot resolve the data directory");
            return;
        }
    };

    if let Err(error) = peregrine_ipc::runtime::serve(app.handle(), directory) {
        eprintln!("peregrine: {error}");
        tracing::error!(%error, "the application runtime did not start");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_error_says_the_shell_failed_to_start() {
        let error = ShellError::Runtime(tauri::Error::AssetNotFound("index.html".to_owned()));
        assert!(
            error
                .to_string()
                .starts_with("the desktop shell could not start:"),
            "unexpected message: {error}"
        );
    }
}
