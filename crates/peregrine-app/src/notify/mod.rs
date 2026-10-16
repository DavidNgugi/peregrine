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
//! `notification` table's repository. What lives in `peregrine-platform` and is **not written
//! yet**: the `OsNotifier` adapter that talks to the notification server, which PG-E1-14 owns.

pub mod service;

use jiff::Timestamp;
use jiff::civil::Time;
use peregrine_domain::notify::{
    EnableReason, EventSetting, NotificationEvent, NotifyError, OsNotifier, Outgoing,
    QuietHoursWindow, Severity,
};

pub use peregrine_domain::centre::{
    CentreCursor, CentreEntry, CentreError, Delivery, DeliveryState, NewCentreEntry,
    NotificationCentre, NotificationId, RecentNotification, RecentQuery, RecentWindow, Recorded,
    SubjectKind, SubjectNaming,
};
pub use peregrine_domain::notify::{Consent, ConsentDecision};
pub use service::{NotificationService, ServiceError};

/// The preferences the shipping binary uses until \`PG-E20-01\`'s settings registry exists.
///
/// It answers the catalogue defaults and stores nothing. \`Preferences::default\` carries
/// \`Consent::UNASKED\`, so no toast is delivered while the centre row is still written — which
/// is R31's order and ADR 0015 §1's. Replacing it with a real store is \`PG-E20-01\`'s job;
/// writing through it is \`PG-E1-20\`'s.
#[derive(Debug, Clone, Copy, Default)]
pub struct CataloguePreferences;

impl NotificationPreferences for CataloguePreferences {
    fn load(
        &self,
    ) -> impl std::future::Future<Output = Result<Preferences, PreferencesError>> + Send {
        std::future::ready(Ok(Preferences::default()))
    }

    fn save(
        &self,
        _preferences: &Preferences,
    ) -> impl std::future::Future<Output = Result<(), PreferencesError>> + Send {
        // Nothing is stored yet, so a save is accepted and dropped rather than reported as a
        // failure: the only writers would be the wizard and permissions UI that do not exist.
        std::future::ready(Ok(()))
    }
}

/// The OS notifier until \`PG-E1-14\` writes the adapter that talks to the notification server.
///
/// It reports the honest outcome — no server is reachable — rather than claiming a toast the
/// user never saw. The service reaches it only when consent permits delivery, which the
/// catalogue defaults do not, so today the centre row is the whole record.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoOsNotifier;

impl OsNotifier for NoOsNotifier {
    fn notify(&self, _notification: &Outgoing) -> Result<i32, NotifyError> {
        Err(NotifyError::Unavailable)
    }
}

/// The one notification the acquisition path raises, so a job does not have to name the
/// service's four generic ports.
///
/// The future is boxed to keep the trait object-safe: a job holds \`Arc<dyn NotifyOffers>\`, and
/// an \`impl Future\` return would make that impossible.
pub trait NotifyOffers: Send + Sync {
    /// Records that a search found only alternatives to the preferred quality.
    fn offers_available<'a>(
        &'a self,
        scope: peregrine_domain::AccountScope,
        title_id: i64,
        label: Option<String>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>;
}

/// Where the user's notification choices are stored.
///
/// Not yet implemented, and deliberately not implemented here: the settings registry that
/// owns `settings` is PG-E20-01's, and the wizard and permissions UI that write through it
/// are PG-E1-20's. Until it exists the service is constructed with the test double in
/// `crates/peregrine-app/tests/notify.rs`, and nothing in the shipping binary constructs a
/// [`NotificationService`] yet.
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
}
