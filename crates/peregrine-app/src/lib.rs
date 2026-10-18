#![forbid(unsafe_code)]
//! The services that compose the ports into use cases: library, discovery,
//! acquisition, playback, subtitles, settings, casting, notifications, accounts,
//! and maintenance.
//!
//! This is the only place where multiple ports are combined.

pub mod acquisition;
pub mod artwork;
pub mod events;
pub mod indexers;
pub mod jobs;
pub mod library;
mod local_posters;
mod media_source;
pub mod notify;
mod observability;
pub mod providers;
pub mod prowlarr;
pub mod runtime;
#[cfg(test)]
mod testing;
mod volume;

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
// The managed Prowlarr pack (docs/adr/0024): the component's own surface, re-exported so the
// command surface in the next slice can name what it starts without reaching into the module.
pub use prowlarr::{Installed, ProwlarrError, Supervisor, VERSION as PROWLARR_VERSION};
// The runtime's own surface speaks these, so a caller that only uses `Runtime` — the IPC
// layer, which may depend on this crate and not on storage — can name what it is handed
// without a second dependency edge (AGENTS R6).
pub use peregrine_storage::{
    Cursor, DownloadJobRow, JobPhase, JobRecord, PageRequest, StorageError, WatchProgressRow,
};
pub use runtime::{
    BrowsePage, CredentialRequest, Health, HomeRail, LibraryTitleDetail, LibraryTitleEntry,
    PlaybackSubtitle, ProwlarrStatus, RailsState, Recommendations, Runtime, RuntimeConfig,
    RuntimeError, SessionId, ShellStatus, StartedCapture, StartedJob, StartedPlayback,
    TitleEpisodeDetail, TitlePlayback, TitleSeasonDetail, WatchAvailabilityView, WatchOfferView,
};
// What one rail holds, for the reason the block above gives: the command surface answers a DTO
// built from a `RailItem`, and it may not reach into `peregrine-app`'s job module to name it.
pub use jobs::discover_rails::RailItem;
// The credential commands hand the runtime a candidate, and the wrapper that makes one safe to
// hold belongs to the secrets crate: re-exported for the reason above, so the command surface can
// name what it passes without a second dependency edge (AGENTS R6, SECURITY.md T13).
pub use peregrine_secrets::Secret;
// The store port travels with it, for the same reason and one more: a command test that starts a
// real runtime has to hand it a store, and the alternative is a test reading the developer's own
// keychain — which `AGENTS R14` forbids and which measured as a stalled read on macOS the first
// time a rebuilt binary asked (PG-E5-12).
pub use peregrine_secrets::{SecretKey, SecretStore, SecretStoreError};
// The debug build's file-backed store, for the shell to compose without a second dependency edge;
// release builds do not compile it.
#[cfg(debug_assertions)]
pub use peregrine_secrets::DevFileStore;
// The command surface speaks these too, and `peregrine-ipc` may depend on this crate and not
// on `peregrine-domain` (AGENTS R6): re-exported rather than duplicated so a kind added to the
// domain cannot reach the wire in a form the domain does not have. `TitleSubtype` is here for the
// same completeness rather than for a command field: it is a public field of `TitleIdentity`, which
// this block already exports, so a caller that must build or destructure the row a command answers
// with would otherwise have a type it cannot name.
pub use peregrine_domain::{
    DeletionPlan, EpisodeNumber, LibraryRoot, LibraryRootKind, LocalPlayback, PreferredQuality,
    Provider, ScanMode, SeasonNumber, TitleOffer, TrashError, WatchProgress,
};
// The notification centre's rows and severity, for the reason above: the commands that list the
// centre name them, and `peregrine-ipc` may not take a `peregrine-domain` edge (AGENTS R6).
pub use peregrine_domain::catalogue::{Region, WatchCategory};
pub use peregrine_domain::notify::Severity;
pub use peregrine_storage::{
    ArtworkPath, LibraryTitleRow, MatchRuleRow, MetadataState, TitleEpisodeRow, TitleIdentity,
    TitleKind, TitleSeasonRow, TitleSubtype, UnmatchedFileRow,
};
// And the instant a root's fields carry, for the same reason: the command surface has to name
// the type it converts, and `jiff` is not a dependency it may take (AGENTS R6).
pub use jiff::Timestamp;

pub use peregrine_providers::error::ProviderError;
// The indexer vocabulary, for the reason the provider one above gives: the command surface answers
// DTOs built from these and may not depend on `peregrine-indexers` itself (AGENTS R6).
pub use indexers::{DEFAULT_LIMIT, IndexerConfigError, IndexerStatus, MAX_LIMIT, PROWLARR};
pub use peregrine_indexers::{
    ConfiguredIndexer, IndexerDefinition, IndexerError, IndexerField, IndexerOption,
    IndexerRelease, NewIndexer, TorrentSource,
};
// The rail vocabulary, for the same reason: the command surface answers a DTO whose `rail` field
// is one of these, and a caller that had to reach for `peregrine-providers` to name it would be a
// second dependency edge for a string (AGENTS R6).
pub use peregrine_providers::provider::RailKind;
// The browse vocabulary, for the reason the rail one gives and one more: a browse *request* is
// built by the command surface out of a screen's filters, so the command has to name the same type
// the runtime takes rather than a second copy of it that could drift.
pub use peregrine_providers::provider::{BrowseRequest, BrowseSort, GenreRef, ProviderPage};

pub use peregrine_storage::{CastRow, DownloadScheduleRow, PersonRow};
