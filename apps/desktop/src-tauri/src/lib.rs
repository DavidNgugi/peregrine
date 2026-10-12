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
        // The window is created `visible: false` and this plugin is what shows it, so the
        // saved size and position are in place before the first frame: the window never
        // appears at the config's default geometry and then jumps to the restored one.
        // Restoring, tracking, and saving geometry are the plugin's job and not this
        // shell's (docs/ARCHITECTURE.md §6 "State ownership").
        .plugin(tauri_plugin_window_state::Builder::default().build())
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

    /// The window contract the React chrome is written against, asserted against the
    /// config the runtime was built with rather than against the file on disk.
    ///
    /// Three of these are load-bearing and easy to delete by accident: `visible: false`
    /// is what lets `tauri-plugin-window-state` show the window after restoring its
    /// geometry; `decorations: true` is what keeps the Windows Snap Layouts, the system
    /// menu, and the Linux desktop's own window controls, which is why no platform has a
    /// web-drawn caption button (`docs/DESIGN_SYSTEM.md` §8, docs/ARCHITECTURE.md §17); and
    /// the overlay title bar with `hiddenTitle` is what makes the macOS chrome a title bar
    /// the app draws into.
    ///
    /// The three overlay fields are macOS-only by implementation — Tauri applies them
    /// under `cfg(target_os = "macos")` — so they can live in the shared config instead of
    /// in a `tauri.macos.conf.json` that would have to restate the whole window entry,
    /// because a platform config is merged as a JSON merge patch and replaces arrays.
    #[test]
    fn main_window_declares_restorable_overlay_chrome() {
        // The runtime has to be named here: the macro expands to a generic function, and
        // outside `run` there is no builder to infer the runtime from. `test = true` skips
        // the macOS Info.plist embed, which a second expansion inside one binary cannot do;
        // the config is still read and embedded, which is what the assertions below read.
        let context: tauri::Context<tauri::Wry> = tauri::generate_context!(test = true);
        let windows = &context.config().app.windows;
        assert_eq!(windows.len(), 1, "the shell creates exactly one window");

        let window = windows.first().expect("asserted exactly one window above");
        assert_eq!(window.label, "main");
        assert!(
            !window.visible,
            "the window-state plugin shows the window, after restoring its geometry"
        );
        assert!(
            window.decorations,
            "the OS keeps its frame, its caption buttons, and its Snap Layouts"
        );
        assert_eq!(window.title_bar_style, tauri::utils::TitleBarStyle::Overlay);
        assert!(window.hidden_title, "the chrome renders the app name");
        assert_eq!(
            window.traffic_light_position,
            Some(tauri::utils::config::LogicalPosition { x: 16.0, y: 18.0 }),
            "traffic lights are placed in logical points, so 125% and 200% agree"
        );
        assert!(
            window.width >= window.min_width.unwrap_or_default(),
            "the window opens at or above its own minimum"
        );
        assert!(
            window.height >= window.min_height.unwrap_or_default(),
            "the window opens at or above its own minimum"
        );
    }
}
