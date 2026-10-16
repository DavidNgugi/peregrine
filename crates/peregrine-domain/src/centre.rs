//! The in-app notification centre: its port, its rows, and the one question the
//! catalogue must answer before a media name is put in notification text.
//!
//! [ADR 0015](../../../docs/adr/0015-native-notifications.md) §1 and AGENTS R31 make the centre a
//! record rather than a view: the row is written *before* any OS call, because the toast can
//! fail in ways we cannot detect (no notification daemon on Linux, the app muted in macOS
//! System Settings, a click lost once a Windows toast leaves the screen) and nothing can be
//! retracted on either platform. `DATA_MODEL` §2's `notification` table is that record, and
//! invariants I13, I14 and I15 are its rules.
//!
//! Two shapes here do the structural half of that work, so a repository cannot be handed
//! something the scanner has not seen:
//!
//! - [`NewCentreEntry`] holds [`NotificationText`] and [`DeepLink`], whose only constructors
//!   scan. An unbounded `String` has no path into a row (I15).
//! - A new row is always `pending`: the delivery outcome arrives through
//!   [`NotificationCentre::mark_delivered`], so a suppressed row cannot be written without
//!   the reason the schema's `CHECK` requires (I13).
//!
//! The port lives in the domain for the same reason [`OsNotifier`](crate::notify::OsNotifier)
//! does (ARCHITECTURE §5): `peregrine-storage` implements it and may depend on
//! `peregrine-domain` only, so the trait has to be visible from the crate that implements it.
//! What does *not* live here is the decision: the planner in [`crate::notify`] is the only
//! function that chooses between [`Delivery`] outcomes, and the service in `peregrine-app` is
//! its only caller (I14).

use crate::account::AccountScope;
use crate::catalogue::TitleKind;
use crate::notify::{DeepLink, NotificationText, Severity, SuppressionReason};
use crate::stored_enum;
use thiserror::Error;

stored_enum! {
    /// The kind of thing a notification is about, as `notification.subject_kind` stores it.
    ///
    /// The `CHECK` constraint's set exactly: an event whose subject is not one of these has
    /// no subject to look up, and therefore no name to show.
    SubjectKind {
        /// A `title` row.
        Title => "title",
        /// An `episode` row.
        Episode => "episode",
        /// A `download` row.
        Download => "download",
        /// A `device` row.
        Device => "device",
        /// An application update.
        Update => "update",
    }
}

stored_enum! {
    /// What happened to a row's OS delivery, as `notification.delivery_state` stores it.
    ///
    /// The `delivered` case carries the values `native_id` and `delivered_at` hold, so the
    /// four spellings and the five columns cannot drift apart in the row mapper.
    DeliveryState {
        /// Recorded, not yet attempted.
        Pending => "pending",
        /// The OS accepted the toast.
        Delivered => "delivered",
        /// Held back before any OS call.
        Suppressed => "suppressed",
        /// The OS channel refused it.
        Failed => "failed",
    }
}

/// The row identifier the `notification` table assigns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct NotificationId(pub i64);

/// A row to insert into the centre.
///
/// There is no `delivery` field: a row starts `pending`, and the outcome is applied by
/// [`NotificationCentre::mark_delivered`]. That is what makes I13's "a suppressed
/// notification always records a reason" structural rather than a convention — the row
/// cannot be created already suppressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCentreEntry {
    /// The catalogue event's stable key, or `<event>.summary` for a burst roll-up.
    pub event: String,
    /// Normal or critical, from the event's severity.
    pub severity: Severity,
    /// The scanned title.
    pub title: NotificationText,
    /// The scanned body.
    pub body: Option<NotificationText>,
    /// The scanned deep link, shared with the OS payload.
    pub deep_link: Option<DeepLink>,
    /// The subject's kind, when the event has one.
    pub subject_kind: Option<SubjectKind>,
    /// The subject's row id, when the event has one.
    pub subject_id: Option<i64>,
    /// The key repeated events coalesce on.
    pub dedupe_key: Option<String>,
    /// How far back [`Self::dedupe_key`] coalesces, in milliseconds.
    ///
    /// Per-entry rather than a constant on the port because the two keys have different
    /// horizons: a repeated event coalesces over the dedupe window of ADR 0015 §4, while a
    /// burst summary is refreshed for as long as the burst itself lasts.
    pub dedupe_window_millis: i64,
    /// The key a burst summary counts its completions by.
    pub group_key: Option<String>,
    /// The resolved sound value, when one was asked for.
    pub sound: Option<String>,
    /// Unix milliseconds UTC, from the injected clock.
    pub created_at: i64,
}

/// What happened when the entry was recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorded {
    /// A new row was inserted.
    Inserted(NotificationId),
    /// A row inside its dedupe window was refreshed instead of duplicated.
    Updated(NotificationId),
}

impl Recorded {
    /// The row the caller should act on.
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
    /// Normal or critical.
    pub severity: Severity,
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
    /// The subject's row id, when the event named one.
    ///
    /// With [`Self::title_kind`] this is what the app's title route needs. The stored deep link
    /// carries no kind, so a route built from it alone would have to guess -- and a guessed kind
    /// opens the wrong screen, which is worse than opening nothing.
    pub subject_id: Option<i64>,
    /// The media kind of the subject, when the subject is a title.
    pub title_kind: Option<TitleKind>,
}

/// A keyset cursor: the `(created_at, id)` of the last row a page ended on.
///
/// Two values rather than one because `created_at` is milliseconds and a burst writes many
/// rows inside one: an id alone would order by insertion rather than by time, and a
/// timestamp alone would drop every row that ties with the page boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CentreCursor {
    /// The last row's `created_at`.
    pub created_at: i64,
    /// The last row's id, which breaks ties between rows sharing a millisecond.
    pub id: NotificationId,
}

/// A row the planner counts, as the repository reads it.
///
/// Deliberately not [`CentreEntry`]: the planner needs the keys and the outcome, not the
/// copy, and a narrower read is a narrower thing to get wrong.
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

/// The windowed rows a policy decision is made from.
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
    /// Count toasts delivered inside `rate_limit_window_millis`.
    pub rate_limit_window_millis: i64,
}

/// What [`NotificationCentre::recent`] answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentWindow {
    /// The rows the two keys selected, inside their windows and visible to the scope.
    pub rows: Vec<RecentNotification>,
    /// Toasts handed to the OS inside the rate-limit window.
    ///
    /// Counted by `delivered_at` rather than by the current `delivery_state`, and counted
    /// across the whole installation rather than within the scope. Both are deliberate:
    /// ADR 0015 §4 calls the limit global, and a count that a coalescing update or a
    /// restricted account could shrink is a limit that stops limiting.
    pub delivered_in_window: usize,
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
#[derive(Debug, Clone, PartialEq, Eq, Error)]
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

/// Where the in-app centre is stored.
///
/// Every read takes an [`AccountScope`] (AGENTS R34): an entry about a title outside the
/// scope is absent from the result rather than filtered by the caller, so it is
/// indistinguishable from an entry that does not exist (I26).
pub trait NotificationCentre {
    /// Inserts one entry, or refreshes the row its dedupe key already names.
    ///
    /// The write happens before any OS delivery is attempted, so a process that dies
    /// between the two leaves a readable entry rather than a lost event.
    fn record(
        &self,
        entry: &NewCentreEntry,
    ) -> impl std::future::Future<Output = Result<Recorded, CentreError>> + Send;

    /// Lists centre entries, newest first, without their dismissed rows.
    fn list(
        &self,
        scope: AccountScope,
        cursor: Option<CentreCursor>,
        limit: u32,
    ) -> impl std::future::Future<Output = Result<Vec<CentreEntry>, CentreError>> + Send;

    /// The windowed rows and the global toast count a policy decision needs.
    fn recent(
        &self,
        scope: AccountScope,
        query: &RecentQuery,
    ) -> impl std::future::Future<Output = Result<RecentWindow, CentreError>> + Send;

    /// The delivery state of one entry, or `None` when no such row exists or the scope
    /// may not see it.
    fn delivery(
        &self,
        scope: AccountScope,
        id: NotificationId,
    ) -> impl std::future::Future<Output = Result<Option<Delivery>, CentreError>> + Send;

    /// Records the outcome of one OS delivery attempt.
    ///
    /// Returns whether the row was there to update: an entry dismissed while its toast was
    /// in flight must not resurrect itself.
    fn mark_delivered(
        &self,
        id: NotificationId,
        delivery: &Delivery,
    ) -> impl std::future::Future<Output = Result<bool, CentreError>> + Send;

    /// Marks one entry the scope may see as read, answering whether a row changed.
    ///
    /// The scope is applied in the same predicate the reads use, so an entry an account may not
    /// see cannot be marked; marking an already-read row is a no-op rather than an error.
    fn mark_read(
        &self,
        scope: AccountScope,
        id: NotificationId,
        at: i64,
    ) -> impl std::future::Future<Output = Result<bool, CentreError>> + Send;

    /// Marks every unread entry the scope may see as read, answering how many changed.
    fn mark_all_read(
        &self,
        scope: AccountScope,
        at: i64,
    ) -> impl std::future::Future<Output = Result<u64, CentreError>> + Send;

    /// Dismisses one entry the scope may see, answering whether a row changed.
    ///
    /// A dismissed row leaves [`Self::list`]; its delivery history stays in the row, so a toast
    /// in flight cannot resurrect it.
    fn dismiss(
        &self,
        scope: AccountScope,
        id: NotificationId,
        at: i64,
    ) -> impl std::future::Future<Output = Result<bool, CentreError>> + Send;

    /// How many entries are held for the quiet-hours digest.
    ///
    /// Counts the rows suppressed for quiet hours since the last digest, visible to `scope`,
    /// so the digest stands for the events the account may see and a critical event — which
    /// is delivered rather than held — is never part of the count (I14).
    fn held_for_digest(
        &self,
        scope: AccountScope,
    ) -> impl std::future::Future<Output = Result<usize, CentreError>> + Send;
}

/// The catalogue's answer to the one question a notification must ask before it puts a media
/// name in text that will sit on a lock screen.
///
/// ADR 0015 §7 bans an adult-flagged title from notification text outright, and I26 requires
/// that a restricted account's notifications carry nothing it may not see. The name is
/// therefore decoration that is dropped rather than text that is trusted: the fallback copy
/// ("Download finished") is still the notification the user asked for.
pub trait SubjectNaming {
    /// Whether `subject`'s display name may appear in notification text.
    ///
    /// Fails closed: a subject the store cannot resolve is not nameable.
    fn may_name(
        &self,
        scope: AccountScope,
        kind: SubjectKind,
        id: i64,
    ) -> impl std::future::Future<Output = Result<bool, CentreError>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::{DEDUPE_WINDOW_MILLIS, MAX_TEXT_BYTES};
    use crate::redaction::check_elements;

    /// The two spellings of every stored enum are the ones the migration's `CHECK` lists.
    ///
    /// The `notification` table is read out of a live schema in `peregrine-storage`; here the
    /// pair is pinned so that a variant cannot be added with a spelling the constraint would
    /// reject, and so the read half exists for every write half.
    #[test]
    fn every_subject_kind_and_delivery_state_round_trips() {
        let subjects = [
            (SubjectKind::Title, "title"),
            (SubjectKind::Episode, "episode"),
            (SubjectKind::Download, "download"),
            (SubjectKind::Device, "device"),
            (SubjectKind::Update, "update"),
        ];
        for (kind, spelling) in subjects {
            assert_eq!(kind.as_str(), spelling);
            assert_eq!(SubjectKind::parse(spelling), Some(kind));
        }
        assert_eq!(SubjectKind::parse("season"), None);
        assert_eq!(SubjectKind::ALL.len(), subjects.len());

        let states = [
            (DeliveryState::Pending, "pending"),
            (DeliveryState::Delivered, "delivered"),
            (DeliveryState::Suppressed, "suppressed"),
            (DeliveryState::Failed, "failed"),
        ];
        for (state, spelling) in states {
            assert_eq!(state.as_str(), spelling);
            assert_eq!(DeliveryState::parse(spelling), Some(state));
        }
        assert_eq!(DeliveryState::parse("queued"), None);
        assert_eq!(DeliveryState::ALL.len(), states.len());
    }

    /// I15's structural half: the only title a row can hold is one the scanner has seen.
    ///
    /// The assertion is a failure to build rather than a comparison — `NotificationText`
    /// has no other constructor — which is what makes the insert path provably scanned
    /// rather than scanned by convention.
    #[test]
    fn a_row_cannot_be_built_around_unscanned_title_text() {
        assert_eq!(
            NotificationText::checked("/Users/dave/Downloads/x.mkv", MAX_TEXT_BYTES),
            Err(crate::redaction::RedactionError::Path)
        );
        let scanned = NotificationText::checked("Download finished", MAX_TEXT_BYTES)
            .expect("ordinary copy passes");
        assert_eq!(scanned.as_str(), "Download finished");

        let link = crate::notify::deep_link("downloads", &[]).expect("a known route");
        assert_eq!(link.as_str(), "peregrine://downloads");
        assert_eq!(check_elements(link.as_str(), 256), Ok(()));
    }

    /// A cursor carries both ordering values, so a page boundary inside one millisecond is
    /// still expressible: a burst writes many rows at one instant.
    #[test]
    fn a_cursor_orders_by_instant_and_then_by_id() {
        let first = CentreCursor {
            created_at: 1_700_000_000_000,
            id: NotificationId(9),
        };
        let second = CentreCursor {
            created_at: 1_700_000_000_000,
            id: NotificationId(8),
        };
        assert_ne!(first, second, "the id breaks the tie");
        assert!(second.id < first.id);
        assert_eq!(Recorded::Inserted(first.id).id(), first.id);
        assert_eq!(Recorded::Updated(first.id).id(), first.id);
    }

    /// The dedupe window travels with the entry, because a burst summary coalesces over the
    /// burst window rather than the dedupe window.
    #[test]
    fn a_dedupe_window_is_per_entry() {
        let entry = |dedupe_window_millis: i64| NewCentreEntry {
            event: "download.completed".to_owned(),
            severity: Severity::Normal,
            title: NotificationText::checked("Download finished", MAX_TEXT_BYTES)
                .expect("ordinary copy passes"),
            body: None,
            deep_link: None,
            subject_kind: Some(SubjectKind::Download),
            subject_id: Some(7),
            dedupe_key: Some("download.completed:7".to_owned()),
            dedupe_window_millis,
            group_key: Some("download.completed".to_owned()),
            sound: None,
            created_at: 1_700_000_000_000,
        };
        assert_eq!(entry(DEDUPE_WINDOW_MILLIS).dedupe_window_millis, 60_000);
        assert_ne!(
            entry(DEDUPE_WINDOW_MILLIS),
            entry(crate::notify::BURST_WINDOW_MILLIS)
        );
    }
}
