#![forbid(unsafe_code)]
//! Entities, value objects, invariants, quality scoring, release-name parsing,
//! subtitle timing and format models, state machines, and domain events.
//!
//! No IO, no async, no clock, and no randomness: every time and ID is injected, so
//! every rule here is testable without a database, a network, or a GUI.
//!
//! [`AccountScope`] is the parameter every content read takes (AGENTS R34); the vocabulary
//! for what an account may see lives here so the UI can never compute it and no adapter can
//! invent its own.

pub mod account;
pub mod catalogue;
pub mod download;
pub mod id;
pub mod inspect;
pub mod library;
pub mod name;
pub mod notify;
pub mod ordering;
pub mod path;
pub mod ports;
pub mod quality;
pub mod rank;
pub mod redaction;
pub mod release;
pub mod subtitle;
pub mod text;
pub mod watch;

pub use account::{AccountError, AccountId, AccountScope, Visibility};
pub use catalogue::{
    AliasKind, CatalogueError, Company, CompanyRole, Credit, CreditKind, Episode, EpisodeNumber,
    EpisodeOrdering, EpisodePosition, EpisodeType, Genre, Keyword, MetadataState, OrderingScheme,
    Person, Season, SeasonNumber, Title, TitleAlias, TitleKind, TitleStatus, TitleSubtype,
};
pub use download::{DownloadEvent, DownloadState, TransitionRejected, transition};
pub use id::{ExternalId, ExternalIdError, Provider};
pub use inspect::{
    EntryKind, Finding, InspectionLimits, OptIn, Reason, SafetyReport, TorrentDescription,
    TorrentEntry, TorrentInspector, Verdict,
};
pub use library::{
    LibraryRoot, LibraryRootKind, MediaFile, MediaItem, MediaItemState, MediaOrigin,
    PlaybackPosition, ProgressSource, STORAGE_FLOOR_PERCENT, ScanMode, SmartCollection,
    StoredWatchProgress, SubtitleFormat, SubtitleOrigin, SubtitleOwner, SubtitleTrack,
    UnmatchedFile, WatchOrigin, WatchProgress, WatchProgressError, WatchlistEntry,
};
pub use name::{LibraryName, NameError, Numbering, UnresolvedReason};
pub use notify::{
    BURST_SUMMARY_THRESHOLD, DEDUPE_WINDOW_MILLIS, DeepLink, DeepLinkError, EnableReason,
    EventSetting, Focus, MAX_DEEP_LINK_BYTES, MAX_TEXT_BYTES, NotificationEvent,
    NotificationPayload, NotificationText, NotifyError, OsNotifier, Outgoing, Payload, Planned,
    PlannerRequest, PlannerState, Platform, QuietHoursWindow, RATE_LIMIT_PER_MINUTE, Severity,
    SoundChoice, SoundError, SubjectLabel, SuppressionReason, burst_deep_link, deep_link,
    deep_link_for, plan, validate_custom_sound,
};
pub use ordering::{
    DEFAULT_SCHEME, EpisodeOrderingStrategy, OrderedEpisodes, Unplaced, UnplacedReason,
};
pub use path::{MAX_COMPONENT_BYTES, PathGuard, PathGuardError, SafePath};
pub use ports::{Clock, IdGenerator};
pub use quality::{
    GroupRules, QualityProfile, QualityProfileError, QualityRank, QualityRules, SizeCaps,
};
pub use rank::{
    Candidate, MAX_SCORE, Position, RankReason, Ranking, ReasonKind, Rejection, ReleaseRanker,
};
pub use redaction::{RedactionError, check_elements, check_text, is_safe};
pub use release::{
    AudioCodec, EpisodeSpan, HdrFormat, ReleaseName, ReleaseSource, RepackKind, Resolution,
    VideoCodec,
};
pub use subtitle::{
    AssFields, Conversion, FormatError, FrameRate, MAX_MILLIS, MicrosPerFrame, Millis, SubtitleCue,
    SubtitleDocument, SubtitleTiming, TimingError,
};
pub use text::{NonEmptyText, TextError};
pub use watch::{
    EpisodePlacement, FinalizeOutcome, RESUME_PERCENT, RESUME_REWIND_MS, ResumePoint,
    WATCHED_PERCENT, finalize, is_watched, next_episode, resume_point,
};

/// Declares an enum that mirrors one `TEXT ... CHECK (col IN (...))` constraint.
///
/// The caller documents the variants; the macro supplies the three things every stored enum
/// owes the repositories: the database spelling, the complete list in the constraint's order,
/// and a parse that refuses a value this build does not know. Generating them together is
/// what makes forgetting the read half of the mapping — the half a new enum is most likely to
/// omit — impossible.
///
/// It is defined after the modules that use it so that they import it explicitly by path;
/// `macro_rules!` textual scope would otherwise make that import look unused.
macro_rules! stored_enum {
    (
        $(#[$enum_meta:meta])*
        $name:ident { $($(#[$variant_meta:meta])* $variant:ident => $text:literal),+ $(,)? }
    ) => {
        $(#[$enum_meta])*
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            Eq,
            Hash,
            ::serde::Serialize,
            ::serde::Deserialize,
        )]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($(#[$variant_meta])* $variant,)+
        }

        impl $name {
            /// Every variant, in the order the database constraint lists them.
            pub const ALL: &[Self] = &[$(Self::$variant),+];

            /// The value the `CHECK` constraint accepts.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)+
                }
            }

            /// Parses the stored value, or `None` for one this build does not know.
            #[must_use]
            pub fn parse(value: &str) -> Option<Self> {
                match value {
                    $($text => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}
pub(crate) use stored_enum;
