#![forbid(unsafe_code)]
//! The Tauri application shell: it creates the main window and runs the event
//! loop, and does nothing else.
//!
//! Business logic belongs in `peregrine-app` and reaches the frontend over the
//! IPC contract (docs/ARCHITECTURE.md §7). The crate depends on no internal
//! crate, so there is no path by which a rule can drift in here.

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
    tauri::Builder::default()
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
