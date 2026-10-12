//! The tray extra and its state-aware menu (PG-E1-09).
//!
//! The menu is described as data ([`spec`]) and built from that description
//! ([`install`]), the same split `menu.rs` uses: `spec` is a pure function of the core's
//! state, so what the tray says can be asserted on a machine that never draws one.
//!
//! # Where the numbers come from
//!
//! The counts are read from the running runtime — the job journal and scheduler own them
//! (docs/ARCHITECTURE.md §6, "State ownership") — and the tray never derives them from a
//! webview event. The shell re-reads them on an interval and rebuilds the menu only when
//! a count actually changed, because `muda`, which Tauri's menus sit on, will only create
//! a platform menu item on the main thread.
//!
//! # The window-state hazard
//!
//! `tauri-plugin-window-state` persists `visible` under `StateFlags::all()` and calls
//! `show()` only when the restored state says visible. A tray action that *hides* the
//! window therefore writes `visible: false`, and the next launch restores it and never
//! appears. Nothing here hides the window — hiding is close-to-tray, which is PG-E1-10 —
//! and `lib.rs` shows the window itself when the restored state says it is hidden, which
//! is the half of the fix that does not depend on this module.

use std::time::Duration;

use peregrine_ipc::ShellStatus;
use tauri::menu::{Menu, MenuBuilder, MenuItemBuilder, PredefinedMenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconId};
use tauri::{AppHandle, Manager as _, Runtime};

/// The tray's id, so a later update finds the icon it already built.
const TRAY_ID: &str = "peregrine-tray";

/// The item that raises the window.
const ID_SHOW: &str = "tray.show";

/// The item that stops the application.
const ID_QUIT: &str = "tray.quit";

/// The row that reports how much work is in flight.
const ID_ACTIVE: &str = "tray.active_downloads";

/// The row that reports how many torrents are seeding.
const ID_SEEDING: &str = "tray.seeding";

/// How often the tray re-reads the core's counters.
///
/// Two seconds is a readability choice, not a budget: a count that lags a job's start by
/// two seconds is still the count the user is looking for, and a faster poll would rebuild
/// a native menu while the user is reading it.
const REFRESH: Duration = Duration::from_secs(2);

/// One line of the tray menu, as data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrayEntry {
    /// A count the user reads but cannot click.
    Status {
        /// The id the platform knows the row by.
        id: &'static str,
        /// The text, with the count already in it.
        text: String,
    },
    /// A divider between the counts and the actions.
    Separator,
    /// Something the user can choose.
    Action {
        /// The id the menu event reports.
        id: &'static str,
        /// The label.
        text: &'static str,
    },
}

/// The menu the given state presents.
#[must_use]
pub(crate) fn spec(status: &ShellStatus) -> Vec<TrayEntry> {
    vec![
        TrayEntry::Status {
            id: ID_ACTIVE,
            text: format!("Active downloads: {}", status.active_downloads),
        },
        TrayEntry::Status {
            id: ID_SEEDING,
            text: format!("Seeding: {}", status.seeding),
        },
        TrayEntry::Separator,
        TrayEntry::Action {
            id: ID_SHOW,
            text: "Show Peregrine",
        },
        TrayEntry::Action {
            id: ID_QUIT,
            text: "Quit Peregrine",
        },
    ]
}

/// Builds the tray for `status`, removes it when the state says the tray is off, and
/// updates the menu of a tray that already exists.
///
/// Must run on the main thread: every branch either creates a platform menu item or asks
/// the running tray for one.
pub(crate) fn install<R: Runtime>(app: &AppHandle<R>, status: &ShellStatus) -> tauri::Result<()> {
    let id = TrayIconId::new(TRAY_ID);
    if !status.tray_enabled {
        // Dropping the tray is what hides it: the platform destroys the icon and its
        // menu, which is the "hidden when disabled" half of the acceptance criterion.
        drop(app.remove_tray_by_id(&id));
        return Ok(());
    }

    let menu = build_menu(app, status)?;
    if let Some(tray) = app.tray_by_id(&id) {
        return tray.set_menu(Some(menu));
    }

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| on_menu_event(app, &event));
    // Without an icon the platform draws an empty slot on macOS and Windows, which looks
    // like a broken tray rather than a tray. The bundle's own icon is the only image this
    // shell has (docs/DESIGN_SYSTEM.md §8: no web-drawn chrome).
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    drop(builder.build(app)?);
    // Worth a line: the counts are the acceptance criterion, and this is the one place they
    // are known to have reached the platform rather than only the spec function.
    tracing::info!(
        active_downloads = status.active_downloads,
        seeding = status.seeding,
        "the tray is showing the core's counters"
    );
    Ok(())
}

/// Reads the core's counters on an interval and updates the tray when they change.
///
/// A missing runtime is not an error to report here: the tray is created by the first
/// answer that arrives, and a runtime that never starts leaves the shell without one
/// rather than with a menu that claims zero work is happening.
pub(crate) fn spawn_refresh<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(REFRESH);
        let mut last: Option<ShellStatus> = None;
        loop {
            ticker.tick().await;
            let Some(status) = peregrine_ipc::runtime::shell_status(&app).await else {
                continue;
            };
            if last == Some(status) {
                continue;
            }
            last = Some(status);
            let update = app.clone();
            // `muda` refuses to create a platform menu item off the main thread.
            if let Err(error) = app.run_on_main_thread(move || {
                if let Err(error) = install(&update, &status) {
                    tracing::warn!(%error, "the tray menu could not be updated");
                }
            }) {
                tracing::warn!(%error, "the tray update could not reach the main thread");
            }
        }
    });
}

/// Builds the menu [`spec`] describes.
fn build_menu<R: Runtime>(app: &AppHandle<R>, status: &ShellStatus) -> tauri::Result<Menu<R>> {
    let mut builder = MenuBuilder::new(app);
    for entry in spec(status) {
        builder = match entry {
            // Disabled rather than absent: a menu that only listed actions could not show
            // the counts the acceptance criterion asks for, and a clickable count would
            // promise an action that does not exist.
            TrayEntry::Status { id, text } => builder.item(
                &MenuItemBuilder::with_id(id, text)
                    .enabled(false)
                    .build(app)?,
            ),
            TrayEntry::Separator => builder.item(&PredefinedMenuItem::separator(app)?),
            TrayEntry::Action { id, text } => {
                builder.item(&MenuItemBuilder::with_id(id, text).build(app)?)
            }
        };
    }
    builder.build()
}

/// Raises the window, or stops the application.
fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: &tauri::menu::MenuEvent) {
    match event.id().as_ref() {
        ID_SHOW => show_main_window(app),
        ID_QUIT => app.exit(0),
        other => tracing::debug!(id = other, "an unrecognised tray entry was chosen"),
    }
}

/// Shows, unminimizes, and focuses the main window.
///
/// `show` is called unconditionally for the same reason `lib.rs` calls it at startup: the
/// window may have been left hidden in the window-state file, and the tray's whole purpose
/// is to be the way back to a window the user cannot see.
fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window("main") else {
        tracing::debug!("the tray was used while there was no main window to show");
        return;
    };
    if let Err(error) = window
        .show()
        .and_then(|()| window.unminimize())
        .and_then(|()| window.set_focus())
    {
        tracing::warn!(%error, "the main window could not be shown from the tray");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(active_downloads: u32, seeding: u32, tray_enabled: bool) -> ShellStatus {
        ShellStatus {
            active_downloads,
            seeding,
            tray_enabled,
        }
    }

    #[test]
    fn the_menu_names_both_counts_the_acceptance_asks_for() {
        let lines = spec(&status(3, 5, true));
        assert!(lines.contains(&TrayEntry::Status {
            id: ID_ACTIVE,
            text: "Active downloads: 3".to_owned(),
        }));
        assert!(lines.contains(&TrayEntry::Status {
            id: ID_SEEDING,
            text: "Seeding: 5".to_owned(),
        }));
    }

    /// Zero is reported, not omitted: "nothing is downloading" is information, and a menu
    /// that hid the row would leave the user unable to tell it from a broken tray.
    #[test]
    fn an_idle_runtime_reports_zeroes_rather_than_nothing() {
        assert_eq!(
            &spec(&status(0, 0, true))[..2],
            [
                TrayEntry::Status {
                    id: ID_ACTIVE,
                    text: "Active downloads: 0".to_owned(),
                },
                TrayEntry::Status {
                    id: ID_SEEDING,
                    text: "Seeding: 0".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn the_menu_offers_a_way_back_to_the_window_and_a_way_out() {
        let ids: Vec<&str> = spec(&status(1, 0, true))
            .into_iter()
            .filter_map(|entry| match entry {
                TrayEntry::Action { id, .. } => Some(id),
                TrayEntry::Status { .. } | TrayEntry::Separator => None,
            })
            .collect();
        assert_eq!(ids, [ID_SHOW, ID_QUIT]);
    }

    /// The counts are the core's, so the menu cannot disagree with the runtime it reports
    /// on; the ordering of the separator is part of what the user reads.
    #[test]
    fn the_counts_come_before_the_actions_and_are_separated() {
        let lines = spec(&status(2, 1, true));
        assert!(matches!(lines[0], TrayEntry::Status { id: ID_ACTIVE, .. }));
        assert!(matches!(lines[1], TrayEntry::Status { id: ID_SEEDING, .. }));
        assert_eq!(lines[2], TrayEntry::Separator);
        assert!(matches!(lines[3], TrayEntry::Action { id: ID_SHOW, .. }));
        assert!(matches!(lines[4], TrayEntry::Action { id: ID_QUIT, .. }));
    }

    /// "Hidden when disabled" run through the real install path rather than asserted of the
    /// spec: the disabled branch removes whatever tray exists and creates nothing.
    #[test]
    fn a_disabled_tray_leaves_no_icon_behind() {
        let app = tauri::test::mock_app();
        let id = TrayIconId::new(TRAY_ID);

        install(app.handle(), &status(0, 0, false)).expect("a hidden tray is not a failure");

        assert!(app.tray_by_id(&id).is_none());
    }
}
