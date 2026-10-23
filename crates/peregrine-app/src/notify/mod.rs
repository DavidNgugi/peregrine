//! The notification service: the in-app centre is the record, the OS toast is a
//! side effect.
//!
//! [ADR 0015](../../../docs/adr/0015-native-notifications.md) §1 and AGENTS R31
//! require the centre row to exist *before* any toast is attempted, because the
//! toast can fail in ways we cannot detect: no notification daemon on Linux, the
//! app muted in macOS System Settings, a click lost once a Windows toast leaves
//! the screen, and nothing retractable on either platform. The order of the calls
//! in [`service::NotificationService::notify`] is therefore the design, not an
//! implementation detail.
//!
//! What lives here: the settings the user owns ([`Preferences`] and its port), the
//! [`Outcome`] a caller reports, and the service that composes the ports. What lives in
//! `peregrine-domain`: the catalogue, the copy, the deep-link routes, the leak scanner, the
//! planner, and the centre's own port and rows. What lives in `peregrine-storage`: the
//! `notification` table's repository. What lives in the shell (`apps/desktop/src-tauri`): the
//! `OsNotifier` adapter that talks to `tauri-plugin-notification`, installed here through
//! [`install_os_notifier`] before the runtime composes the service.

pub mod service;

use std::sync::{Arc, OnceLock};

use jiff::Timestamp;
use jiff::civil::Time;
use peregrine_domain::notify::{
    EnableReason, EventSetting, NotificationEvent, NotifyError, OsNotifier, Outgoing,
    QuietHoursWindow, Severity,
};
use peregrine_domain::ports::Clock;
use peregrine_storage::Database;

pub use peregrine_domain::centre::{
    CentreCursor, CentreEntry, CentreError, Delivery, DeliveryState, NewCentreEntry,
    NotificationCentre, NotificationId, RecentNotification, RecentQuery, RecentWindow, Recorded,
    SubjectKind, SubjectNaming,
};
pub use peregrine_domain::notify::{Consent, ConsentDecision};
pub use service::{NotificationService, ServiceError};

/// The notification preferences the shipping runtime owns.
///
/// **Consent is the only field this stores, and that is the whole of what the Settings screen
/// writes today.** The other fields answer [`Preferences::default`], which is the ADR 0015 §3
/// catalogue; the settings registry that will own them is PG-E20-01's.
///
/// The consent lives in the existing `settings` table rather than in a new one, because it is one
/// answer to one question and the registry has not arrived to give the rest a home.
#[derive(Clone)]
pub struct StoredPreferences {
    database: Arc<Database>,
    clock: Arc<dyn Clock + Send + Sync>,
}

impl StoredPreferences {
    /// Builds the store over the database and the clock this process already holds.
    #[must_use]
    pub fn new(database: Arc<Database>, clock: Arc<dyn Clock + Send + Sync>) -> Self {
        Self { database, clock }
    }
}

impl NotificationPreferences for StoredPreferences {
    fn load(
        &self,
    ) -> impl std::future::Future<Output = Result<Preferences, PreferencesError>> + Send {
        let database = Arc::clone(&self.database);
        async move {
            let consent = database.notification_consent().await.map_err(|error| {
                tracing::warn!(%error, "the stored notification consent could not be read");
                PreferencesError::Unavailable {
                    message: "the notification consent could not be read".to_owned(),
                }
            })?;
            Ok(Preferences {
                consent,
                ..Preferences::default()
            })
        }
    }

    fn save(
        &self,
        preferences: &Preferences,
    ) -> impl std::future::Future<Output = Result<(), PreferencesError>> + Send {
        let database = Arc::clone(&self.database);
        let consent = preferences.consent;
        let at = self.clock.now();
        async move {
            database
                .set_notification_consent(consent, at)
                .await
                .map_err(|error| {
                    tracing::warn!(%error, "the notification consent could not be stored");
                    PreferencesError::Unavailable {
                        message: "the notification consent could not be stored".to_owned(),
                    }
                })
        }
    }
}

/// The OS notifier for a process with no adapter: tests, and a build without the plugin.
///
/// It reports the honest outcome — no server is reachable — rather than claiming a toast the
/// user never saw. The service reaches it when consent permits delivery and no shell adapter was
/// installed: a test process, or a build without the plugin. The shipping shell installs
/// `TauriOsNotifier`, so there the centre row is not the whole record.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoOsNotifier;

impl OsNotifier for NoOsNotifier {
    fn notify(&self, _notification: &Outgoing) -> Result<i32, NotifyError> {
        Err(NotifyError::Unavailable)
    }
}

/// The shell's OS notifier, installed once before the runtime starts.
///
/// The adapter names `tauri`, so it lives in the shell crate and reaches this process through
/// this slot rather than a field on [`crate::RuntimeConfig`]: the IPC layer composes that
/// configuration and may not name a Tauri type either (AGENTS R8). Set-once because a process
/// has one notification channel; a second install is ignored rather than replacing the first.
static INSTALLED_OS_NOTIFIER: OnceLock<Arc<dyn OsNotifier + Send + Sync>> = OnceLock::new();

/// Installs the shell's [`OsNotifier`] for this process.
///
/// Called from the shell's `setup`, before [`crate::Runtime::start`] composes the notification
/// service. It does not ask the OS for permission and cannot grant consent; the adapter it
/// receives reports a refusal on its own (AGENTS R31).
pub fn install_os_notifier(notifier: Arc<dyn OsNotifier + Send + Sync>) {
    let _ = INSTALLED_OS_NOTIFIER.set(notifier);
}

/// The OS notifier the runtime composes.
///
/// The installed adapter when [`install_os_notifier`] ran before startup, and [`NoOsNotifier`]
/// otherwise. A wrapper rather than a bare `Arc<dyn OsNotifier>` because the orphan rule does
/// not let the standard library's `Arc` implement the domain's port here.
pub struct InstalledOsNotifier(Arc<dyn OsNotifier + Send + Sync>);

impl InstalledOsNotifier {
    /// The installed adapter, or the placeholder when none was installed.
    #[must_use]
    pub fn installed() -> Self {
        Self(
            INSTALLED_OS_NOTIFIER
                .get()
                .cloned()
                .unwrap_or_else(|| Arc::new(NoOsNotifier)),
        )
    }
}

impl OsNotifier for InstalledOsNotifier {
    fn notify(&self, notification: &Outgoing) -> Result<i32, NotifyError> {
        self.0.notify(notification)
    }
}

/// The notifications the acquisition path raises, so a job does not have to name the
/// service's four generic ports.
///
/// The futures are boxed to keep the trait object-safe: a job holds `Arc<dyn NotifyAcquisition>`,
/// and an `impl Future` return would make that impossible.
pub trait NotifyAcquisition: Send + Sync {
    /// Records that a search found only alternatives to the preferred quality.
    fn offers_available<'a>(
        &'a self,
        scope: peregrine_domain::AccountScope,
        title_id: i64,
        label: Option<String>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>;

    /// Records that an acquire job for `title_id` ended in failure.
    ///
    /// The title is named because the job that found nothing wrote no download row: there
    /// is no other subject to open. A cancellation is not this event, so a caller that
    /// stopped at the person's request raises nothing.
    fn failed<'a>(
        &'a self,
        scope: peregrine_domain::AccountScope,
        title_id: i64,
        label: Option<String>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>;
}

/// Where the user's notification choices are stored.
///
/// The shipping implementation is [`StoredPreferences`], which carries the consent today and
/// leaves the per-event choices to the catalogue until PG-E20-01's settings registry owns them.
/// A test supplies its own store, because the service must not reach a database it was not given.
pub trait NotificationPreferences {
    /// Reads the current preferences, or the catalogue defaults when unset.
    fn load(
        &self,
    ) -> impl std::future::Future<Output = Result<Preferences, PreferencesError>> + Send;

    /// Replaces the stored preferences.
    fn save(
        &self,
        preferences: &Preferences,
    ) -> impl std::future::Future<Output = Result<(), PreferencesError>> + Send;
}

/// Why the preferences could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PreferencesError {
    /// The storage layer refused the operation.
    #[error("the notification preferences could not be reached: {message}")]
    Unavailable {
        /// The storage error, flattened.
        message: String,
    },
    /// A stored preference does not match the documented shape.
    #[error("stored notification preferences are not valid: {message}")]
    Corrupt {
        /// What was stored that the schema does not allow.
        message: String,
    },
}

/// The user's notification choices, with the catalogue defaults filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preferences {
    /// The user's answer to "may we notify you?", which is core state rather than a UI
    /// value: no renderer can lose it, and the planner reads it before every attempt.
    pub consent: Consent,
    /// Whether the master sound switch is on.
    pub sounds_enabled: bool,
    /// Whether subjects are replaced by a generic string.
    pub hide_titles: bool,
    /// The quiet-hours window, local civil time.
    pub quiet_hours: Option<QuietHours>,
    /// The per-event enable and sound rows.
    pub events: Vec<EventPreference>,
}

impl Default for Preferences {
    /// The defaults from ADR 0015 §3: the catalogue, with consent not yet given.
    fn default() -> Self {
        Self {
            // Declining is the default: we do not notify before we are asked.
            consent: Consent::UNASKED,
            sounds_enabled: true,
            hide_titles: false,
            quiet_hours: None,
            events: NotificationEvent::ALL
                .into_iter()
                .map(EventPreference::defaults)
                .collect(),
        }
    }
}

impl Preferences {
    /// The enable and sound state for one event.
    #[must_use]
    pub fn event(&self, event: NotificationEvent) -> Option<&EventPreference> {
        self.events.iter().find(|row| row.event == event)
    }

    /// Whether the app may ask the user about OS notifications, unprompted (PG-E1-20).
    #[must_use]
    pub const fn should_prompt(&self) -> bool {
        self.consent.should_prompt()
    }

    /// Whether the OS may be asked to show a toast under the stored consent.
    #[must_use]
    pub const fn permits_delivery(&self) -> bool {
        self.consent.permits_delivery()
    }

    /// The planner's view of one event.
    ///
    /// A missing row falls back to the catalogue rather than failing: an
    /// incomplete settings write must not silence a security event.
    #[must_use]
    pub fn setting(&self, event: NotificationEvent) -> EventSetting {
        match self.event(event) {
            Some(row) => EventSetting {
                event,
                // A critical event is on whatever the row says; the repository
                // refuses to persist `false` for one, and this is the second belt.
                enabled: row.enabled || event.severity() == Severity::Critical,
                reason: row.reason,
            },
            None => EventSetting {
                event,
                enabled: event.default_enabled(),
                reason: EnableReason::CatalogueDefault,
            },
        }
    }

    /// Whether the event's sound switch is on.
    #[must_use]
    pub fn sound(&self, event: NotificationEvent) -> bool {
        self.event(event).map_or_else(
            || event.default_sound(),
            |row| row.sound && self.sounds_enabled,
        )
    }
}

/// A window during which normal events are held back.
///
/// The same shape as the domain's [`QuietHoursWindow`], held by the settings
/// rather than by the policy: a civil time rather than an instant, because quiet
/// hours are a preference about the user's evening and storing one as a UTC
/// instant would place it in the wrong evening after a travel or a DST change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuietHours {
    /// The local time the window opens.
    pub start: Time,
    /// The local time the window closes.
    pub end: Time,
}

impl QuietHours {
    /// A window from `start` to `end`, wrapping midnight when `end` is earlier.
    #[must_use]
    pub const fn window(start: Time, end: Time) -> Self {
        Self { start, end }
    }

    /// The domain window the planner measures an instant against.
    #[must_use]
    pub const fn as_window(self) -> QuietHoursWindow {
        QuietHoursWindow::window(self.start, self.end)
    }

    /// Whether the window is open at an injected instant.
    #[must_use]
    pub fn contains(self, now: Timestamp) -> bool {
        self.as_window().is_open_at(now)
    }
}

/// One event's switches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventPreference {
    /// The event.
    pub event: NotificationEvent,
    /// Whether it may toast.
    pub enabled: bool,
    /// Whether it plays a sound.
    pub sound: bool,
    /// Why `enabled` has its value.
    pub reason: EnableReason,
}

impl EventPreference {
    /// The catalogue row from ADR 0015 §3.
    #[must_use]
    pub fn defaults(event: NotificationEvent) -> Self {
        Self {
            event,
            enabled: event.default_enabled(),
            sound: event.default_sound(),
            reason: EnableReason::CatalogueDefault,
        }
    }
}

/// What the service received, for callers that want to report it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The centre row the event became.
    pub id: NotificationId,
    /// Whether the row was new or refreshed an entry inside the dedupe window.
    pub recorded: Recorded,
    /// What happened to the OS attempt.
    pub delivery: Delivery,
    /// How many events this row stands for: always one for a single event, the burst count for
    /// a summary, and the number of held events for a quiet-hours digest.
    pub aggregated: u32,
}

impl Outcome {
    /// Whether the OS accepted a toast for this event.
    #[must_use]
    pub const fn delivered(&self) -> bool {
        matches!(self.delivery, Delivery::Delivered { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(hour: i8) -> Time {
        Time::new(hour, 0, 0, 0).expect("a valid hour")
    }

    fn ts(millis: i64) -> Timestamp {
        Timestamp::from_millisecond(millis).expect("in range")
    }

    #[test]
    fn the_default_is_to_ask_before_notifying() {
        let preferences = Preferences::default();
        assert_eq!(
            preferences.consent,
            Consent::UNASKED,
            "R31: the user is asked before we notify them"
        );
        assert!(
            !preferences.permits_delivery(),
            "an unasked user is not notified"
        );
        assert!(
            preferences.should_prompt(),
            "and is the one case the wizard may ask"
        );
        assert_eq!(
            preferences
                .setting(NotificationEvent::DownloadCompleted)
                .enabled,
            NotificationEvent::DownloadCompleted.default_enabled()
        );
        assert_eq!(preferences.events.len(), NotificationEvent::ALL.len());
    }

    #[test]
    fn a_stored_switch_cannot_turn_off_a_critical_event() {
        let mut preferences = Preferences {
            consent: Consent::granted(),
            ..Preferences::default()
        };
        if let Some(row) = preferences
            .events
            .iter_mut()
            .find(|row| row.event == NotificationEvent::TorrentBlocked)
        {
            row.enabled = false;
            row.reason = EnableReason::CannotDisable;
        }
        assert!(
            preferences
                .setting(NotificationEvent::TorrentBlocked)
                .enabled,
            "a security event is never disabled by a stored row"
        );
    }

    #[test]
    fn a_missing_row_falls_back_to_the_catalogue() {
        let preferences = Preferences {
            events: Vec::new(),
            ..Preferences::default()
        };
        assert_eq!(
            preferences.setting(NotificationEvent::DiskSpaceLow).enabled,
            NotificationEvent::DiskSpaceLow.default_enabled()
        );
    }

    #[test]
    fn quiet_hours_wrap_midnight() {
        let window = QuietHours::window(at(22), at(7));
        assert!(window.contains(ts(1_700_000_000_000)));
        let midday = Timestamp::from_millisecond(1_700_046_000_000).expect("in range");
        assert!(!window.contains(midday));
    }

    /// The shell is the only installer, and it is a different binary, so a process that
    /// composes the runtime without one keeps the honest no-server answer.
    #[test]
    fn an_uninstalled_os_channel_reports_unavailable() {
        let outgoing = Outgoing {
            notification_id: 1,
            title: "Download finished".to_owned(),
            body: None,
            deep_link: "peregrine://downloads".to_owned(),
            sound: None,
        };
        assert_eq!(
            InstalledOsNotifier::installed().notify(&outgoing),
            Err(NotifyError::Unavailable),
            "a build with no OS adapter reports the honest outcome rather than claiming a toast"
        );
    }

    #[test]
    fn the_master_sound_switch_gates_every_event() {
        let preferences = Preferences {
            sounds_enabled: false,
            ..Preferences::default()
        };
        for event in NotificationEvent::ALL {
            assert!(
                !preferences.sound(event),
                "{event:?} must be silent with the master switch off"
            );
        }
    }

    /// The consent is the one preference the Settings screen writes, and it survives a restart:
    /// a second store over the same database reads the answer back, while every other choice
    /// stays the catalogue default PG-E20-01 will replace.
    #[tokio::test]
    async fn a_stored_consent_survives_a_new_store() {
        let directory = tempfile::TempDir::new().expect("a temporary directory");
        let database = Arc::new(
            Database::open(directory.path().join("peregrine.sqlite"))
                .await
                .expect("the database opens"),
        );
        let store = StoredPreferences::new(
            Arc::clone(&database),
            Arc::new(peregrine_testkit::FakeClock::default()),
        );
        assert_eq!(
            store.load().await.expect("the default loads").consent,
            Consent::UNASKED
        );

        let granted = Consent {
            decision: ConsentDecision::Granted,
            dont_ask_again: true,
            os_granted: Some(true),
        };
        store
            .save(&Preferences {
                consent: granted,
                ..Preferences::default()
            })
            .await
            .expect("the consent is stored");

        let reloaded = StoredPreferences::new(
            Arc::clone(&database),
            Arc::new(peregrine_testkit::FakeClock::default()),
        )
        .load()
        .await
        .expect("the stored consent loads");
        assert_eq!(reloaded.consent, granted);
        assert_eq!(
            reloaded.sounds_enabled,
            Preferences::default().sounds_enabled,
            "the fields PG-E20-01 will own still answer the catalogue"
        );
        assert_eq!(reloaded.events.len(), Preferences::default().events.len());
    }
}
