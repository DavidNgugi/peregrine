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

mod external;
mod media;
mod menu;
mod tray;

/// Why the desktop shell could not start or had to stop.
#[derive(Debug, thiserror::Error)]
pub enum ShellError {
    /// The Tauri runtime failed to initialise or exited with an error.
    #[error("the desktop shell could not start: {0}")]
    Runtime(#[from] tauri::Error),
    /// The OS refused a menu, a submenu, or one of their items.
    #[error("the application menu could not be installed: {0}")]
    Menu(tauri::Error),
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
        // Registered before anything else, because the plugin's job begins and ends
        // before `setup`: a second launch hands its arguments to the first instance and
        // exits without ever reaching the event loop (PG-E1-04).
        .plugin(single_instance())
        // The window is created `visible: false` and this plugin is what shows it, so the
        // saved size and position are in place before the first frame: the window never
        // appears at the config's default geometry and then jumps to the restored one.
        // Restoring, tracking, and saving geometry are the plugin's job and not this
        // shell's (docs/ARCHITECTURE.md §6 "State ownership").
        .plugin(tauri_plugin_window_state::Builder::default().build())
        // The pickers are called from the webview, so the plugin answers on the frontend's
        // behalf and the ACL is what decides whether it may (PG-E1-12).
        .plugin(tauri_plugin_dialog::init())
        // The `peregrine://` scheme (PG-E1-11): the plugin is what the OS is told about at
        // bundle time, and its `on_open_url` is the one delivery path that never appears in
        // this process's argument vector — macOS delivers a URL as an Apple Event. The
        // single-instance plugin's own `deep-link` feature is deliberately *not* enabled:
        // it would feed the same argument vector into this plugin a second time, so one
        // deep link would be routed twice.
        .plugin(tauri_plugin_deep_link::init())
        // The login item (PG-E1-13). `LaunchAgent` is the launcher macOS uses for a
        // non-App-Store build, and registering the plugin creates no login item: autostart
        // stays off until the settings toggle asks for it.
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .invoke_handler(ipc.invoke_handler())
        // The document loads after `setup` has run, and an event emitted into a webview
        // with no listener is gone. This is where a link that arrived on a cold start is
        // released (PG-E1-11).
        .on_page_load(|webview, _payload| {
            if let Some(delivery) = webview.try_state::<external::Delivery>() {
                delivery.flush(webview.app_handle());
            }
        })
        .setup(move |app| {
            ipc.mount_events(app);
            start_runtime(app);
            // Installed before anything can be routed: the single-instance callback and the
            // deep-link listener both run on the event loop, and this is the state that
            // holds a link until the window can receive it.
            app.manage(external::Delivery::default());
            // A menu the user cannot use is worse than no menu, so a failure to install
            // one is reported and the app keeps running with the platform default.
            if let Err(error) = menu::install(app.handle()) {
                eprintln!("peregrine: {error}");
                tracing::error!(%error, "the application menu was not installed");
            }
            // The registry must not claim an accelerator an application menu item already
            // owns: the menu would dispatch one of the two, and which one is the platform's
            // business rather than ours (PG-E1-08). Checked here, where both exist, as well
            // as by the command that draws the Settings list.
            if let Err(conflict) = peregrine_ipc::shortcuts::validate_registry() {
                tracing::error!(
                    %conflict,
                    "the shortcut registry conflicts with the application menu"
                );
            }
            route_own_arguments(app);
            listen_for_opened_urls(app);
            start_tray(app);
            // Before the document loads: an `<img>` pointing at a cached image is answered
            // by the asset protocol, and the scope has to be filled before the first one is
            // requested. A failure is reported and not fatal — images stay at their
            // placeholder, and no other surface depends on this (PG-E2-08).
            if let Err(error) = media::install(app.handle()) {
                eprintln!("peregrine: {error}");
                tracing::warn!(
                    %error,
                    "the asset protocol was not scoped, so cached images will not load"
                );
            }
            // After the window-state plugin has restored the geometry, which is what can
            // leave the window hidden; see the function.
            ensure_main_window_visible(app.handle());
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

/// Single-instance enforcement: a second launch raises the window and forwards its
/// arguments, and then exits without starting a runtime of its own.
fn single_instance() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri_plugin_single_instance::init(|app, arguments, working_directory| {
        let input = external::ExternalInput::new(arguments, working_directory);
        tracing::debug!(
            argc = input.arguments.len(),
            "a second launch reached the running instance (PG-E1-04)"
        );
        focus_main_window(app);
        external::forward(app, &input);
    })
}

/// Routes a deep link this process was *launched* with.
///
/// The single-instance plugin only sees a second launch; the first launch's own arguments
/// reach no plugin at all, which is the half of "cold start and warm start both route"
/// that PG-E1-04 deliberately left to PG-E1-11. The values stay inside
/// [`external::forward`], which logs a route and never the text (SECURITY.md T13).
fn route_own_arguments(app: &mut tauri::App) {
    let arguments: Vec<String> = std::env::args_os()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    // A relative path argument would be relative to this directory, and the runtime's own
    // working directory is not it once a handler runs; the router carries it alongside.
    let working_directory = std::env::current_dir()
        .map(|directory| directory.to_string_lossy().into_owned())
        .unwrap_or_default();
    external::forward(
        app.handle(),
        &external::ExternalInput::new(arguments, working_directory),
    );
}

/// Routes a `peregrine://` URL the OS hands to the running application.
///
/// macOS reaches this path, and it is the only one it reaches: Launch Services opens the
/// already-running application and delivers an Apple Event, so the URL is never in any
/// process's argument vector.
fn listen_for_opened_urls(app: &mut tauri::App) {
    use tauri_plugin_deep_link::DeepLinkExt as _;

    let handle = app.handle().clone();
    app.deep_link().on_open_url(move |event| {
        let urls: Vec<String> = event
            .urls()
            .into_iter()
            .map(|url| url.to_string())
            .collect();
        external::forward_urls(&handle, &urls);
    });
}

/// Creates the tray and keeps its counts current (PG-E1-09).
///
/// The first state is read synchronously so the tray exists before the event loop turns;
/// a runtime that has not started yet is not an error, and the refresh loop creates the
/// tray on the first answer that arrives.
fn start_tray(app: &mut tauri::App) {
    if let Some(status) =
        tauri::async_runtime::block_on(peregrine_ipc::runtime::shell_status(app.handle()))
    {
        if let Err(error) = tray::install(app.handle(), &status) {
            tracing::warn!(%error, "the tray could not be created");
        }
    } else {
        tracing::debug!("the tray waits for the runtime's first counters");
    }
    tray::spawn_refresh(app.handle().clone());
}

/// Raises and focuses the main window, which is the part of a second launch the user sees.
///
/// Unconditional rather than `if is_minimized()`: asking costs a round trip to the event
/// loop, and this callback already runs on it, so the question would have to be asked from
/// another thread and the answer would be stale by the time it arrived. Unminimizing a
/// window that is not minimized is a no-op in the platform.
///
/// A failure is logged and not returned: the user launched the app to use the window that
/// is already open, and a focus failure must not turn into a second instance or a crash.
fn focus_main_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let Some(window) = app.get_webview_window("main") else {
        // A second launch that arrives while the first is shutting its window down. There
        // is nothing to focus and nothing to report to a user who is already exiting.
        tracing::debug!("a second launch found no main window to focus");
        return;
    };
    if let Err(error) = window.unminimize().and_then(|()| window.set_focus()) {
        tracing::warn!(%error, "the main window could not be raised for a second launch");
    }
}

/// Shows the main window when the restored window state says it is hidden.
///
/// `tauri-plugin-window-state` 2.5.0 saves `visible` under `StateFlags::all()` and calls
/// `show()` in `restore_state` only when the restored value is `true` (its line
/// `if flags.contains(StateFlags::VISIBLE) && should_show`). So a state file that says
/// `visible: false` — which any action that hides the window writes, and which
/// `docs/ARCHITECTURE.md` §17 warns about for the tray — makes the next launch restore the
/// geometry and never appear. Removing the VISIBLE flag is not a fix: the plugin would then
/// stop calling `show()` entirely, and the window would never appear at all.
///
/// So the shell owns the guarantee instead. This runs in `setup`, after the plugin has
/// restored, and is a no-op when the window is already up.
///
/// One consequence to keep in mind for later work: a "start hidden, minimized to the tray"
/// preference cannot be expressed while this stands. That behaviour is close-to-tray
/// (PG-E1-10), and it will have to replace this check rather than live beside it.
fn ensure_main_window_visible<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    match window.is_visible() {
        Ok(true) => {}
        Ok(false) => {
            tracing::debug!("the restored window state hid the window; showing it");
            if let Err(error) = window.show() {
                tracing::warn!(%error, "the main window could not be shown");
            }
        }
        Err(error) => tracing::warn!(%error, "the main window's visibility could not be read"),
    }
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
