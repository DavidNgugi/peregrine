//! The application menu (PG-E1-07), and the reason a menu item exists at all.
//!
//! The menu is described first as data ([`spec`]) and then built from that description
//! ([`install`]). The split is what makes the per-OS layout testable: `spec` is a pure
//! function of the target OS, so the macOS application menu can be asserted on a machine
//! that is not macOS. The build step cannot be tested this way — `muda`, which Tauri's
//! menu API sits on, refuses to create a platform menu item off the main thread
//! (`muda::PlatformMenuItem can only be created on the main thread`) — so it is kept to a
//! translation of the description and nothing else.
//!
//! Every item is either a `PredefinedMenuItem`, whose behaviour and accelerator belong to
//! the OS, or an explicitly disabled `MenuItem`. There is no third kind: an enabled item
//! with no command behind it is a lie the user acts on.

use crate::ShellError;
use tauri::Runtime;
use tauri::menu::{
    AboutMetadata, MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder,
};

/// An OS-provided menu item: cut, copy, paste, and the rest. Its behaviour, its text, and
/// its accelerator are the platform's.
type OsItem = fn(&tauri::AppHandle<tauri::Wry>) -> tauri::Result<PredefinedMenuItem<tauri::Wry>>;

/// One entry in a submenu.
pub(crate) enum Item {
    /// An item the platform owns.
    Os(OsItem),
    /// An item this application owns, which is disabled until the named task lands.
    Deferred {
        /// The identifier the item will report when it is enabled and handled.
        id: &'static str,
        /// The label the user reads.
        text: &'static str,
    },
}

/// One top-level submenu.
pub(crate) struct Submenu {
    /// The title, except for the macOS application menu, which takes the bundle name.
    pub(crate) title: &'static str,
    /// Whether this is the macOS application menu, which is titled with the bundle name
    /// by [`install`] rather than by [`spec`].
    pub(crate) application: bool,
    /// The items, in the order the OS presents them.
    pub(crate) items: Vec<Item>,
}

/// The menu this build presents, as data.
///
/// The `Edit` items are the platform's own, which is what makes cut, copy, paste, and
/// select-all work in a text input on every OS (PG-E1-07's acceptance criterion): a
/// web-drawn clipboard action would have to reimplement the platform's first-responder
/// routing, and would get it wrong. `View` holds only full screen, a window operation the
/// OS owns; zoom is a webview operation no command serves, so it is absent rather than
/// present and dead. `Playback` is the one menu whose actions this application owns, and
/// playback does not exist until Phase 3, so its items are disabled here.
///
/// The macOS application menu is the only platform-conditional part of the layout. It
/// carries what macOS requires the first submenu to carry — About, Services, Hide, Hide
/// Others, Quit — and the OS places it, not this code.
#[must_use]
pub(crate) fn spec() -> Vec<Submenu> {
    let mut menus = Vec::with_capacity(6);

    if cfg!(target_os = "macos") {
        menus.push(Submenu {
            title: "",
            application: true,
            items: vec![
                Item::Os(|app| PredefinedMenuItem::about(app, None, Some(about(app)))),
                Item::Os(PredefinedMenuItem::separator),
                Item::Os(|app| PredefinedMenuItem::services(app, None)),
                Item::Os(PredefinedMenuItem::separator),
                Item::Os(|app| PredefinedMenuItem::hide(app, None)),
                Item::Os(|app| PredefinedMenuItem::hide_others(app, None)),
                Item::Os(PredefinedMenuItem::separator),
                Item::Os(|app| PredefinedMenuItem::quit(app, None)),
            ],
        });
    }

    menus.push(Submenu {
        title: "Edit",
        application: false,
        items: vec![
            Item::Os(|app| PredefinedMenuItem::undo(app, None)),
            Item::Os(|app| PredefinedMenuItem::redo(app, None)),
            Item::Os(PredefinedMenuItem::separator),
            Item::Os(|app| PredefinedMenuItem::cut(app, None)),
            Item::Os(|app| PredefinedMenuItem::copy(app, None)),
            Item::Os(|app| PredefinedMenuItem::paste(app, None)),
            Item::Os(|app| PredefinedMenuItem::select_all(app, None)),
        ],
    });

    menus.push(Submenu {
        title: "View",
        application: false,
        items: vec![Item::Os(|app| PredefinedMenuItem::fullscreen(app, None))],
    });

    menus.push(Submenu {
        title: "Playback",
        application: false,
        items: vec![
            Item::Deferred {
                id: "playback.play_pause",
                text: "Play/Pause",
            },
            Item::Deferred {
                id: "playback.next",
                text: "Next",
            },
            Item::Deferred {
                id: "playback.previous",
                text: "Previous",
            },
        ],
    });

    menus.push(Submenu {
        title: "Window",
        application: false,
        items: vec![
            Item::Os(|app| PredefinedMenuItem::minimize(app, None)),
            Item::Os(|app| PredefinedMenuItem::maximize(app, None)),
            Item::Os(PredefinedMenuItem::separator),
            Item::Os(|app| PredefinedMenuItem::close_window(app, None)),
        ],
    });

    menus.push(Submenu {
        title: "Help",
        application: false,
        // Only macOS forbids an About entry here; every other platform has no application
        // menu for it to live in.
        items: if cfg!(target_os = "macos") {
            Vec::new()
        } else {
            vec![Item::Os(|app| {
                PredefinedMenuItem::about(app, None, Some(about(app)))
            })]
        },
    });

    menus
}

/// The About panel's contents, from the metadata the bundle was built with.
fn about<R: Runtime>(app: &tauri::AppHandle<R>) -> AboutMetadata<'static> {
    let package = app.package_info();
    AboutMetadata {
        name: Some(package.name.clone()),
        version: Some(package.version.to_string()),
        copyright: app.config().bundle.copyright.clone(),
        ..AboutMetadata::default()
    }
}

/// Builds the menu described by [`spec`] and installs it on the application.
///
/// Installing rather than returning keeps a second install from being possible. The return
/// value of `set_menu` is the menu Tauri replaced: on macOS that is the default menu
/// `App::build` created before `setup` ran, and dropping it is deliberate. This menu
/// carries the same Edit and Window items, so the default would only add a second Quit.
///
/// # Errors
///
/// [`ShellError::Menu`] when the OS refuses a menu item or a submenu.
pub(crate) fn install(app: &tauri::AppHandle) -> Result<(), ShellError> {
    let mut root = MenuBuilder::new(app);
    for submenu in spec() {
        let title = if submenu.application {
            app.package_info().name.clone()
        } else {
            submenu.title.to_owned()
        };
        let mut builder = SubmenuBuilder::new(app, title);
        for item in submenu.items {
            builder = match item {
                Item::Os(create) => builder.item(&create(app).map_err(ShellError::Menu)?),
                // The id is declared now so the handler that enables this in Phase 3 does
                // not change what the OS already knows the item by. It is inert while
                // `enabled(false)` stands.
                Item::Deferred { id, text } => builder.item(
                    &MenuItemBuilder::with_id(id, text)
                        .enabled(false)
                        .build(app)
                        .map_err(ShellError::Menu)?,
                ),
            };
        }
        root = root.item(&builder.build().map_err(ShellError::Menu)?);
    }
    root.build()
        .and_then(|menu| app.set_menu(menu))
        .map(|_replaced| ())
        .map_err(ShellError::Menu)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn titles() -> Vec<&'static str> {
        spec().into_iter().map(|menu| menu.title).collect()
    }

    fn items_of(title: &str) -> Vec<Item> {
        spec()
            .into_iter()
            .find(|menu| menu.title == title)
            .unwrap_or_else(|| panic!("no {title} submenu"))
            .items
    }

    #[test]
    fn the_application_menu_comes_first_on_macos_and_exists_nowhere_else() {
        let first = spec().into_iter().next().expect("at least one submenu");
        if cfg!(target_os = "macos") {
            assert!(
                first.application,
                "the first submenu on macOS is the app menu"
            );
            assert_eq!(
                titles(),
                ["", "Edit", "View", "Playback", "Window", "Help"],
                "macOS takes the application menu, and About lives inside it"
            );
        } else {
            assert!(
                !first.application,
                "no platform but macOS has an application menu"
            );
            assert_eq!(
                titles(),
                ["Edit", "View", "Playback", "Window", "Help"],
                "About moves to Help where there is no application menu"
            );
        }
    }

    /// The item-level placement the top-level assertion cannot see: macOS forbids an About
    /// entry in Help, and the other platforms have nowhere else to put it.
    #[test]
    fn about_moves_to_the_application_menu_on_macos_and_to_help_elsewhere() {
        let help_has_about = items_of("Help")
            .iter()
            .any(|item| matches!(item, Item::Os(_)));
        if cfg!(target_os = "macos") {
            assert!(!help_has_about, "macOS puts About in the application menu");
            assert_eq!(
                items_of("").len(),
                8,
                "About, separator, Services, separator, Hide, Hide Others, separator, Quit"
            );
        } else {
            assert!(
                help_has_about,
                "About is the only item Help can hold this phase"
            );
        }
    }

    #[test]
    fn the_edit_menu_is_the_platforms_own_clipboard_actions() {
        // Six actions and the separator between undo/redo and the clipboard. All of them
        // are the OS's, which is what makes Cut/Copy/Paste/Select All work in an input.
        let edit = items_of("Edit");
        assert_eq!(edit.len(), 7);
        assert!(edit.iter().all(|item| matches!(item, Item::Os(_))));
    }

    #[test]
    fn every_playback_item_is_deferred_and_named_uniquely() {
        let playback = items_of("Playback");
        assert!(
            !playback.is_empty(),
            "the Playback menu exists so the gap is visible"
        );
        let ids: BTreeSet<&str> = playback
            .iter()
            .map(|item| match item {
                Item::Deferred { id, text } => {
                    assert!(!text.is_empty(), "a deferred item still needs a label");
                    *id
                }
                Item::Os(_) => panic!("this phase has no playback action for the OS to own"),
            })
            .collect();
        assert_eq!(
            ids.len(),
            playback.len(),
            "two items cannot share an id, or the Phase 3 handler cannot tell them apart"
        );
    }

    #[test]
    fn every_menu_the_task_names_is_present_once() {
        let titles = titles();
        let unique: BTreeSet<&str> = titles.iter().copied().collect();
        assert_eq!(titles.len(), unique.len(), "a submenu appears once");
        for required in ["Edit", "View", "Playback", "Window", "Help"] {
            assert!(unique.contains(required), "{required} is missing");
        }
    }
}
