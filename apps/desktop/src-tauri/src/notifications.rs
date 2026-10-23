//! Native notification delivery: the Tauri adapter behind [`OsNotifier`] (PG-E1-14).
//!
//! The decision to notify is the core's ([`peregrine_domain::notify::plan`], composed by
//! `peregrine-app`'s service, AGENTS R31); this module is only the hand that shows the
//! toast. Two things live here and nowhere else in the shipping binary:
//!
//! * the **click handler**, registered on the plugin before any toast can be shown. ADR
//!   0015 §6 makes that ordering load-bearing rather than tidy: the plugin only asks the
//!   OS to report clicks while a handler exists, and a notification shown before the
//!   first handler reports nothing at all, ever.
//! * the **rotating int32 id** the plugin requires, persisted in the app data directory.
//!
//! The adapter is handed to the core at startup through `peregrine_ipc::install_os_notifier`,
//! before the runtime composes its notification service; the core names no Tauri type
//! (AGENTS R8). Consent stays the core's question and is not asked here: the shell requests the
//! *OS* permission once, in `setup`, and the adapter reports a refusal on every event rather
//! than prompting from a background path (AGENTS R31).
//!
//! # What the plugin can and cannot tell us
//!
//! `tauri-plugin-notification` 2.6.0 is fire-and-forget: `show()` prepares the payload and
//! returns, the OS call happens on a spawned task, and the task's error is discarded (its
//! `imp::Notification::show` is `let _ = notification.show()`, and the tracked path does
//! the same). So a successful [`OsNotifier::notify`] means "the toast was handed to the
//! plugin", not "the user saw it", and the plugin's only desktop error is an I/O failure
//! while preparing the payload. That is exactly why AGENTS R31 makes the in-app centre the
//! record and the toast the bonus: there is no channel here to report a missing
//! notification daemon, a muted app, or a lost click, so nothing may depend on one.
//!
//! # Where the id counter lives, and what it does at the wrap
//!
//! `notification-id`, a one-line decimal file in the application data directory beside
//! `peregrine.sqlite` and `logs/`. The plugin's `id` is an `i32` and cannot hold the
//! centre's `i64` row id, so the id is not a key into anything: ids are issued `1..=i32::MAX`
//! and wrap back to 1, and 0 is never issued because an absent or unreadable file is read
//! as "nothing issued yet". A wrap therefore reuses an id the machine saw about two
//! billion toasts ago; it is harmless because nothing routes by id — a click carries the
//! deep link in `extra`, and the centre row carries its own `i64` id — and because on
//! macOS and Windows the plugin ignores the value entirely. On Linux the plugin keeps our
//! id beside the server's own id so `remove_active` can name a toast; nothing in this
//! build removes one, and a collision there would at worst close a stale toast.
//!
//! The file is written through a sibling temporary and a rename, so a crash mid-write
//! leaves the previous value rather than a truncated one, and a write failure is logged
//! and does not stop the toast: the in-memory counter keeps this process's ids unique,
//! which is all that matters while it runs.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use peregrine_domain::notify::{NotifyError, OsNotifier, Outgoing};
use tauri::{AppHandle, Manager as _, Runtime};
use tauri_plugin_notification::{ActionPerformed, NotificationExt as _, PermissionState};

/// The `extra` key a toast carries its deep link under.
///
/// The same link the centre row stores, so the two destinations cannot disagree (ADR 0015
/// §6). The value is what a click reports; the toast's visible text never carries a link.
pub(crate) const DEEP_LINK_KEY: &str = "deepLink";

/// The file the rotating id counter is persisted in, inside the app data directory.
pub(crate) const ID_COUNTER_FILE: &str = "notification-id";

/// The first id issued, and the id the counter wraps back to.
///
/// One rather than zero so that 0 can mean "nothing has been issued", which is what an
/// absent, empty, or unreadable file has to mean.
const FIRST_ID: i32 = 1;

/// The action a click on the notification body is reported as, on every desktop platform.
const CLICK_ACTION: &str = "tap";

/// Why native notifications could not be set up at all.
#[derive(Debug, thiserror::Error)]
pub(crate) enum NotificationError {
    /// The plugin refused to register the click handler.
    #[error("the notification click handler could not be registered: {0}")]
    Click(#[from] tauri_plugin_notification::Error),
    /// The application data directory, where the id counter lives, could not be resolved.
    #[error("the notification id counter's directory could not be resolved")]
    DataDirectory(#[source] tauri::Error),
}

/// Registers the click handler and installs the [`TauriOsNotifier`] the service composes.
///
/// Called once from the shell's `setup`, before the runtime can raise an event and before
/// the document loads, which is the earliest point at which the ordering ADR 0015 §6
/// requires is guaranteed.
///
/// # Errors
///
/// [`NotificationError`] when the handler cannot be registered or the data directory
/// cannot be resolved. A failure is reported and not fatal: the centre is the record, and
/// an app whose toasts do not work still works.
pub(crate) fn install<R: Runtime>(app: &AppHandle<R>) -> Result<(), NotificationError> {
    register_click_handler(app)?;
    // The OS's own permission is asked for once, here, where the user is in the application —
    // never from a background tick. On desktop the plugin answers `Granted` without prompting;
    // a platform that does prompt asks while the app is in front of the person. A refusal is
    // not fatal: the adapter reports it on every event and the centre keeps the record (R31).
    match app.notification().request_permission() {
        Ok(state) => tracing::debug!(?state, "the OS notification permission was requested"),
        Err(error) => {
            tracing::warn!(%error, "the OS notification permission could not be requested");
        }
    }
    let directory = app
        .path()
        .app_data_dir()
        .map_err(NotificationError::DataDirectory)?;
    peregrine_ipc::install_os_notifier(Arc::new(TauriOsNotifier::new(
        app.clone(),
        directory.join(ID_COUNTER_FILE),
    )));
    Ok(())
}

/// Registers the handler a click reaches, routing it through the external-link seam.
///
/// Separate from [`install`] so the routing half is testable without resolving a real data
/// directory.
fn register_click_handler<R: Runtime>(app: &AppHandle<R>) -> Result<(), NotificationError> {
    let handle = app.clone();
    app.notification()
        .on_action(move |performed| route_click(&handle, performed))?;
    tracing::debug!("the notification click handler was registered");
    Ok(())
}

/// Routes a click on a toast through the same router a `peregrine://` link takes.
///
/// [`crate::external::forward_urls`] is the one path from "something asked us to open a
/// link" to "the link was classified": it parses with the domain's grammar, refuses
/// everything the grammar refuses, and delivers through [`crate::external::Delivery`] so a
/// click that arrives before the document has loaded is held rather than lost. A click is
/// therefore not a second, weaker route — it is the same one.
fn route_click<R: Runtime>(app: &AppHandle<R>, performed: &ActionPerformed) {
    let Some(link) = clicked_link(performed) else {
        return;
    };
    // Never the link itself: the diagnostics tail is exported (SECURITY.md T13), and the
    // router already logs the route it classified.
    tracing::debug!(
        action = performed.action_id(),
        "a notification click carried a link"
    );
    crate::external::forward_urls(app, &[link]);
}

/// The deep link a performed action carries, when it is a click that carries one.
///
/// Only the body click routes: this build registers no action types, so any other action
/// id is not something we asked for and is ignored rather than guessed at.
fn clicked_link(performed: &ActionPerformed) -> Option<String> {
    if performed.action_id() != CLICK_ACTION {
        return None;
    }
    let value = performed.notification()?.extra().get(DEEP_LINK_KEY)?;
    value.as_str().map(str::to_owned)
}

/// The rotating int32 identifier a toast is shown under.
pub(crate) struct NativeIds {
    path: PathBuf,
    last: Mutex<i32>,
}

impl NativeIds {
    /// Reads the counter, treating an absent or unreadable file as "nothing issued yet".
    ///
    /// Called once per launch: the value is the last id issued by any previous launch, so
    /// a restart continues the sequence rather than repeating it.
    fn open(path: PathBuf) -> Self {
        let last = read_last(&path).unwrap_or(0);
        Self {
            path,
            last: Mutex::new(last),
        }
    }

    /// The next id, persisted before it is returned.
    fn next(&self) -> i32 {
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        let next = if *last == i32::MAX {
            FIRST_ID
        } else {
            (*last).max(0) + 1
        };
        *last = next;
        if let Err(error) = write_last(&self.path, next) {
            // The toast still goes out: the in-memory counter keeps this process unique,
            // and the next launch repeats at most the ids issued since the last write.
            tracing::warn!(%error, "the notification id counter could not be persisted");
        }
        next
    }
}

/// Reads the last issued id, if the file holds one this build can use.
fn read_last(path: &Path) -> Option<i32> {
    let text = std::fs::read_to_string(path).ok()?;
    let value = text.trim().parse::<i32>().ok()?;
    (FIRST_ID..=i32::MAX).contains(&value).then_some(value)
}

/// Persists the last issued id, through a sibling temporary and a rename.
fn write_last(path: &Path, value: i32) -> std::io::Result<()> {
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, value.to_string())?;
    std::fs::rename(&temporary, path)
}

/// The `OsNotifier` adapter the notification service is composed with.
pub(crate) struct TauriOsNotifier<R: Runtime> {
    app: AppHandle<R>,
    ids: NativeIds,
}

impl<R: Runtime> TauriOsNotifier<R> {
    /// Builds the adapter, reading the counter from `counter_path`.
    fn new(app: AppHandle<R>, counter_path: PathBuf) -> Self {
        Self {
            app,
            ids: NativeIds::open(counter_path),
        }
    }

    /// The OS's own permission state, read without asking for it.
    ///
    /// The request happens once at startup; this never prompts, so a notification raised by a
    /// background job cannot raise a system dialog. A refusal is the port's failure, which the
    /// service records on the centre row beside the entry it already wrote (AGENTS R31).
    fn permitted(&self) -> Result<(), NotifyError> {
        let state = self
            .app
            .notification()
            .permission_state()
            .map_err(|error| refusal(&error))?;
        permission_decision(state)
    }

    /// Hands one toast to the plugin and reports the id it was shown under.
    fn show(&self, outgoing: &Outgoing) -> Result<i32, NotifyError> {
        self.permitted()?;
        let toast = Toast::of(self.ids.next(), outgoing);
        let mut builder = self
            .app
            .notification()
            .builder()
            .id(toast.id)
            .title(toast.title)
            // The link travels in `extra`, which is what a click reports back, so the
            // toast's own text never needs to hold a destination (ADR 0015 §6, §7).
            .extra(DEEP_LINK_KEY, toast.deep_link);
        if let Some(body) = toast.body {
            builder = builder.body(body);
        }
        if let Some(sound) = toast.sound {
            builder = builder.sound(sound);
        }
        builder
            .show()
            .map(|()| toast.id)
            .map_err(|error| refusal(&error))
    }
}

impl<R: Runtime> OsNotifier for TauriOsNotifier<R> {
    fn notify(&self, notification: &Outgoing) -> Result<i32, NotifyError> {
        self.show(notification)
    }
}

/// What the port answers for the OS's permission state.
///
/// `Prompt` is a refusal rather than a reason to ask: the app's consent is the core's question
/// (AGENTS R31), and a notification raised in the background must not put a system dialog in
/// front of the user. The plugin's desktop `permission_state` always answers `Granted`; the
/// other arms exist because a denial is a fact this adapter must report wherever it occurs.
fn permission_decision(state: PermissionState) -> Result<(), NotifyError> {
    match state {
        PermissionState::Granted => Ok(()),
        PermissionState::Denied
        | PermissionState::Prompt
        | PermissionState::PromptWithRationale => Err(NotifyError::Refused {
            message: "the system has not granted permission to show notifications".to_owned(),
        }),
    }
}

/// One prepared toast, as the plugin's builder receives it.
///
/// A value rather than a chain of builder calls so the mapping from the port's
/// [`Outgoing`] to what the OS is handed is testable without showing anything: the plugin
/// has no way to inspect a builder, and its `show()` is asynchronous and silent, so the
/// only honest place to assert the payload is here.
struct Toast {
    id: i32,
    title: String,
    body: Option<String>,
    sound: Option<String>,
    deep_link: String,
}

impl Toast {
    fn of(id: i32, outgoing: &Outgoing) -> Self {
        Self {
            id,
            title: outgoing.title.clone(),
            body: outgoing.body.clone(),
            // Already resolved per platform by `NotificationPayload::render`, from the
            // event's sound switch and `Platform::system_sound` (ADR 0015 §5). PG-E1-21
            // owns whether those values are the right ones; this adapter passes the value
            // it is given, and a refused one degrades to a silent toast.
            sound: outgoing.sound.clone(),
            deep_link: outgoing.deep_link.clone(),
        }
    }
}

/// The port's failure for a plugin error, so the service records a reason it understands.
///
/// The plugin has one desktop error — an I/O failure while preparing the payload — and
/// neither it nor the OS reports a refused or missing notification server, so this is the
/// only failure the adapter can observe. The message is the plugin's own text; it names
/// the operation that failed, not any path of ours.
fn refusal(error: &tauri_plugin_notification::Error) -> NotifyError {
    NotifyError::Refused {
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peregrine_domain::notify::Platform;
    use serde_json::json;

    fn mock() -> tauri::App<tauri::test::MockRuntime> {
        let app = tauri::test::mock_builder()
            .plugin(tauri_plugin_notification::init())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("a mock application");
        app.manage(crate::external::Delivery::default());
        app
    }

    fn outgoing(title: &str, deep_link: &str, sound: Option<&str>) -> Outgoing {
        Outgoing {
            notification_id: 7,
            title: title.to_owned(),
            body: Some("Open the queue to see them".to_owned()),
            deep_link: deep_link.to_owned(),
            sound: sound.map(str::to_owned),
        }
    }

    /// A click is the same input an external launch is, so it takes the same route: the
    /// link reaches the delivery state as the canonical link this application built.
    #[test]
    fn a_click_routes_through_the_external_link_router() {
        let app = mock();
        let performed: ActionPerformed = serde_json::from_value(json!({
            "actionId": "tap",
            "inputValue": null,
            "notification": {
                "id": 1,
                "extra": { DEEP_LINK_KEY: "peregrine://title/tmdb/123" },
            },
        }))
        .expect("the plugin's own payload shape");

        route_click(app.handle(), &performed);

        assert_eq!(
            app.state::<crate::external::Delivery>()
                .pending()
                .as_deref(),
            Some("peregrine://title/tmdb/123"),
            "the click was held as a deep link the window has not yet seen"
        );
    }

    /// A link the grammar refuses is refused here too, and never reaches the webview: the
    /// click path must not be a way around the router's checks.
    #[test]
    fn a_click_carrying_a_refused_link_reaches_nothing() {
        for hostile in [
            "peregrine://magnet?xt=urn:btih:0123456789abcdef0123456789abcdef01234567",
            "javascript:alert(1)",
            "peregrine://nope",
        ] {
            let app = mock();
            let performed: ActionPerformed = serde_json::from_value(json!({
                "actionId": "tap",
                "inputValue": null,
                "notification": { "id": 1, "extra": { DEEP_LINK_KEY: hostile } },
            }))
            .expect("the plugin's own payload shape");

            route_click(app.handle(), &performed);

            assert_eq!(
                app.state::<crate::external::Delivery>().pending(),
                None,
                "{hostile} must not become an event payload"
            );
        }
    }

    /// Nothing routes a click that is not a body click, or that carries no link.
    #[test]
    fn a_click_without_a_link_or_with_another_action_routes_nothing() {
        let app = mock();
        for payload in [
            json!({
                "actionId": "tap",
                "inputValue": null,
                "notification": { "id": 1, "extra": {} },
            }),
            json!({
                "actionId": "snooze",
                "inputValue": null,
                "notification": { "id": 1, "extra": { DEEP_LINK_KEY: "peregrine://downloads" } },
            }),
            json!({ "actionId": "tap", "inputValue": null }),
            json!({
                "actionId": "tap",
                "inputValue": null,
                "notification": { "id": 1, "extra": { DEEP_LINK_KEY: 7 } },
            }),
        ] {
            let performed: ActionPerformed =
                serde_json::from_value(payload).expect("the plugin's own payload shape");
            route_click(app.handle(), &performed);
            assert_eq!(app.state::<crate::external::Delivery>().pending(), None);
        }
    }

    /// The handler registers against a real plugin instance without touching the OS.
    #[test]
    fn the_click_handler_registers_at_startup() {
        let app = mock();
        register_click_handler(app.handle()).expect("the plugin always accepts a handler");
    }

    /// What the OS is handed is what the port carried: the resolved sound, the deep link
    /// in `extra`'s slot, and the optional body.
    #[test]
    fn a_toast_carries_the_resolved_sound_the_body_and_the_deep_link() {
        let toast = Toast::of(
            7,
            &outgoing("Download finished", "peregrine://downloads", Some("Ping")),
        );
        assert_eq!(toast.id, 7);
        assert_eq!(toast.title, "Download finished");
        assert_eq!(toast.body.as_deref(), Some("Open the queue to see them"));
        assert_eq!(toast.sound.as_deref(), Some("Ping"));
        assert_eq!(toast.deep_link, "peregrine://downloads");

        let silent = Toast::of(
            8,
            &outgoing("Library scan finished", "peregrine://notifications", None),
        );
        assert_eq!(
            silent.sound, None,
            "a silent event sends no sound value rather than an empty one"
        );
    }

    /// The counter is consecutive, survives a restart, and never issues an id the plugin
    /// cannot hold.
    #[test]
    fn the_counter_is_consecutive_and_survives_a_restart() {
        let directory = tempfile::tempdir().expect("a temporary data directory");
        let path = directory.path().join(ID_COUNTER_FILE);

        let ids = NativeIds::open(path.clone());
        assert_eq!(ids.next(), 1);
        assert_eq!(ids.next(), 2);
        assert_eq!(ids.next(), 3);

        let reopened = NativeIds::open(path.clone());
        assert_eq!(reopened.next(), 4, "a restart continues the sequence");
        assert_eq!(
            std::fs::read_to_string(&path)
                .expect("the counter is written")
                .trim(),
            "4"
        );
    }

    /// At the top of the range the counter wraps to the first id, and 0 is never issued —
    /// which is what lets an absent file mean "nothing yet".
    #[test]
    fn the_counter_wraps_and_never_issues_zero() {
        let directory = tempfile::tempdir().expect("a temporary data directory");
        let path = directory.path().join(ID_COUNTER_FILE);
        std::fs::write(&path, i32::MAX.to_string()).expect("the fixture writes");

        let ids = NativeIds::open(path.clone());
        assert_eq!(ids.next(), FIRST_ID, "the wrap goes back to the first id");
        assert_eq!(ids.next(), FIRST_ID + 1, "and then counts on");
        assert_ne!(FIRST_ID, 0);
    }

    /// An absent, empty, negative, out-of-range, or unparsable counter starts the sequence
    /// rather than failing a notification.
    #[test]
    fn an_unusable_counter_starts_the_sequence() {
        for fixture in [
            None,
            Some(""),
            Some("banana"),
            Some("-5"),
            Some("0"),
            Some("99999999999"),
        ] {
            let directory = tempfile::tempdir().expect("a temporary data directory");
            let path = directory.path().join(ID_COUNTER_FILE);
            if let Some(text) = fixture {
                std::fs::write(&path, text).expect("the fixture writes");
            }
            let ids = NativeIds::open(path);
            assert_eq!(ids.next(), FIRST_ID, "fixture {fixture:?}");
        }
    }

    /// A plugin error reaches the port as a refusal the service can record, with the
    /// plugin's own sentence and no path of ours in it.
    #[test]
    fn a_plugin_error_becomes_a_recordable_refusal() {
        let error = tauri_plugin_notification::Error::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "the running executable could not be resolved",
        ));
        let refusal = refusal(&error);
        match &refusal {
            NotifyError::Refused { message } => {
                assert!(message.contains("the running executable could not be resolved"));
            }
            other @ NotifyError::Unavailable => {
                panic!("a preparation failure is not {other:?}")
            }
        }
        assert_eq!(
            refusal.to_string(),
            "the notification server refused the toast: the running executable could not be resolved"
        );
    }

    /// The event's sound switch is what decides whether the plugin is given a sound at all,
    /// and the value it is given is the one the domain resolved for this platform. PG-E1-21
    /// owns whether that value is the right one; this adapter holds no second table.
    #[test]
    fn the_event_s_sound_switch_decides_whether_a_sound_is_sent() {
        let sound = Platform::current().system_sound();
        assert_eq!(
            Toast::of(
                1,
                &outgoing("Download finished", "peregrine://downloads", Some(sound)),
            )
            .sound
            .as_deref(),
            Some(sound),
            "switch on: the platform's resolved sound reaches the toast"
        );
        assert_eq!(
            Toast::of(
                2,
                &outgoing("Download finished", "peregrine://downloads", None)
            )
            .sound,
            None,
            "switch off: no sound value is sent, so the toast is silent"
        );
    }

    /// A permission the OS has not granted is the port's failure, not a crash and not a
    /// prompt: the service records it on the row the centre already holds.
    #[test]
    fn a_permission_refusal_is_reported_rather_than_prompted_for() {
        assert!(
            permission_decision(PermissionState::Granted).is_ok(),
            "a granted permission is not a failure"
        );
        for state in [
            PermissionState::Denied,
            PermissionState::Prompt,
            PermissionState::PromptWithRationale,
        ] {
            match permission_decision(state) {
                Err(NotifyError::Refused { message }) => assert_eq!(
                    message, "the system has not granted permission to show notifications",
                    "{state:?} must be a recordable refusal"
                ),
                other => panic!("{state:?} must be refused, got {other:?}"),
            }
        }
    }

    /// The platform's own sound value is what the domain resolved; this adapter has no
    /// second table that could drift from it.
    #[test]
    fn the_sound_value_comes_from_the_domain_and_not_a_second_table() {
        let platform = Platform::current();
        let toast = Toast::of(
            1,
            &outgoing("x", "peregrine://downloads", Some(platform.system_sound())),
        );
        assert_eq!(toast.sound.as_deref(), Some(platform.system_sound()));
    }
}
