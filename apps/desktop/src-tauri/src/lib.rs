#![forbid(unsafe_code)]
//! The Tauri application shell: it registers the command surface, creates the
//! main window, and runs the event loop, and does nothing else.
//!
//! Business logic belongs in `peregrine-app` and reaches the frontend over the
//! IPC contract (docs/ARCHITECTURE.md §7). The command surface itself is owned by
//! `peregrine-ipc`, which is also what `cargo xtask gen-ipc` exports the
//! TypeScript bindings from, so the shell and the generated client can never
//! register different commands.

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
    tracing::info!(target: "peregrine_desktop", "starting the desktop shell");
    // One builder instance: Tauri's invoke handler and the event mount both read
    // from it, so a command cannot be invokable without also being exported.
    let ipc = peregrine_ipc::builder();
    tauri::Builder::default()
        .invoke_handler(ipc.invoke_handler())
        .setup(move |app| {
            ipc.mount_events(app);
            Ok(())
        })
        .run(tauri::generate_context!())
        .map_err(ShellError::Runtime)
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
