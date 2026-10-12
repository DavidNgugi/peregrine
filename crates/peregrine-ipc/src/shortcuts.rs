//! The shortcut registry: the one list of keyboard shortcuts this application has
//! (PG-E1-08).
//!
//! The Settings screen's shortcut list is *generated* from [`registry`] over IPC, and
//! the shell checks the same registry against the application menu at startup, so
//! there is no second list to maintain beside it.
//!
//! # Why the accelerators are `muda`'s types
//!
//! `menu.rs` does not spell out "Cmd on macOS, Ctrl elsewhere": its accelerators are
//! `muda`'s, and `muda`'s [`CMD_OR_CTRL`] is what makes them platform-correct. A
//! registry that wrote `"⌘K"` for macOS and `"Ctrl+K"` for Windows as two strings
//! would be a second table that can disagree with the menu. So an entry here holds
//! the same [`Modifiers`] and [`Code`] a menu item is built from, and the platform
//! form is *rendered* from those pieces by [`Shortcut::render`].
//!
//! [`Modifiers`] and [`Code`] come from `muda`'s own re-export, and `muda` is a
//! target-specific dependency `tauri` already carries on every desktop platform, so
//! this adds a direct edge and no crate to the graph.

use muda::accelerator::{Accelerator, CMD_OR_CTRL, Code, Modifiers};
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::{AppError, ErrorCode};
use crate::guard::guard_response;

/// A keyboard shortcut, held as the pieces a platform accelerator is built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Shortcut {
    modifiers: Modifiers,
    key: Code,
}

impl Shortcut {
    /// Builds a shortcut from the platform's own modifier set and key.
    #[must_use]
    pub const fn new(modifiers: Modifiers, key: Code) -> Self {
        Self { modifiers, key }
    }

    /// The accelerator for a menu item, in the form `muda` takes.
    #[must_use]
    pub fn to_accelerator(self) -> Accelerator {
        Accelerator::new(self.modifiers, self.key)
    }

    /// Whether `modifier` is one of the shortcut's own.
    #[must_use]
    fn holds(self, modifier: Modifiers) -> bool {
        self.modifiers.contains(modifier)
    }

    /// The keys a user reads, in the platform's own notation.
    ///
    /// macOS writes modifiers as symbols in Apple's order (Control, Option, Shift,
    /// Command), because that is what the menu bar shows and a Settings list that
    /// disagreed with the menu bar would be worse than no list. Every other platform
    /// writes them as words joined by `+`.
    ///
    /// `is_macos` is a parameter rather than a `cfg` inside, so the branch this build
    /// cannot run is still asserted by a test on the machine that builds it.
    #[must_use]
    pub fn render_for(self, is_macos: bool) -> String {
        if is_macos {
            let mut rendered = String::new();
            for (modifier, symbol) in [
                (Modifiers::CONTROL, "⌃"),
                (Modifiers::ALT, "⌥"),
                (Modifiers::SHIFT, "⇧"),
                (Modifiers::META, "⌘"),
            ] {
                if self.holds(modifier) {
                    rendered.push_str(symbol);
                }
            }
            rendered.push_str(&key_label(self.key));
            rendered
        } else {
            let mut parts: Vec<String> = Vec::new();
            // `Meta` is the Windows key, which no shortcut here uses; naming it rather
            // than dropping it means a binding that adds it renders the keys the user
            // must actually press.
            for (modifier, name) in [
                (Modifiers::META, "Meta"),
                (Modifiers::CONTROL, "Ctrl"),
                (Modifiers::ALT, "Alt"),
                (Modifiers::SHIFT, "Shift"),
            ] {
                if self.holds(modifier) {
                    parts.push(name.to_owned());
                }
            }
            parts.push(key_label(self.key));
            parts.join("+")
        }
    }

    /// The rendered shortcut for the platform this binary was built for.
    #[must_use]
    pub fn render(self) -> String {
        self.render_for(cfg!(target_os = "macos"))
    }
}

/// The key's own label, without modifiers.
fn key_label(key: Code) -> String {
    match key {
        Code::Digit0 => "0".to_owned(),
        Code::Digit1 => "1".to_owned(),
        Code::Digit2 => "2".to_owned(),
        Code::Digit3 => "3".to_owned(),
        Code::Digit4 => "4".to_owned(),
        Code::Digit5 => "5".to_owned(),
        Code::Digit6 => "6".to_owned(),
        Code::Digit7 => "7".to_owned(),
        Code::Digit8 => "8".to_owned(),
        Code::Digit9 => "9".to_owned(),
        Code::Comma => ",".to_owned(),
        Code::Period => ".".to_owned(),
        Code::Slash => "/".to_owned(),
        Code::Space => "Space".to_owned(),
        // `Debug` spells a `keyboard_types::Code` the way the webview's
        // `KeyboardEvent.code` does, so the letter keys and anything this registry does
        // not use yet still render as the key the user presses rather than as a value
        // that only a programmer can read.
        other => format!("{other:?}").trim_start_matches("Key").to_owned(),
    }
}

/// One action a shortcut can be bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShortcutAction {
    /// The stable id the frontend dispatches on.
    pub id: &'static str,
    /// The translation key for the label the user reads.
    pub label_key: &'static str,
    /// The accelerator, in the form the menu is built from.
    pub accelerator: Shortcut,
}

/// Why two accelerators cannot both exist.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShortcutConflict {
    /// Two registry entries claim one accelerator.
    #[error("`{first}` and `{second}` both claim {accelerator}")]
    Duplicate {
        /// The first action, in registry order.
        first: &'static str,
        /// The action that repeated it.
        second: &'static str,
        /// The rendered accelerator they share.
        accelerator: String,
    },
    /// A binding claims an accelerator an application menu item already owns.
    #[error("`{action}` claims {accelerator}, which the application menu owns")]
    MenuOwned {
        /// The registry action.
        action: &'static str,
        /// The rendered accelerator.
        accelerator: String,
    },
}

/// The shortcuts this application has, in the order Settings lists them.
///
/// Every entry is an action the UI can perform today: the three screens the shell
/// has. A previous package refused to add an enabled menu item with nothing behind it
/// (PG-E1-07), and the same rule applies here — an accelerator that does nothing is
/// worse than an unlisted one.
#[must_use]
pub fn registry() -> Vec<ShortcutAction> {
    vec![
        ShortcutAction {
            id: "navigation.discover",
            label_key: "shortcuts.discover",
            accelerator: Shortcut::new(CMD_OR_CTRL, Code::Digit1),
        },
        ShortcutAction {
            id: "navigation.jobs",
            label_key: "shortcuts.jobs",
            accelerator: Shortcut::new(CMD_OR_CTRL, Code::Digit2),
        },
        ShortcutAction {
            id: "navigation.settings",
            label_key: "shortcuts.settings",
            accelerator: Shortcut::new(CMD_OR_CTRL, Code::Digit3),
        },
    ]
}

/// Checks a list of actions against itself and against the accelerators the
/// application menu already owns.
///
/// This is the conflict detection. It runs at startup as well as in the tests: a
/// registry that binds an accelerator twice, or that binds one the menu dispatches,
/// would fire an action twice or shadow a menu item, and both read to a user as "the
/// keyboard is flaky".
///
/// `reserved` is what the installed menu claims. The OS-owned items — the Edit,
/// Window, and application menus — get their accelerators from `muda`'s own
/// predefined table, and `muda` exposes no accessor for it, so they cannot be read
/// back; what this can see is the accelerators the application itself declares. That
/// gap is named in the task note rather than papered over with a copy of `muda`'s
/// table, which is the drift this registry exists to avoid.
///
/// # Errors
///
/// [`ShortcutConflict`] naming the entries.
pub fn validate(actions: &[ShortcutAction], reserved: &[Shortcut]) -> Result<(), ShortcutConflict> {
    for (index, action) in actions.iter().enumerate() {
        if reserved.contains(&action.accelerator) {
            return Err(ShortcutConflict::MenuOwned {
                action: action.id,
                accelerator: action.accelerator.render(),
            });
        }
        if let Some(earlier) = actions[..index]
            .iter()
            .find(|earlier| earlier.accelerator == action.accelerator)
        {
            return Err(ShortcutConflict::Duplicate {
                first: earlier.id,
                second: action.id,
                accelerator: action.accelerator.render(),
            });
        }
    }
    Ok(())
}

/// Checks the shipped registry against the accelerators the menu declares.
///
/// # Errors
///
/// [`ShortcutConflict`], which is a build defect rather than a user error.
pub fn validate_registry() -> Result<(), ShortcutConflict> {
    validate(&registry(), &reserved_accelerators())
}

/// The accelerators the application menu owns, as the menu module declares them.
///
/// The shell passes the menu's application-owned items here; until such an item
/// exists the set is empty, and the conflict detector is still what a new item has to
/// satisfy.
#[must_use]
pub fn reserved_accelerators() -> Vec<Shortcut> {
    Vec::new()
}

/// One row of the Settings shortcut list.
///
/// The four modifier flags are the shape the webview compares against `KeyboardEvent`, and
/// a flat set of booleans is what makes that comparison three lines of TypeScript rather
/// than a parsing routine. They are a wire DTO, not a state machine.
#[allow(
    clippy::struct_excessive_bools,
    reason = "the wire shape mirrors KeyboardEvent's four modifier flags deliberately"
)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutBinding {
    /// The action id.
    pub id: String,
    /// The translation key for the label.
    pub label_key: String,
    /// The accelerator as the user reads it on this platform.
    pub display: String,
    /// The `KeyboardEvent.code` value the webview sees for the key.
    pub code: String,
    /// Whether the platform's Command key (Meta) is held.
    pub meta: bool,
    /// Whether Control is held.
    pub ctrl: bool,
    /// Whether Alt or Option is held.
    pub alt: bool,
    /// Whether Shift is held.
    pub shift: bool,
}

impl From<&ShortcutAction> for ShortcutBinding {
    fn from(action: &ShortcutAction) -> Self {
        let accelerator = action.accelerator;
        Self {
            id: action.id.to_owned(),
            label_key: action.label_key.to_owned(),
            display: accelerator.render(),
            // The `Debug` name of a `keyboard_types::Code` is its W3C spelling, which is
            // the value the webview reads from `event.code`; comparing them is what lets
            // the frontend match a key without a table of its own.
            code: format!("{:?}", accelerator.key),
            meta: accelerator.holds(Modifiers::META),
            ctrl: accelerator.holds(Modifiers::CONTROL),
            alt: accelerator.holds(Modifiers::ALT),
            shift: accelerator.holds(Modifiers::SHIFT),
        }
    }
}

/// Every shortcut this application has, as the Settings list renders it.
///
/// # Errors
///
/// [`ErrorCode::Internal`] when the registry conflicts with itself or with the menu,
/// because a list whose entries cannot be told apart is not one the UI should draw.
#[tauri::command]
#[specta::specta]
pub fn list_shortcuts() -> Result<Vec<ShortcutBinding>, AppError> {
    validate_registry().map_err(|conflict| {
        AppError::new(ErrorCode::Internal)
            .with_detail("The keyboard shortcuts are misconfigured, so the list is unavailable.")
            .with_cause(conflict.to_string())
    })?;
    Ok(guard_response(
        "shortcuts::list_shortcuts",
        registry().iter().map(ShortcutBinding::from).collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registry_entry_renders_a_platform_form() {
        let rendered: Vec<String> = registry()
            .iter()
            .map(|action| action.accelerator.render())
            .collect();
        let expected = if cfg!(target_os = "macos") {
            vec!["⌘1", "⌘2", "⌘3"]
        } else {
            vec!["Ctrl+1", "Ctrl+2", "Ctrl+3"]
        };
        assert_eq!(rendered, expected);
    }

    /// `CMD_OR_CTRL` is resolved at *compile* time, so a macOS build of the registry
    /// holds the Command key and a Windows build holds Control; that is the whole point
    /// of naming `muda`'s constant instead of writing two strings. What varies at
    /// runtime is the notation, and both notations are asserted here on one machine.
    #[test]
    fn both_notations_are_rendered_from_one_binding() {
        let command_or_control = registry()
            .first()
            .expect("the registry is not empty")
            .accelerator;
        if cfg!(target_os = "macos") {
            assert_eq!(command_or_control.render(), "⌘1");
            assert_eq!(
                command_or_control.render_for(false),
                "Meta+1",
                "the same modifier bit is the Windows key under the other notation"
            );
        } else {
            assert_eq!(command_or_control.render(), "Ctrl+1");
            assert_eq!(
                command_or_control.render_for(true),
                "⌃1",
                "the same modifier bit is Control under the macOS notation"
            );
        }

        let control = Shortcut::new(Modifiers::CONTROL, Code::Digit1);
        assert_eq!(control.render_for(true), "⌃1");
        assert_eq!(control.render_for(false), "Ctrl+1");
    }

    #[test]
    fn a_modified_binding_names_every_modifier_the_user_must_hold() {
        let shortcut = Shortcut::new(
            Modifiers::META | Modifiers::ALT | Modifiers::SHIFT,
            Code::KeyK,
        );
        assert_eq!(shortcut.render_for(true), "⌥⇧⌘K");
        assert_eq!(shortcut.render_for(false), "Meta+Alt+Shift+K");
    }

    #[test]
    fn two_actions_claiming_one_accelerator_is_a_refusal() {
        let actions = [
            ShortcutAction {
                id: "duplicate.one",
                label_key: "shortcuts.discover",
                accelerator: Shortcut::new(CMD_OR_CTRL, Code::Digit9),
            },
            ShortcutAction {
                id: "duplicate.two",
                label_key: "shortcuts.jobs",
                accelerator: Shortcut::new(CMD_OR_CTRL, Code::Digit9),
            },
        ];
        let conflict = validate(&actions, &[]).unwrap_err();
        assert!(
            matches!(conflict, ShortcutConflict::Duplicate { first, second, .. }
                if first == "duplicate.one" && second == "duplicate.two"),
            "a second claim on one accelerator has to be refused"
        );
    }

    #[test]
    fn a_binding_that_collides_with_a_menu_item_is_a_refusal() {
        // The accelerator a menu item already owns, which is what the shell passes in
        // when the application menu declares one.
        let reserved = [Shortcut::new(CMD_OR_CTRL, Code::Digit1)];
        let conflict = validate(&registry(), &reserved).unwrap_err();
        assert!(
            matches!(&conflict, ShortcutConflict::MenuOwned { action, .. }
                if *action == "navigation.discover"),
            "unexpected conflict: {conflict:?}"
        );
        assert!(conflict.to_string().contains("⌘1") || conflict.to_string().contains("Ctrl+1"));
    }

    #[test]
    fn the_shipped_registry_is_conflict_free() {
        validate_registry().expect("the shipped registry has no conflict");
    }

    #[test]
    fn a_binding_names_the_key_the_webview_sees() {
        let bindings: Vec<ShortcutBinding> = registry().iter().map(ShortcutBinding::from).collect();
        assert_eq!(
            bindings
                .iter()
                .map(|binding| binding.code.as_str())
                .collect::<Vec<_>>(),
            ["Digit1", "Digit2", "Digit3"],
            "event.code in the webview spells the same key the same way"
        );
        let first = bindings.first().expect("the registry is not empty");
        assert_eq!(first.meta, cfg!(target_os = "macos"));
        assert_eq!(first.ctrl, !cfg!(target_os = "macos"));
        assert!(!first.alt && !first.shift);
    }

    #[test]
    fn the_command_answers_with_one_row_per_registry_entry() {
        let rows = list_shortcuts().expect("the shipped registry is conflict-free");
        assert_eq!(rows.len(), registry().len());
        assert_eq!(rows[0].id, "navigation.discover");
        assert_ne!(rows[0].label_key, "");
    }
}
