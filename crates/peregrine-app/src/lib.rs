#![forbid(unsafe_code)]
//! The services that compose the ports into use cases: library, discovery,
//! acquisition, playback, subtitles, settings, casting, notifications, accounts,
//! and maintenance.
//!
//! This is the only place where multiple ports are combined.

pub mod notify;
mod observability;

pub use notify::{
    CentreCursor, CentreEntry, CentreError, Delivery, EventPreference, NewCentreEntry,
    NotificationCentre, NotificationId, NotificationPreferences, NotificationService, Outcome,
    Preferences, PreferencesError, QuietHours, RecentNotification, RecentQuery, Recorded,
    ServiceError,
};
pub use observability::{LogConfig, Observability, ObservabilityError};
