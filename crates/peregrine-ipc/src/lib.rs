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
pub mod backup;
pub mod bindings;
pub mod discover;
pub mod downloads;
mod error;
pub mod guard;
pub mod indexers;
pub mod jobs;
pub mod library;
pub mod network;
pub mod notifications;
pub mod playback;
pub mod profiles;
pub mod providers;
pub mod prowlarr;
pub mod runtime;
pub mod shortcuts;
pub mod system;
pub mod usenet;
pub mod watch;
pub mod web;

// The shell reads the tray's counters through [`runtime::shell_status`] and may not depend
// on `peregrine-app` directly, so the type that function answers with is named here rather
// than reached through a second dependency edge (AGENTS R6).
pub use peregrine_app::ShellStatus;
// The shell installs the native notification adapter (PG-E1-14) and, for the same reason,
// may not depend on `peregrine-app` directly. The function is re-exported here so the one
// edge the shell takes to the core is this crate.
pub use peregrine_app::notify::install_os_notifier;

#[cfg(test)]
mod contract_tests;
#[cfg(test)]
mod redaction;
#[cfg(test)]
mod test_support;

use tauri_specta::{Builder, collect_commands, collect_events};

/// The command surface, registered once.
///
/// This is the only place a command is registered; the desktop shell and the
/// binding generator both read it, so a command that is invokable is also
/// exported. Everything in docs/ARCHITECTURE.md §7 joins this list as it is
/// built; today that is the `system` and `jobs` namespaces.
#[must_use]
#[allow(clippy::too_many_lines)] // One registration table; a reader has to see every command in one place.
pub fn builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        // `build_info` is generic over the runtime, so the macro cannot infer which one
        // this builder registers; the desktop shell uses `Wry`. The commands that take
        // managed state are not generic and need no turbofish.
        .commands(collect_commands![
            system::build_info::<tauri::Wry>,
            system::health,
            system::provider_attribution,
            system::log_tail,
            backup::create_backup::<tauri::Wry>,
            backup::restore_backup::<tauri::Wry>,
            network::public_ip,
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
            library::watch_availability,
            library::get_person,
            library::person_filmography,
            library::start_playback,
            library::play_externally,
            library::stop_playback,
            library::start_poster_capture,
            library::stop_poster_capture,
            discover::search_providers,
            discover::search_tiles,
            discover::home_rails,
            discover::refresh_rails,
            discover::browse,
            discover::genres,
            discover::recommendations,
            library::refresh_title,
            library::search_titles,
            library::list_episodes,
            library::list_seasons,
            library::match_unmatched,
            library::forget_match_rule,
            web::web_start,
            web::web_stop,
            web::web_status,
            web::web_lan_address,
            web::web_lan_hostname,
            web::web_set_keep_awake,
            web::web_pairings,
            web::web_approve,
            web::web_approve_code,
            web::web_deny,
            web::web_devices,
            web::web_rename,
            web::web_revoke,
            providers::status,
            providers::set_key,
            providers::test_connection,
            providers::clear_key,
            indexers::indexer_status,
            indexers::set_indexer,
            indexers::clear_indexer,
            indexers::test_indexer,
            indexers::search_releases,
            indexers::list_indexers,
            indexers::indexer_catalog,
            indexers::indexer_definition,
            indexers::add_indexer,
            indexers::remove_indexer,
            downloads::acquire,
            downloads::acquire_season,
            downloads::list_downloads,
            downloads::schedule_download,
            downloads::download_schedules,
            downloads::cancel_download_schedule,
            downloads::pause_download,
            downloads::resume_download,
            downloads::find_download_peers,
            downloads::remove_download,
            downloads::dismiss_download,
            downloads::clear_finished_downloads,
            downloads::plan_download_deletion,
            downloads::delete_download,
            downloads::preferred_quality,
            downloads::set_preferred_quality,
            downloads::list_release_offers,
            downloads::choose_release_offer,
            downloads::dismiss_release_offer,
            usenet::usenet_status,
            usenet::save_usenet_server,
            usenet::remove_usenet_server,
            usenet::test_usenet_server,
            usenet::set_usenet_archives,
            usenet::consent_usenet_download,
            notifications::notification_preferences,
            notifications::set_notification_consent,
            notifications::list_notifications,
            notifications::mark_notification_read,
            notifications::mark_all_notifications_read,
            notifications::dismiss_notification,
            profiles::list_profiles,
            profiles::create_profile,
            profiles::select_profile,
            profiles::update_profile,
            watch::continue_watching,
            watch::record_progress,
            watch::set_title_watched,
            watch::set_episode_watched,
            watch::set_season_watched,
            watch::viewing_history,
            watch::watch_metrics,
            watch::clear_watch_history,
            watch::watchlist,
            watch::add_to_watchlist,
            watch::remove_from_watchlist,
            playback::playback_preferences,
            playback::set_playback_preferences,
            prowlarr::prowlarr_status,
            prowlarr::install_prowlarr,
            prowlarr::start_prowlarr,
            prowlarr::stop_prowlarr,
        ])
        .events(collect_events![])
}
