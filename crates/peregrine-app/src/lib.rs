#![forbid(unsafe_code)]
//! The services that compose the ports into use cases: library, discovery,
//! acquisition, playback, subtitles, settings, casting, notifications, accounts,
//! and maintenance.
//!
//! This is the only place where multiple ports are combined.

pub mod events;
pub mod jobs;
pub mod library;
pub mod notify;
mod observability;
pub mod providers;
pub mod runtime;
#[cfg(test)]
mod testing;

pub use events::{Event, EventBus, EventSink, EventSinkError, EventStats, Topic};
pub use library::{
    IndexError, LibraryError, LibraryService, MatchAs, ScanError, ScanOutcome, Scanner,
};
pub use notify::{
    CentreCursor, CentreEntry, CentreError, Delivery, DeliveryState, EventPreference,
    NewCentreEntry, NotificationCentre, NotificationId, NotificationPreferences,
    NotificationService, Outcome, Preferences, PreferencesError, QuietHours, RecentNotification,
    RecentQuery, RecentWindow, Recorded, ServiceError, SubjectKind, SubjectNaming,
};
pub use observability::{LogConfig, Observability, ObservabilityError};
// The runtime's own surface speaks these, so a caller that only uses `Runtime` — the IPC
// layer, which may depend on this crate and not on storage — can name what it is handed
// without a second dependency edge (AGENTS R6).
pub use peregrine_storage::{Cursor, JobPhase, JobRecord, PageRequest, StorageError};
pub use runtime::{Health, Runtime, RuntimeConfig, RuntimeError, ShellStatus, StartedJob};
// The command surface speaks these too, and `peregrine-ipc` may depend on this crate and not
// on `peregrine-domain` (AGENTS R6): re-exported rather than duplicated so a kind added to the
// domain cannot reach the wire in a form the domain does not have.
pub use peregrine_domain::{EpisodeNumber, LibraryRoot, LibraryRootKind, ScanMode, SeasonNumber};
pub use peregrine_storage::{
    LibraryTitleRow, MatchRuleRow, MetadataState, TitleEpisodeRow, TitleIdentity, TitleKind,
    UnmatchedFileRow,
};
// And the instant a root's fields carry, for the same reason: the command surface has to name
// the type it converts, and `jiff` is not a dependency it may take (AGENTS R6).
pub use jiff::Timestamp;
