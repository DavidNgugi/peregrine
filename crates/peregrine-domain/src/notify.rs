//! The notification catalogue, the delivery policy, and the ports that keep the
//! OS at arm's length.
//!
//! [ADR 0015](../../../docs/adr/0015-native-notifications.md) is the contract and
//! AGENTS R31 is the rule; both say the same thing in different words: the in-app
//! centre is the record and the OS toast is a bonus, so the decision *not* to
//! toast is as much a part of this module as the decision to.
//!
//! Three things live here and nowhere else:
//!
//! - [`NotificationEvent`] is the closed catalogue. An event that is not in it
//!   cannot be notified, which is what keeps the copy auditable.
//! - [`NotificationPayload::render`] is the only way to build user-visible text,
//!   so every title, body, and deep link has passed [`crate::redaction`] by the
//!   time anything can store or emit it.
//! - [`plan`] is the policy: consent, per-event enable, quiet hours, dedupe,
//!   focus, burst coalescing, and the global rate limit, in one deterministic
//!   function over injected state.
//!
//! Every type here is pure. The clock, the identifiers, the stored rows, and the
//! OS are all injected by the service that composes this.

use jiff::Timestamp;
use jiff::civil::Time;
use thiserror::Error;

use crate::redaction::{self, RedactionError};

pub use crate::redaction::RedactionError as UnsafeContentError;

/// How long a title or body may be before it is refused.
///
/// A cap rather than a style guide: an unbounded body is how a log line or a
/// stack trace ends up on a lock screen.
pub const MAX_TEXT_BYTES: usize = 512;

/// The longest deep link accepted.
pub const MAX_DEEP_LINK_BYTES: usize = 256;

/// The dedupe window from ADR 0015 §4: a repeat inside it updates the entry.
pub const DEDUPE_WINDOW_MILLIS: i64 = 60_000;

/// The burst window from ADR 0015 §4.
pub const BURST_WINDOW_MILLIS: i64 = 120_000;

/// The number of completions inside [`BURST_WINDOW_MILLIS`] above which the
/// individual toasts are replaced by one summary.
pub const BURST_SUMMARY_THRESHOLD: usize = 3;

/// The global toast rate limit from ADR 0015 §4.
pub const RATE_LIMIT_PER_MINUTE: usize = 4;

/// The window the global rate limit counts over.
pub const RATE_LIMIT_WINDOW_MILLIS: i64 = 60_000;

/// The longest a subject label may be.
pub const MAX_SUBJECT_LABEL_BYTES: usize = 120;

/// The label used when the caller's subject label is refused by the scanner.
pub const GENERIC_SUBJECT_LABEL: &str = "A title";

/// The severity of a catalogue event.
///
/// Critical is about quiet hours, never about focus modes: a security or disk
/// event wakes the quiet-hours policy up, but it still does not override the OS's
/// Do Not Disturb, because the OS contract is not ours to break.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Ordinary traffic: may be held by quiet hours.
    Normal,
    /// Security or disk: exempt from quiet hours, never from focus modes.
    Critical,
}

impl Severity {
    /// The value the `notification.severity` constraint accepts.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Critical => "critical",
        }
    }

    /// Parses the stored value, or `None` for one this build does not know.
    ///
    /// The read half of the mapping, next to the write half: a `CHECK` makes an unknown
    /// value impossible, so a `None` here is a row written by another version.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "normal" => Some(Self::Normal),
            "critical" => Some(Self::Critical),
            _ => None,
        }
    }
}

/// The user's sound preference for one event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoundChoice {
    /// No sound hint is sent.
    None,
    /// The platform's system sound for this event class.
    Default,
    /// A user-supplied file, validated against the platform's extensions.
    Custom(String),
}

/// Where the process is running, as far as sound values care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// macOS: a named system sound, or an in-bundle file.
    MacOs,
    /// Windows: a path to a system `.wav`.
    Windows,
    /// Linux: an XDG theme sound name, or a path.
    Linux,
}

impl Platform {
    /// The platform this build targets.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }

    /// The default sound value for this platform, per ADR 0015 §5.
    ///
    /// These are the baseline values `SPIKE-11` verifies on real hardware; the
    /// adapter passes them to the plugin unchanged, and a wrong value degrades to
    /// a silent toast rather than a failed one.
    #[must_use]
    pub const fn system_sound(self) -> &'static str {
        match self {
            Self::MacOs => "Ping",
            Self::Windows => r"C:\Windows\Media\Windows Notify System Generic.wav",
            Self::Linux => "message-new-instant",
        }
    }
}

/// Why a custom sound file was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SoundError {
    /// The path is not one the platform's player accepts.
    #[error("`{path}` is not a sound file this platform accepts (expected one of: {expected})")]
    UnsupportedExtension {
        /// The refused path.
        path: String,
        /// The extensions this platform accepts, joined for the message.
        expected: String,
    },
}

/// Why a notification event is set the way it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnableReason {
    /// The user chose this value.
    UserChoice,
    /// The default from ADR 0015 §3.
    CatalogueDefault,
    /// A security outcome cannot be turned off.
    CannotDisable,
}

/// A catalogue event together with the user's enablement decision for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventSetting {
    /// The event.
    pub event: NotificationEvent,
    /// Whether the event may raise a toast at all.
    pub enabled: bool,
    /// Why it has that value.
    pub reason: EnableReason,
}

/// The closed event catalogue from ADR 0015 §3.
///
/// The key returned by [`NotificationEvent::as_str`] is what the `notification`
/// table stores and what the settings registry keys on, so it is stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationEvent {
    /// A download finished.
    DownloadCompleted,
    /// A download failed, including a stall past the retry limit.
    DownloadFailed,
    /// The safety gate blocked a torrent. Critical, and not disableable.
    TorrentBlocked,
    /// A watchlisted show has a new episode.
    NewEpisodeAvailable,
    /// Free disk space crossed the low threshold. Critical.
    DiskSpaceLow,
    /// An application update is available. Never installs.
    UpdateAvailable,
    /// A subtitle download finished.
    SubtitleDownloadFinished,
    /// A library scan or metadata sync finished.
    LibraryScanFinished,
    /// A cast session ended.
    CastSessionEnded,
    /// The quiet-hours roll-up of events that were held back.
    ///
    /// Not user-facing as a switch: it exists so the digest itself has a row in
    /// the centre with a key, exactly like every other delivery.
    NotificationDigest,
}

impl NotificationEvent {
    /// Every event, in the order ADR 0015 §3 lists them, with the internal
    /// digest last.
    pub const ALL: [Self; 10] = [
        Self::DownloadCompleted,
        Self::DownloadFailed,
        Self::TorrentBlocked,
        Self::NewEpisodeAvailable,
        Self::DiskSpaceLow,
        Self::UpdateAvailable,
        Self::SubtitleDownloadFinished,
        Self::LibraryScanFinished,
        Self::CastSessionEnded,
        Self::NotificationDigest,
    ];

    /// The stable key stored in `notification.event` and in settings.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DownloadCompleted => "download.completed",
            Self::DownloadFailed => "download.failed",
            Self::TorrentBlocked => "torrent.blocked",
            Self::NewEpisodeAvailable => "episode.available",
            Self::DiskSpaceLow => "disk.low",
            Self::UpdateAvailable => "update.available",
            Self::SubtitleDownloadFinished => "subtitle.downloaded",
            Self::LibraryScanFinished => "scan.finished",
            Self::CastSessionEnded => "cast.ended",
            Self::NotificationDigest => "notification.digest",
        }
    }

    /// Whether the event may toast before the user has decided anything.
    ///
    /// A security outcome is exempt from the master switch as well as quiet
    /// hours: silently blocking a torrent the user asked for is the failure this
    /// default exists to prevent.
    #[must_use]
    pub const fn default_enabled(self) -> bool {
        match self {
            Self::DownloadCompleted
            | Self::DownloadFailed
            | Self::TorrentBlocked
            | Self::NewEpisodeAvailable
            | Self::DiskSpaceLow
            | Self::UpdateAvailable => true,
            Self::SubtitleDownloadFinished
            | Self::LibraryScanFinished
            | Self::CastSessionEnded
            | Self::NotificationDigest => false,
        }
    }

    /// Whether the event plays a sound before the user has decided anything.
    #[must_use]
    pub const fn default_sound(self) -> bool {
        match self {
            Self::DownloadCompleted
            | Self::DownloadFailed
            | Self::TorrentBlocked
            | Self::DiskSpaceLow => true,
            Self::NewEpisodeAvailable
            | Self::UpdateAvailable
            | Self::SubtitleDownloadFinished
            | Self::LibraryScanFinished
            | Self::CastSessionEnded
            | Self::NotificationDigest => false,
        }
    }

    /// Whether a quiet-hours window holds the event back.
    ///
    /// A property of the event rather than of the settings: it is what makes
    /// [`PlannerState`] able to answer "does quiet hours apply" without a second
    /// boolean.
    #[must_use]
    pub const fn severity(self) -> Severity {
        match self {
            Self::TorrentBlocked | Self::DiskSpaceLow => Severity::Critical,
            _ => Severity::Normal,
        }
    }

    /// Whether the event is coalesced when several land together.
    #[must_use]
    pub const fn aggregates(self) -> bool {
        matches!(self, Self::DownloadCompleted | Self::NewEpisodeAvailable)
    }
}

/// A window during which normal events are held back.
///
/// `start > end` wraps midnight: `22:00..07:00` is one window, not two, and the
/// wrap is the case every other implementation gets wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuietHoursWindow {
    /// The local time the window opens.
    pub start: Time,
    /// The local time the window closes.
    pub end: Time,
}

impl QuietHoursWindow {
    /// A window from `start` to `end`, wrapping midnight when `end` is earlier.
    #[must_use]
    pub const fn window(start: Time, end: Time) -> Self {
        Self { start, end }
    }

    /// Whether `at` is inside the window.
    #[must_use]
    pub fn contains(self, at: Time) -> bool {
        if self.start <= self.end {
            at >= self.start && at < self.end
        } else {
            at >= self.start || at < self.end
        }
    }

    /// Whether the window is open at an injected instant, in UTC.
    ///
    /// UTC rather than the machine's zone on purpose: a quiet-hours window is a
    /// preference about the user's evening, and a rule whose answer depends on
    /// the developer's timezone is not testable. The zone the preference was
    /// written in belongs to the settings row (`PG-E20-02`), which will convert
    /// to the civil time before calling this.
    #[must_use]
    pub fn is_open_at(self, now: Timestamp) -> bool {
        self.contains(now.to_zoned(jiff::tz::TimeZone::UTC).time())
    }
}

/// The request a caller makes of the planner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannerRequest {
    /// The event that happened.
    pub event: NotificationEvent,
    /// The identifier of the thing it happened to, when it has one.
    pub subject_id: i64,
    /// How many rows share a dedupe key and are inside the window.
    pub recent_matching_keys: usize,
    /// How many rows were delivered to the OS inside the rate-limit window.
    pub delivered_in_window: usize,
    /// How many aggregated rows are inside the burst window.
    pub burst_size: usize,
}

/// Whether the app currently has the user's attention.
///
/// Two booleans rather than one because they are two facts: a focused window on
/// another screen is not the same as a focused window showing the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Focus {
    /// Whether any app window has focus.
    pub window: bool,
    /// Whether the screen the event is about is the one on top.
    pub already_on_screen: bool,
}

/// The user's standing choices, read from settings.
///
/// One struct rather than four loose booleans because a caller must not be able
/// to pass "consented" for one event and forget it for the next: the values are
/// read together by [`plan`] and belong together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannerState {
    /// Whether the user agreed to OS notifications at all.
    pub consented: bool,
    /// Whether the event's switch is on.
    pub enabled: bool,
    /// The quiet-hours window, and the instant it is measured at.
    pub quiet_hours: Option<(QuietHoursWindow, Timestamp)>,
    /// Whether the window has the user's attention.
    pub focus: Focus,
}

/// What the planner decided, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Planned {
    /// The centre gets the entry; the OS gets nothing, for this reason.
    CentreOnly {
        /// The reason recorded on the entry.
        reason: SuppressionReason,
    },
    /// The centre gets the entry and the OS gets a toast.
    Deliver,
    /// The centre gets one summary entry; the OS gets nothing.
    DeliverSummary,
}

/// Why an event did not reach the OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuppressionReason {
    /// The user turned the event off.
    Disabled,
    /// The user declined OS notifications.
    PermissionDenied,
    /// The quiet-hours window is open.
    QuietHours,
    /// The same event arrived inside the dedupe window.
    Duplicate,
    /// The window is focused on the screen the event is about.
    FocusSuppressed,
    /// The global toast rate limit is spent.
    RateLimited,
    /// The OS channel refused the toast before it was shown.
    DeliveryFailed,
}

impl SuppressionReason {
    /// The value the `notification.suppressed_reason` constraint accepts.
    ///
    /// `DeliveryFailed` is the one reason that is not a suppression: a failed
    /// delivery is recorded as a failure, so it maps to `None` and the caller
    /// writes `delivery_state = 'failed'` with the error beside it.
    #[must_use]
    pub const fn as_db_str(self) -> Option<&'static str> {
        match self {
            Self::Disabled => Some("disabled"),
            Self::PermissionDenied => Some("permission_denied"),
            Self::QuietHours => Some("quiet_hours"),
            Self::Duplicate => Some("duplicate"),
            Self::FocusSuppressed => Some("focus_suppressed"),
            Self::RateLimited => Some("rate_limited"),
            Self::DeliveryFailed => None,
        }
    }

    /// Parses the value the `notification.suppressed_reason` constraint accepts.
    ///
    /// `DeliveryFailed` has no spelling to parse and is therefore never returned: the
    /// database records a failed delivery as `delivery_state = 'failed'`.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "disabled" => Some(Self::Disabled),
            "permission_denied" => Some(Self::PermissionDenied),
            "quiet_hours" => Some(Self::QuietHours),
            "duplicate" => Some(Self::Duplicate),
            "focus_suppressed" => Some(Self::FocusSuppressed),
            "rate_limited" => Some(Self::RateLimited),
            _ => None,
        }
    }
}

/// The decision, in the order the reasons are checked.
///
/// The order is the policy: consent before enablement, quiet hours before
/// deduplication, focus before the rate limit, and the burst summary before the
/// limit so a burst is coalesced rather than silently spread across a minute.
#[must_use]
pub fn plan(request: &PlannerRequest, state: &PlannerState) -> Planned {
    let critical = request.event.severity() == Severity::Critical;
    if !state.consented {
        return Planned::CentreOnly {
            reason: SuppressionReason::PermissionDenied,
        };
    }
    if !state.enabled && !critical {
        return Planned::CentreOnly {
            reason: SuppressionReason::Disabled,
        };
    }
    if !critical
        && state
            .quiet_hours
            .is_some_and(|(window, now)| window.is_open_at(now))
    {
        return Planned::CentreOnly {
            reason: SuppressionReason::QuietHours,
        };
    }
    if request.recent_matching_keys > 0 {
        return Planned::CentreOnly {
            reason: SuppressionReason::Duplicate,
        };
    }
    if state.focus.window && state.focus.already_on_screen {
        return Planned::CentreOnly {
            reason: SuppressionReason::FocusSuppressed,
        };
    }
    if request.event.aggregates() && request.burst_size > BURST_SUMMARY_THRESHOLD {
        return Planned::DeliverSummary;
    }
    if request.delivered_in_window >= RATE_LIMIT_PER_MINUTE {
        return Planned::CentreOnly {
            reason: SuppressionReason::RateLimited,
        };
    }
    Planned::Deliver
}

/// A deep link the app can route on click.
///
/// The route set is closed, mirroring the ADR's examples. It exists so that the
/// `extra` payload and the centre's `deep_link` column cannot become a channel for
/// arbitrary text that the scanner would have to be trusted to catch: a caller
/// builds the route from identifiers, and an unknown route is a typed error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeepLink(String);

impl DeepLink {
    /// The link's text, which the centre row stores and the toast carries.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The authority of every link this app owns.
const DEEP_LINK_SCHEME: &str = "peregrine://";

/// The routes the click handler knows, per ADR 0015 §6.
const DEEP_LINK_ROUTES: [&str; 6] = [
    "downloads",
    "queue",
    "update",
    "notifications",
    "title",
    "settings",
];

/// Why a deep link was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DeepLinkError {
    /// The text is not a `peregrine://` link.
    #[error("`{link}` is not a peregrine:// deep link")]
    NotOwned {
        /// The refused link.
        link: String,
    },
    /// The route is not one the app routes on.
    #[error("`{route}` is not a route this app handles")]
    UnknownRoute {
        /// The refused route.
        route: String,
    },
    /// The link carries a query, a fragment, or an empty segment.
    #[error("a deep link must not carry a query, a fragment, or an empty segment")]
    NotCanonical,
    /// A segment is not a plain identifier.
    #[error("`{segment}` is not a route identifier")]
    NotAnIdentifier {
        /// The refused segment.
        segment: String,
    },
    /// The link failed the leak scan.
    #[error(transparent)]
    Redaction(#[from] RedactionError),
}

/// Builds a link from a closed route and 1-based segments.
pub fn deep_link(route: &str, segments: &[&str]) -> Result<DeepLink, DeepLinkError> {
    if !DEEP_LINK_ROUTES.contains(&route) {
        return Err(DeepLinkError::UnknownRoute {
            route: route.to_owned(),
        });
    }
    let expected = match route {
        "title" => 2,
        _ => 0,
    };
    if segments.len() != expected {
        return Err(DeepLinkError::NotCanonical);
    }
    let mut link = format!("{DEEP_LINK_SCHEME}{route}");
    for segment in segments {
        if segment.is_empty() || !segment.chars().all(is_identifier_character) {
            return Err(DeepLinkError::NotAnIdentifier {
                segment: (*segment).to_owned(),
            });
        }
        link.push('/');
        link.push_str(segment);
    }
    Ok(DeepLink(link))
}

/// The deep link for a notification about `subject_id` on `event`.
pub fn deep_link_for(event: NotificationEvent, subject_id: i64) -> Result<DeepLink, DeepLinkError> {
    let subject = subject_id.to_string();
    match event {
        NotificationEvent::DownloadCompleted | NotificationEvent::DownloadFailed => {
            deep_link("downloads", &[])
        }
        NotificationEvent::NewEpisodeAvailable | NotificationEvent::SubtitleDownloadFinished => {
            let id = subject.as_str();
            deep_link("title", &["tmdb", id])
        }
        NotificationEvent::UpdateAvailable => deep_link("update", &[]),
        NotificationEvent::DiskSpaceLow => deep_link("settings", &[]),
        NotificationEvent::CastSessionEnded => deep_link("queue", &[]),
        NotificationEvent::TorrentBlocked => deep_link("notifications", &[]),
        NotificationEvent::LibraryScanFinished | NotificationEvent::NotificationDigest => {
            deep_link("notifications", &[])
        }
    }
}

/// The deep link the burst summary's click opens: the queue, not five titles.
#[must_use]
pub fn burst_deep_link() -> DeepLink {
    // The route and segment count are constants, so this cannot fail at runtime;
    // the fallback exists only because `deep_link` returns a `Result`.
    deep_link("queue", &[]).unwrap_or_else(|_| DeepLink(format!("{DEEP_LINK_SCHEME}queue")))
}

fn is_identifier_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '-' || character == '_' || character == '.'
}

/// Text that has passed the leak scan.
///
/// The field is private and the only constructor scans, so a caller cannot hand a
/// raw `String` to anything that writes a row or builds an OS payload. That is the
/// structural half of R31's no-leak rule; the scanner is the other half.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationText(String);

impl NotificationText {
    /// Scans one string and keeps it, or names the rule it breaks.
    pub fn checked(text: &str, max_bytes: usize) -> Result<Self, RedactionError> {
        redaction::check_text(text, max_bytes)?;
        Ok(Self(text.to_owned()))
    }

    /// The scanned text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A subject label from outside the catalogue — an overview title, a release
/// name, a file name — which is exactly the kind of content R32 labels untrusted.
///
/// It is scanned before use and dropped rather than fatal afterwards, because the
/// label is decoration: "Download finished" is still the notification the user
/// asked for, and refusing the event would hide the download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectLabel(String);

impl SubjectLabel {
    /// Scans a label, returning `None` when it is empty or unsafe.
    #[must_use]
    pub fn sanitized(label: &str) -> Option<Self> {
        if redaction::is_safe(label, MAX_SUBJECT_LABEL_BYTES) {
            Some(Self(label.to_owned()))
        } else {
            None
        }
    }

    /// The scanned label.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The scanned copy of one event, shared by the centre row and the OS payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload {
    /// The scanned title.
    pub(crate) title: NotificationText,
    /// The scanned body, when the event has one.
    pub(crate) body: Option<NotificationText>,
    /// The route the click opens.
    pub(crate) deep_link: DeepLink,
    /// The sound resolved for the platform, when one was asked for.
    pub(crate) sound: Option<String>,
    /// How many events the payload stands for; 1 for a single event.
    pub(crate) aggregated: u32,
}

impl Payload {
    /// The title to show.
    #[must_use]
    pub fn title(&self) -> &str {
        self.title.as_str()
    }

    /// The scanned title itself, which is the only form a centre row accepts.
    #[must_use]
    pub const fn title_text(&self) -> &NotificationText {
        &self.title
    }

    /// The body to show, when there is one.
    #[must_use]
    pub fn body(&self) -> Option<&str> {
        self.body.as_ref().map(NotificationText::as_str)
    }

    /// The scanned body itself, when there is one.
    #[must_use]
    pub const fn body_text(&self) -> Option<&NotificationText> {
        self.body.as_ref()
    }

    /// The deep link's text, which the centre row stores and the toast carries.
    #[must_use]
    pub fn deep_link(&self) -> &str {
        self.deep_link.as_str()
    }

    /// The scanned deep link itself, which is the only form a centre row accepts.
    #[must_use]
    pub const fn deep_link_value(&self) -> &DeepLink {
        &self.deep_link
    }

    /// The resolved sound value, when one was asked for.
    #[must_use]
    pub fn sound(&self) -> Option<&str> {
        self.sound.as_deref()
    }

    /// How many events this payload stands for.
    #[must_use]
    pub const fn aggregated(&self) -> u32 {
        self.aggregated
    }
}

/// What the caller must tell the catalogue for it to render an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationPayload {
    /// The event.
    pub event: NotificationEvent,
    /// The identifier of the thing it happened to.
    pub subject_id: i64,
    /// An untrusted, optional label such as a media title.
    pub subject_label: Option<String>,
    /// How many completions a burst summary stands for.
    pub burst_size: u32,
    /// Whether the user asked for titles to be hidden.
    pub hide_titles: bool,
    /// Whether the event's sound switch is on.
    pub sound_enabled: bool,
    /// The platform the sound value is for.
    pub platform: Platform,
    /// A custom sound file, already validated against `platform`.
    pub custom_sound: Option<String>,
}

impl NotificationPayload {
    /// Renders the event, scanning every string and resolving the sound value.
    ///
    /// Refusing is deliberate rather than repairing: a parameter that looks like a
    /// path, a secret, or a magnet is a caller bug, and a notification is not
    /// worth guessing about.
    ///
    /// # Errors
    ///
    /// [`RedactionError`] names the rule the caller's data broke. A refused event
    /// is never stored and never emitted; the caller reports the bug.
    pub fn render(&self) -> Result<Payload, RedactionError> {
        let (title, body) = self.copy();
        let title = NotificationText::checked(&title, MAX_TEXT_BYTES)?;
        let body = match body {
            Some(body) => Some(NotificationText::checked(&body, MAX_TEXT_BYTES)?),
            None => None,
        };
        // A roll-up of several events is not about one of them, so its click cannot open one of
        // them: ADR 0015 §4 sends a burst summary to the queue. A single event keeps its own
        // route, and the digest — which is not an aggregation, however many events it stands
        // for — keeps the centre's.
        let deep_link = if self.event.aggregates() && self.burst_size > 1 {
            burst_deep_link()
        } else {
            deep_link_for(self.event, self.subject_id).map_err(|error| match error {
                // An unsafe link and an unknown route are the same failure from the
                // caller's side: the notification is abandoned rather than sent with a
                // destination that cannot be trusted.
                DeepLinkError::Redaction(inner) => inner,
                other @ (DeepLinkError::NotOwned { .. }
                | DeepLinkError::UnknownRoute { .. }
                | DeepLinkError::NotCanonical
                | DeepLinkError::NotAnIdentifier { .. }) => RedactionError::UnsafeLink {
                    reason: other.to_string(),
                },
            })?
        };
        let sound = self.resolve_sound();
        Ok(Payload {
            title,
            body,
            deep_link,
            sound,
            aggregated: self.burst_size,
        })
    }

    /// The unscanned copy for this event.
    fn copy(&self) -> (String, Option<String>) {
        match self.event {
            NotificationEvent::DownloadCompleted => {
                if self.burst_size > 1 {
                    (
                        format!("{} downloads finished", self.burst_size),
                        Some("Open the queue to see them".to_owned()),
                    )
                } else {
                    (
                        self.subject("Download finished"),
                        Some("Open the library to see it".to_owned()),
                    )
                }
            }
            NotificationEvent::DownloadFailed => (
                self.subject("Download failed"),
                Some("The queue has the reason".to_owned()),
            ),
            NotificationEvent::TorrentBlocked => (
                "Torrent blocked by the safety check".to_owned(),
                Some("Open notifications to review it".to_owned()),
            ),
            NotificationEvent::NewEpisodeAvailable => (
                self.subject("New episode available"),
                Some("Batched for the evening".to_owned()),
            ),
            NotificationEvent::DiskSpaceLow => (
                "Disk space is low".to_owned(),
                Some("Free space or move the library".to_owned()),
            ),
            NotificationEvent::UpdateAvailable => (
                "An update is available".to_owned(),
                Some("Open the update prompt to install it".to_owned()),
            ),
            NotificationEvent::SubtitleDownloadFinished => (
                self.subject("Subtitle downloaded"),
                Some("It is ready in the player".to_owned()),
            ),
            NotificationEvent::LibraryScanFinished => (
                "Library scan finished".to_owned(),
                Some("Open the library to see what changed".to_owned()),
            ),
            NotificationEvent::CastSessionEnded => (
                "Cast session ended".to_owned(),
                Some("Open the queue to continue".to_owned()),
            ),
            NotificationEvent::NotificationDigest => (
                "While you were away".to_owned(),
                Some("Open notifications to read them".to_owned()),
            ),
        }
    }

    /// The subject label when it is present, useful, and allowed to be shown.
    fn subject(&self, fallback: &str) -> String {
        if self.hide_titles {
            return fallback.to_owned();
        }
        let Some(label) = self.subject_label.as_deref() else {
            return fallback.to_owned();
        };
        match SubjectLabel::sanitized(label) {
            Some(label) => label.as_str().to_owned(),
            None => GENERIC_SUBJECT_LABEL.to_owned(),
        }
    }

    /// The sound value for this event on this platform.
    fn resolve_sound(&self) -> Option<String> {
        if !self.sound_enabled {
            return None;
        }
        if let Some(path) = &self.custom_sound
            && validate_custom_sound(path, self.platform).is_ok()
        {
            return Some(path.clone());
        }
        Some(self.platform.system_sound().to_owned())
    }
}

/// Validates a user-supplied sound file against the platform's extensions.
///
/// # Errors
///
/// [`SoundError::UnsupportedExtension`] when the extension is not one this
/// platform's player accepts; the caller falls back to the system sound.
pub fn validate_custom_sound(path: &str, platform: Platform) -> Result<(), SoundError> {
    let accepted: &[&str] = match platform {
        Platform::Windows => &["wav"],
        Platform::MacOs | Platform::Linux => &["wav", "aiff", "caf", "oga", "ogg"],
    };
    let extension = path
        .rsplit('.')
        .next()
        .filter(|extension| !extension.is_empty() && *extension != path)
        .map(str::to_lowercase);
    if extension
        .as_deref()
        .is_some_and(|extension| accepted.contains(&extension))
    {
        Ok(())
    } else {
        Err(SoundError::UnsupportedExtension {
            path: path.to_owned(),
            expected: accepted.join(", "),
        })
    }
}

/// The call an OS channel receives. Content that has already been scanned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    /// The row this toast belongs to, which becomes the plugin's int32 id.
    pub notification_id: i64,
    /// The scanned title.
    pub title: String,
    /// The scanned body.
    pub body: Option<String>,
    /// The scanned deep link.
    pub deep_link: String,
    /// The resolved sound, or `None` for a silent toast.
    pub sound: Option<String>,
}

impl Outgoing {
    /// Builds the OS call from a stored id and a scanned payload.
    #[must_use]
    pub fn from_payload(notification_id: i64, payload: &Payload) -> Self {
        Self {
            notification_id,
            title: payload.title().to_owned(),
            body: payload.body().map(str::to_owned),
            deep_link: payload.deep_link().to_owned(),
            sound: payload.sound().map(str::to_owned),
        }
    }
}

/// The OS notification channel.
///
/// The port lives here, beside [`crate::ports::Clock`], for the reason
/// ARCHITECTURE §5 gives: `peregrine-platform` owns "notifications" and may depend
/// on `peregrine-domain` only, so the trait has to be visible from the crate that
/// implements it. The adapter is a leaf; the decision belongs to the service.
pub trait OsNotifier {
    /// Shows one toast and reports the native id the OS assigned.
    ///
    /// # Errors
    ///
    /// The error is reported, not fatal: the centre row already exists, and a
    /// missing notification daemon is a normal outcome on Linux.
    fn notify(&self, notification: &Outgoing) -> Result<i32, NotifyError>;
}

/// Why a toast could not be handed to the OS.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NotifyError {
    /// No notification server is reachable.
    #[error("no notification server is reachable on this system")]
    Unavailable,
    /// The OS refused the toast.
    #[error("the notification server refused the toast: {message}")]
    Refused {
        /// The server's message.
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(millis: i64) -> Timestamp {
        Timestamp::from_millisecond(millis).expect("a millisecond inside the representable range")
    }

    fn state() -> PlannerState {
        PlannerState {
            consented: true,
            enabled: true,
            quiet_hours: None,
            focus: Focus::default(),
        }
    }

    fn request(event: NotificationEvent) -> PlannerRequest {
        PlannerRequest {
            event,
            subject_id: 7,
            recent_matching_keys: 0,
            delivered_in_window: 0,
            burst_size: 1,
        }
    }

    fn at(hour: i8) -> Time {
        Time::new(hour, 0, 0, 0).expect("a valid hour")
    }

    #[test]
    fn every_event_has_a_key_a_default_and_a_severity() {
        for event in NotificationEvent::ALL {
            assert!(
                !event.as_str().is_empty() && event.as_str().contains('.'),
                "{event:?} needs a namespaced key"
            );
        }
        assert_eq!(
            NotificationEvent::TorrentBlocked.severity(),
            Severity::Critical
        );
        assert_eq!(
            NotificationEvent::DiskSpaceLow.severity(),
            Severity::Critical
        );
        assert_eq!(
            NotificationEvent::DownloadCompleted.severity(),
            Severity::Normal
        );
        for event in NotificationEvent::ALL {
            if event.severity() == Severity::Critical {
                assert!(
                    event.default_enabled(),
                    "a critical event cannot be defaulted off"
                );
            }
        }
    }

    #[test]
    fn a_declined_user_is_not_notified_but_the_event_is_still_planned() {
        let mut state = state();
        state.consented = false;
        assert_eq!(
            plan(&request(NotificationEvent::DownloadCompleted), &state),
            Planned::CentreOnly {
                reason: SuppressionReason::PermissionDenied
            }
        );
    }

    #[test]
    fn a_disabled_event_is_suppressed_with_the_disabled_reason() {
        let mut state = state();
        state.enabled = false;
        assert_eq!(
            plan(
                &request(NotificationEvent::SubtitleDownloadFinished),
                &state
            ),
            Planned::CentreOnly {
                reason: SuppressionReason::Disabled
            }
        );
    }

    #[test]
    fn quiet_hours_hold_back_a_normal_event() {
        let mut state = state();
        state.quiet_hours = Some((
            QuietHoursWindow::window(at(22), at(7)),
            ts(1_700_000_000_000),
        ));
        assert_eq!(
            plan(&request(NotificationEvent::DownloadCompleted), &state),
            Planned::CentreOnly {
                reason: SuppressionReason::QuietHours
            }
        );
    }

    #[test]
    fn quiet_hours_never_hold_back_a_critical_event() {
        let mut state = state();
        state.quiet_hours = Some((
            QuietHoursWindow::window(at(0), at(23)),
            ts(1_700_000_000_000),
        ));
        for event in [
            NotificationEvent::TorrentBlocked,
            NotificationEvent::DiskSpaceLow,
        ] {
            assert_eq!(
                plan(&request(event), &state),
                Planned::Deliver,
                "{event:?} is exempt from quiet hours"
            );
        }
    }

    #[test]
    fn a_critical_event_that_cannot_be_disabled_still_respects_focus() {
        let mut state = state();
        state.enabled = false;
        state.focus = Focus {
            window: true,
            already_on_screen: true,
        };
        assert_eq!(
            plan(&request(NotificationEvent::TorrentBlocked), &state),
            Planned::CentreOnly {
                reason: SuppressionReason::FocusSuppressed
            },
            "R31 exempts critical events from quiet hours, never from focus modes"
        );
    }

    #[test]
    fn a_repeat_inside_the_dedupe_window_is_a_duplicate() {
        let state = state();
        let mut request = request(NotificationEvent::DownloadCompleted);
        request.recent_matching_keys = 1;
        assert_eq!(
            plan(&request, &state),
            Planned::CentreOnly {
                reason: SuppressionReason::Duplicate
            }
        );
    }

    #[test]
    fn focus_suppression_needs_both_the_window_and_the_screen() {
        let mut focused = state();
        focused.focus = Focus {
            window: true,
            already_on_screen: false,
        };
        assert_eq!(
            plan(&request(NotificationEvent::DownloadCompleted), &focused),
            Planned::Deliver,
            "a focused window on another screen still gets a toast"
        );
        focused.focus.already_on_screen = true;
        assert_eq!(
            plan(&request(NotificationEvent::DownloadCompleted), &focused),
            Planned::CentreOnly {
                reason: SuppressionReason::FocusSuppressed
            }
        );
    }

    #[test]
    fn a_burst_of_five_completions_is_one_summary() {
        let state = state();
        let mut request = request(NotificationEvent::DownloadCompleted);
        request.burst_size = 5;
        assert_eq!(plan(&request, &state), Planned::DeliverSummary);
        request.burst_size = BURST_SUMMARY_THRESHOLD;
        assert_eq!(
            plan(&request, &state),
            Planned::Deliver,
            "three completions are not yet a burst"
        );
    }

    #[test]
    fn the_global_rate_limit_folds_the_rest_into_the_centre() {
        let state = state();
        let mut request = request(NotificationEvent::DownloadFailed);
        request.delivered_in_window = RATE_LIMIT_PER_MINUTE;
        assert_eq!(
            plan(&request, &state),
            Planned::CentreOnly {
                reason: SuppressionReason::RateLimited
            }
        );
    }

    #[test]
    fn a_quiet_hours_window_wraps_midnight() {
        let window = QuietHoursWindow::window(at(22), at(7));
        assert!(window.contains(at(23)), "before midnight is inside");
        assert!(window.contains(at(3)), "after midnight is inside");
        assert!(!window.contains(at(12)), "midday is outside");
        let daytime = QuietHoursWindow::window(at(9), at(17));
        assert!(daytime.contains(at(9)), "the start is inclusive");
        assert!(!daytime.contains(at(17)), "the end is exclusive");
    }

    #[test]
    fn only_the_documented_reasons_reach_the_constraint() {
        let reasons = [
            SuppressionReason::Disabled,
            SuppressionReason::PermissionDenied,
            SuppressionReason::QuietHours,
            SuppressionReason::Duplicate,
            SuppressionReason::FocusSuppressed,
            SuppressionReason::RateLimited,
        ];
        for reason in reasons {
            assert!(reason.as_db_str().is_some(), "{reason:?} must be storable");
            assert_eq!(
                SuppressionReason::parse(reason.as_db_str().expect("just asserted")),
                Some(reason),
                "every storable reason must parse back"
            );
        }
        assert_eq!(
            SuppressionReason::DeliveryFailed.as_db_str(),
            None,
            "a failed delivery is a failure, not a suppression"
        );
        assert_eq!(SuppressionReason::parse("delivery_failed"), None);
        assert_eq!(SuppressionReason::parse(""), None);
    }

    #[test]
    fn both_severities_round_trip_through_their_constraint_spelling() {
        for (severity, spelling) in [
            (Severity::Normal, "normal"),
            (Severity::Critical, "critical"),
        ] {
            assert_eq!(severity.as_str(), spelling);
            assert_eq!(Severity::parse(spelling), Some(severity));
        }
        assert_eq!(Severity::parse("warning"), None);
        assert_eq!(Severity::parse(""), None);
    }

    #[test]
    fn a_summary_routes_to_the_queue_and_a_single_event_to_its_own_screen() {
        let payload = |event, subject_id, burst_size| NotificationPayload {
            event,
            subject_id,
            subject_label: None,
            burst_size,
            hide_titles: false,
            sound_enabled: false,
            platform: Platform::MacOs,
            custom_sound: None,
        };

        assert_eq!(
            payload(NotificationEvent::DownloadCompleted, 7, 1)
                .render()
                .expect("catalogue copy is safe")
                .deep_link(),
            "peregrine://downloads",
            "one finished download opens the library"
        );
        assert_eq!(
            payload(NotificationEvent::DownloadCompleted, 7, 5)
                .render()
                .expect("catalogue copy is safe")
                .deep_link(),
            "peregrine://queue",
            "a burst summary opens the queue, not one of the five titles"
        );
        assert_eq!(
            payload(NotificationEvent::NotificationDigest, 0, 5)
                .render()
                .expect("catalogue copy is safe")
                .deep_link(),
            "peregrine://notifications",
            "the digest is not an aggregation, however many events it stands for"
        );
    }

    #[test]
    fn a_deep_link_is_built_from_a_closed_route_set() {
        assert_eq!(
            deep_link("downloads", &[]).expect("a known route").0,
            "peregrine://downloads"
        );
        assert_eq!(
            deep_link("title", &["tmdb", "123"])
                .expect("a known route")
                .0,
            "peregrine://title/tmdb/123"
        );
        assert_eq!(
            deep_link("magnet", &[]),
            Err(DeepLinkError::UnknownRoute {
                route: "magnet".to_owned()
            }),
            "there is no route by which a magnet could become a link"
        );
        assert_eq!(
            deep_link("title", &["tmdb"]),
            Err(DeepLinkError::NotCanonical),
            "a route's segment count is fixed"
        );
        assert_eq!(
            deep_link("title", &["tmdb", "12 3"]),
            Err(DeepLinkError::NotAnIdentifier {
                segment: "12 3".to_owned()
            })
        );
    }

    #[test]
    fn a_custom_sound_is_validated_per_platform() {
        for path in ["/tmp/chime.wav", "/tmp/chime.aiff", "/tmp/chime.ogg"] {
            assert!(
                validate_custom_sound(path, Platform::MacOs).is_ok(),
                "{path} is playable on macOS"
            );
        }
        assert!(validate_custom_sound("C:\\sounds\\chime.wav", Platform::Windows).is_ok());
        assert_eq!(
            validate_custom_sound("C:\\sounds\\chime.ogg", Platform::Windows),
            Err(SoundError::UnsupportedExtension {
                path: "C:\\sounds\\chime.ogg".to_owned(),
                expected: "wav".to_owned()
            })
        );
        assert!(validate_custom_sound("/tmp/chime", Platform::Linux).is_err());
    }

    #[test]
    fn the_system_sound_is_never_empty() {
        for platform in [Platform::MacOs, Platform::Windows, Platform::Linux] {
            assert!(
                !platform.system_sound().is_empty(),
                "{platform:?} must name a sound value"
            );
        }
        assert_eq!(
            Platform::current(),
            Platform::MacOs,
            "this test host is macOS"
        );
    }
}
