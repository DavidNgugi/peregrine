#![forbid(unsafe_code)]
//! The services that compose the ports into use cases: library, discovery,
//! acquisition, playback, subtitles, settings, casting, notifications, accounts,
//! and maintenance.
//!
//! This is the only place where multiple ports are combined.

pub mod artwork;
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
pub use runtime::{
    CredentialRequest, Health, HomeRail, LibraryTitleDetail, Runtime, RuntimeConfig, RuntimeError,
    ShellStatus, StartedJob,
};
// What one rail holds, for the reason the block above gives: the command surface answers a DTO
// built from a `RailItem`, and it may not reach into `peregrine-app`'s job module to name it.
pub use jobs::discover_rails::RailItem;
// The credential commands hand the runtime a candidate, and the wrapper that makes one safe to
// hold belongs to the secrets crate: re-exported for the reason above, so the command surface can
// name what it passes without a second dependency edge (AGENTS R6, SECURITY.md T13).
pub use peregrine_secrets::Secret;
// The command surface speaks these too, and `peregrine-ipc` may depend on this crate and not
// on `peregrine-domain` (AGENTS R6): re-exported rather than duplicated so a kind added to the
// domain cannot reach the wire in a form the domain does not have. `TitleSubtype` is here for the
// same completeness rather than for a command field: it is a public field of `TitleIdentity`, which
// this block already exports, so a caller that must build or destructure the row a command answers
// with would otherwise have a type it cannot name.
pub use peregrine_domain::{EpisodeNumber, LibraryRoot, LibraryRootKind, ScanMode, SeasonNumber};
pub use peregrine_storage::{
    LibraryTitleRow, MatchRuleRow, MetadataState, TitleEpisodeRow, TitleIdentity, TitleKind,
    TitleSubtype, UnmatchedFileRow,
};
// And the instant a root's fields carry, for the same reason: the command surface has to name
// the type it converts, and `jiff` is not a dependency it may take (AGENTS R6).
pub use jiff::Timestamp;

pub use peregrine_providers::error::ProviderError;
// The rail vocabulary, for the same reason: the command surface answers a DTO whose `rail` field
// is one of these, and a caller that had to reach for `peregrine-providers` to name it would be a
// second dependency edge for a string (AGENTS R6).
pub use peregrine_providers::provider::RailKind;
