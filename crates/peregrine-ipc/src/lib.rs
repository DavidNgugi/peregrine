#![forbid(unsafe_code)]
//! Tauri commands, event emitters, DTO mapping, error mapping, and the specta
//! type export.
//!
//! Commands are twenty lines or fewer: parse, call one service, map the result.
//! There is no logic here.
//!
//! [`builder`] is the single registration point for the command surface. The
//! desktop shell passes it to Tauri, and `peregrine-ipc`'s `gen-ipc` binary
//! exports the TypeScript bindings from the same builder, so a command that is
//! invokable is also a command that is typed.
//!
//! [`runtime::serve`] is the other half: it starts `peregrine-app`'s runtime
//! process-wide, installs it as Tauri's managed state, and points the event bus at
//! the webview. The shell calls it once, from `setup`, and calls
//! [`runtime::shutdown`] on the way out.

pub mod autostart;
pub mod bindings;
pub mod discover;
mod error;
pub mod guard;
pub mod jobs;
pub mod library;
pub mod runtime;
pub mod shortcuts;
pub mod system;

// The shell reads the tray's counters through [`runtime::shell_status`] and may not depend
// on `peregrine-app` directly, so the type that function answers with is named here rather
// than reached through a second dependency edge (AGENTS R6).
pub use peregrine_app::ShellStatus;

#[cfg(test)]
mod contract_tests;
#[cfg(test)]
mod redaction;

use tauri_specta::{Builder, collect_commands, collect_events};

/// The command surface, registered once.
///
/// This is the only place a command is registered; the desktop shell and the
/// binding generator both read it, so a command that is invokable is also
/// exported. Everything in docs/ARCHITECTURE.md §7 joins this list as it is
/// built; today that is the `system` and `jobs` namespaces.
#[must_use]
pub fn builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        // `build_info` is generic over the runtime, so the macro cannot infer which one
        // this builder registers; the desktop shell uses `Wry`. The commands that take
        // managed state are not generic and need no turbofish.
        .commands(collect_commands![
            system::build_info::<tauri::Wry>,
            system::health,
            system::log_tail,
            shortcuts::list_shortcuts,
            autostart::autostart_enabled::<tauri::Wry>,
            autostart::set_autostart::<tauri::Wry>,
            jobs::start_hello,
            jobs::cancel,
            jobs::list,
            library::list_roots,
            library::add_root,
            library::remove_root,
            library::scan_now,
            library::list_titles,
            library::list_unmatched,
            library::list_match_rules,
            library::get_title,
            discover::search_providers,
            library::search_titles,
            library::list_episodes,
            library::match_unmatched,
            library::forget_match_rule,
        ])
        .events(collect_events![])
}
