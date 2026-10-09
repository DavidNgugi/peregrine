#![forbid(unsafe_code)]
//! Entities, value objects, invariants, quality scoring, release-name parsing,
//! subtitle timing and format models, state machines, and domain events.
//!
//! No IO, no async, no clock, and no randomness: every time and ID is injected, so
//! every rule here is testable without a database, a network, or a GUI.

pub mod id;
pub mod notify;
pub mod path;
pub mod ports;
pub mod quality;
pub mod redaction;
pub mod release;

pub use id::{ExternalId, ExternalIdError, Provider};
pub use notify::{
    BURST_SUMMARY_THRESHOLD, DEDUPE_WINDOW_MILLIS, DeepLink, DeepLinkError, EnableReason,
    EventSetting, Focus, MAX_DEEP_LINK_BYTES, MAX_TEXT_BYTES, NotificationEvent,
    NotificationPayload, NotificationText, NotifyError, OsNotifier, Outgoing, Payload, Planned,
    PlannerRequest, PlannerState, Platform, QuietHoursWindow, RATE_LIMIT_PER_MINUTE, Severity,
    SoundChoice, SoundError, SubjectLabel, SuppressionReason, burst_deep_link, deep_link,
    deep_link_for, plan, validate_custom_sound,
};
pub use path::{MAX_COMPONENT_BYTES, PathGuard, PathGuardError, SafePath};
pub use ports::{Clock, IdGenerator};
pub use quality::{
    GroupRules, QualityProfile, QualityProfileError, QualityRank, QualityRules, SizeCaps,
};
pub use redaction::{RedactionError, check_elements, check_text, is_safe};
pub use release::{
    AudioCodec, EpisodeSpan, HdrFormat, ReleaseName, ReleaseSource, RepackKind, Resolution,
    VideoCodec,
};
