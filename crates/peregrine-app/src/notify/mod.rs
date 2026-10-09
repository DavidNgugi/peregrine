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
//! What lives here: the two ports the service needs beyond `peregrine-domain`'s
//! pure policy ([`NotificationCentre`] and [`NotificationPreferences`]), the
//! preference shape, and the service that composes them. What lives in
//! `peregrine-domain`: the catalogue, the copy, the deep-link routes, the leak
//! scanner, and the decision function. What lives in `peregrine-platform`: the
//! `OsNotifier` adapter that talks to the notification server.

pub mod service;

use jiff::Timestamp;
use jiff::civil::Time;
use peregrine_domain::notify::{
    EnableReason, EventSetting, NotificationEvent, QuietHoursWindow, Severity, SuppressionReason,
};

pub use service::{NotificationService, ServiceError};

/// Where the in-app centre is stored.
///
/// ARCHITECTURE §5 puts the notification repository in `peregrine-storage` — the
/// repository is the implementation and the port is the core's — so this trait is
/// the core's half and `peregrine-storage` provides the other.
pub trait NotificationCentre {
    /// Writes one entry and reports what happened to it.
    ///
    /// The write happens before any OS delivery is attempted, so a process that
    /// dies between the two leaves a readable entry rather than a lost event.
    fn record(
        &self,
        entry: &NewCentreEntry,
    ) -> impl std::future::Future<Output = Result<Recorded, CentreError>> + Send;

    /// Lists centre entries, newest first, without their dismissed rows.
    fn list(
        &self,
        cursor: Option<CentreCursor>,
        limit: u32,
    ) -> impl std::future::Future<Output = Result<Vec<CentreEntry>, CentreError>> + Send;

    /// The windowed rows a policy decision needs.
    fn recent(
        &self,
        query: &RecentQuery,
    ) -> impl std::future::Future<Output = Result<Vec<RecentNotification>, CentreError>> + Send;

    /// The delivery state of one entry, or `None` when no such row exists.
    fn delivery(
        &self,
        id: NotificationId,
    ) -> impl std::future::Future<Output = Result<Option<Delivery>, CentreError>> + Send;

    /// Records the outcome of one OS delivery attempt.
    ///
    /// Returns whether the row was there to update: an entry dismissed while its
    /// toast was in flight must not resurrect itself.
    fn mark_delivered(
        &self,
        id: NotificationId,
        delivery: &Delivery,
    ) -> impl std::future::Future<Output = Result<bool, CentreError>> + Send;
}

/// Where the user's notification choices are stored.
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

/// The row identifier the notification table assigns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct NotificationId(pub i64);

/// A row to insert into the centre.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCentreEntry {
    /// The catalogue event's stable key.
    pub event: String,
    /// `normal` or `critical`.
    pub severity: &'static str,
    /// The scanned title.
    pub title: String,
    /// The scanned body.
    pub body: Option<String>,
    /// The scanned deep link, shared with the OS payload.
    pub deep_link: Option<String>,
    /// The subject's kind, when the event has one.
    pub subject_kind: Option<&'static str>,
    /// The subject's row id, when the event has one.
    pub subject_id: Option<i64>,
    /// The key repeated events coalesce on.
    pub dedupe_key: Option<String>,
    /// The key a burst summary groups on.
    pub group_key: Option<String>,
    /// The resolved sound value, when one was asked for.
    pub sound: Option<String>,
    /// What the OS attempt has ended as so far.
    pub delivery: Delivery,
    /// Unix milliseconds UTC, from the injected clock.
    pub created_at: i64,
}

/// What happened when the entry was recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorded {
    /// A new row was inserted.
    Inserted(NotificationId),
    /// A row inside the dedupe window was refreshed instead of duplicated.
    Updated(NotificationId),
}

impl Recorded {
    /// The row the caller should deliver, if any.
    #[must_use]
    pub const fn id(self) -> NotificationId {
        match self {
            Self::Inserted(id) | Self::Updated(id) => id,
        }
    }
}

/// One centre row, as the UI reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CentreEntry {
    /// The row identifier.
    pub id: NotificationId,
    /// The catalogue event's key.
    pub event: String,
    /// `normal` or `critical`.
    pub severity: String,
    /// The stored title.
    pub title: String,
    /// The stored body.
    pub body: Option<String>,
    /// The stored deep link, which is the same one the toast carried.
    pub deep_link: Option<String>,
    /// The delivery state.
    pub delivery: Delivery,
    /// When it was created, in Unix milliseconds UTC.
    pub created_at: i64,
    /// Whether the user has read it.
    pub read: bool,
    /// Whether the user dismissed it.
    pub dismissed: bool,
}

/// A keyset cursor: `(created_at, id)` of the last row a page ended on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CentreCursor {
    /// The last row's `created_at`.
    pub created_at: i64,
    /// The last row's id, which breaks ties between rows sharing a millisecond.
    pub id: NotificationId,
}

/// A row the planner counts, as the repository reads it.
///
/// Deliberately not [`CentreEntry`]: the planner needs the keys and the outcome,
/// not the copy, and a narrower read is a narrower thing to get wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentNotification {
    /// The row identifier.
    pub id: NotificationId,
    /// The dedupe key, when the row has one.
    pub dedupe_key: Option<String>,
    /// The group key, when the row has one.
    pub group_key: Option<String>,
    /// The delivery state.
    pub delivery: Delivery,
    /// When it was created, in Unix milliseconds UTC.
    pub created_at: i64,
}

/// The windowed rows a policy decision needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentQuery {
    /// The instant the windows are measured back from.
    pub now: i64,
    /// Count rows with this dedupe key inside `dedupe_window_millis`.
    pub dedupe_key: Option<String>,
    /// The dedupe window, from ADR 0015 §4.
    pub dedupe_window_millis: i64,
    /// Count rows with this group key inside `burst_window_millis`.
    pub group_key: Option<String>,
    /// The burst window, from ADR 0015 §4.
    pub burst_window_millis: i64,
    /// Count rows delivered inside `rate_limit_window_millis`.
    pub rate_limit_window_millis: i64,
}

/// The outcome of an OS delivery attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    /// Recorded, not yet attempted.
    Pending,
    /// The OS accepted the toast.
    Delivered {
        /// The plugin's int32 id.
        native_id: i32,
        /// When it was handed over, in Unix milliseconds UTC.
        at: i64,
    },
    /// Held back before any OS call, with the reason.
    Suppressed(SuppressionReason),
    /// The OS channel refused it, with the message.
    Failed(String),
}

/// Why the centre could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CentreError {
    /// The storage layer refused the operation.
    #[error("the notification centre could not be reached: {message}")]
    Unavailable {
        /// The storage error, flattened so no driver type crosses this port.
        message: String,
    },
    /// A stored row does not match the documented schema.
    #[error("a stored notification row is not valid: {message}")]
    Corrupt {
        /// What the row said that the schema does not allow.
        message: String,
    },
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
    /// Whether the user agreed to OS notifications at all.
    pub consented: bool,
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
            consented: false,
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
        assert!(
            !preferences.consented,
            "R31: the user is asked before we notify them"
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
            consented: true,
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
