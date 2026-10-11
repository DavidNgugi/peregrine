#![forbid(unsafe_code)]
//! The services that compose the ports into use cases: library, discovery,
//! acquisition, playback, subtitles, settings, casting, notifications, accounts,
//! and maintenance.
//!
//! This is the only place where multiple ports are combined.

pub mod events;
pub mod jobs;
pub mod notify;
mod observability;
pub mod runtime;
#[cfg(test)]
mod testing;

pub use events::{Event, EventBus, EventSink, EventSinkError, EventStats, Topic};
pub use notify::{
    CentreCursor, CentreEntry, CentreError, Delivery, EventPreference, NewCentreEntry,
    NotificationCentre, NotificationId, NotificationPreferences, NotificationService, Outcome,
    Preferences, PreferencesError, QuietHours, RecentNotification, RecentQuery, Recorded,
    ServiceError,
};
pub use observability::{LogConfig, Observability, ObservabilityError};
pub use runtime::{Health, Runtime, RuntimeConfig, RuntimeError, StartedJob};
