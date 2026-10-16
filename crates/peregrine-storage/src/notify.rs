//! The `notification` table: the in-app centre's rows (`DATA_MODEL` §2.8, ADR 0015 §1).
//!
//! The centre is the authoritative record, not a view of one: the service in `peregrine-app`
//! writes a row *before* it attempts any OS delivery, because a missing notification daemon,
//! a muted macOS app, and a lost Windows click are all failures it cannot detect. That puts
//! three invariants here rather than in the caller:
//!
//! - **I13** — a suppressed row always carries a reason, and the schema's
//!   `CHECK (delivery_state <> 'suppressed' OR suppressed_reason IS NOT NULL)` is the last
//!   guard. A new row is always `pending`, so the only way to reach `suppressed` is through
//!   [`apply_delivery`], which refuses a suppression the constraint would reject.
//! - **I14** — this module writes no policy. It stores the severity the event carries and the
//!   outcome the planner chose; `peregrine-domain`'s `plan` is the only decision function.
//! - **I15** — the text columns accept [`NotificationText`] and [`DeepLink`], whose only
//!   constructors scan, so an unscanned string cannot reach a row through this repository.
//!
//! Every read takes an [`AccountScope`] (AGENTS R34) and applies it in SQL, so an entry about
//! a title outside the scope never leaves the database and a hidden entry is indistinguishable
//! from one that does not exist (I26). The one count that is deliberately unscoped is the
//! global toast rate limit, where a per-account count would let an account exceed the limit.
//!
//! Lists are keyset-paginated on `(created_at, id)`: `cargo xtask check-offset` keeps `OFFSET`
//! out of the tree, and a burst writes several rows inside one millisecond, so the id is what
//! breaks the tie.

use std::sync::Arc;

use peregrine_domain::centre::{
    CentreCursor, CentreEntry, CentreError, Delivery, DeliveryState, NewCentreEntry,
    NotificationCentre, NotificationId, RecentNotification, RecentQuery, RecentWindow, Recorded,
    SubjectKind, SubjectNaming,
};
use peregrine_domain::notify::{NotificationText, SuppressionReason};
use peregrine_domain::{AccountScope, DeepLink, TitleKind};
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnection;

use crate::database::Database;
use crate::error::StorageError;
use crate::page::MAX_PAGE_ROWS;
use crate::writer::{WriteRequest, WriteResponse};

/// The centre's rows, over one [`Database`].
///
/// It implements both ports the notification service needs from storage: the centre itself,
/// and the one catalogue question a notification must ask before it names a media title.
#[derive(Debug, Clone)]
pub struct NotificationRepository {
    database: Arc<Database>,
}

impl NotificationRepository {
    /// Wraps the handle the rest of the application shares.
    #[must_use]
    pub fn new(database: Arc<Database>) -> Self {
        Self { database }
    }
}

impl NotificationCentre for NotificationRepository {
    async fn record(&self, entry: &NewCentreEntry) -> Result<Recorded, CentreError> {
        match self
            .database
            .writer()
            .send(WriteRequest::RecordNotification(Box::new(entry.clone())))
            .await
        {
            Ok(WriteResponse::NotificationRecorded { recorded }) => Ok(recorded),
            Ok(other) => Err(unexpected("record", &other)),
            Err(error) => Err(centre_error(error)),
        }
    }

    async fn mark_read(
        &self,
        scope: AccountScope,
        id: NotificationId,
        at: i64,
    ) -> Result<bool, CentreError> {
        match self
            .database
            .writer()
            .send(WriteRequest::MarkNotificationRead { scope, id, at })
            .await
        {
            Ok(WriteResponse::NotificationRead { changed }) => Ok(changed),
            Ok(other) => Err(unexpected("mark_read", &other)),
            Err(error) => Err(centre_error(error)),
        }
    }

    async fn mark_all_read(&self, scope: AccountScope, at: i64) -> Result<u64, CentreError> {
        match self
            .database
            .writer()
            .send(WriteRequest::MarkAllNotificationsRead { scope, at })
            .await
        {
            Ok(WriteResponse::NotificationsRead { changed }) => Ok(changed),
            Ok(other) => Err(unexpected("mark_all_read", &other)),
            Err(error) => Err(centre_error(error)),
        }
    }

    async fn dismiss(
        &self,
        scope: AccountScope,
        id: NotificationId,
        at: i64,
    ) -> Result<bool, CentreError> {
        match self
            .database
            .writer()
            .send(WriteRequest::DismissNotification { scope, id, at })
            .await
        {
            Ok(WriteResponse::NotificationDismissed { changed }) => Ok(changed),
            Ok(other) => Err(unexpected("dismiss", &other)),
            Err(error) => Err(centre_error(error)),
        }
    }

    async fn list(
        &self,
        scope: AccountScope,
        cursor: Option<CentreCursor>,
        limit: u32,
    ) -> Result<Vec<CentreEntry>, CentreError> {
        list(self.database.readers(), scope, cursor, limit)
            .await
            .map_err(centre_error)
    }

    async fn recent(
        &self,
        scope: AccountScope,
        query: &RecentQuery,
    ) -> Result<RecentWindow, CentreError> {
        recent(self.database.readers(), scope, query)
            .await
            .map_err(centre_error)
    }

    async fn delivery(
        &self,
        scope: AccountScope,
        id: NotificationId,
    ) -> Result<Option<Delivery>, CentreError> {
        delivery(self.database.readers(), scope, id)
            .await
            .map_err(centre_error)
    }

    async fn mark_delivered(
        &self,
        id: NotificationId,
        delivery: &Delivery,
    ) -> Result<bool, CentreError> {
        match self
            .database
            .writer()
            .send(WriteRequest::MarkNotificationDelivered {
                id,
                delivery: Box::new(delivery.clone()),
            })
            .await
        {
            Ok(WriteResponse::NotificationDelivered { changed }) => Ok(changed),
            Ok(other) => Err(unexpected("mark_delivered", &other)),
            Err(error) => Err(centre_error(error)),
        }
    }

    async fn held_for_digest(&self, scope: AccountScope) -> Result<usize, CentreError> {
        held_for_digest(self.database.readers(), scope)
            .await
            .map_err(centre_error)
    }
}

impl SubjectNaming for NotificationRepository {
    async fn may_name(
        &self,
        scope: AccountScope,
        kind: SubjectKind,
        id: i64,
    ) -> Result<bool, CentreError> {
        // One `CASE` rather than two statements: the answer is a property of the subject, and
        // a caller should not have to know which of the two joins its id needs. `COALESCE`
        // makes a subject that resolves to nothing unnameable rather than nameable, which is
        // the fail-closed direction the whole gating rule takes.
        let allowed: Option<i64> = sqlx::query_scalar(
            "SELECT COALESCE(CASE \
               WHEN ?2 = 'title' THEN (SELECT (t.adult = 0 AND (?1 = 0 OR t.content_rating IS NOT NULL)) \
                                         FROM title t WHERE t.id = ?3) \
               WHEN ?2 = 'episode' THEN (SELECT (t.adult = 0 AND (?1 = 0 OR t.content_rating IS NOT NULL)) \
                                           FROM episode e JOIN title t ON t.id = e.title_id WHERE e.id = ?3) \
               ELSE 1 END, 0)",
        )
        .bind(hidden_by_scope(scope))
        .bind(kind.as_str())
        .bind(id)
        .fetch_one(self.database.readers())
        .await
        .map_err(StorageError::from_sqlx)
        .map_err(centre_error)?;
        Ok(allowed == Some(1))
    }
}

/// Inserts one row, or refreshes the row its dedupe key already names.
///
/// The lookup and the write are on the writer's own connection, which is the only writer in
/// the process: two events arriving together cannot both conclude they are the first.
///
/// A refresh deliberately leaves `created_at`, `delivered_at`, and `native_id` alone. Keeping
/// `created_at` anchors the window at the first occurrence, so an event repeating every
/// thirty seconds cannot coalesce forever; keeping `delivered_at` keeps the global rate limit
/// counting a toast that really happened. Recording *why* the refresh was not shown is the
/// service's job (I13), because that is policy.
pub(crate) async fn insert_or_refresh(
    connection: &mut SqliteConnection,
    entry: &NewCentreEntry,
) -> Result<Recorded, StorageError> {
    if let Some(key) = entry.dedupe_key.as_deref() {
        let existing: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM notification WHERE dedupe_key = ?1 AND created_at >= ?2 \
             ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(key)
        .bind(entry.created_at - entry.dedupe_window_millis)
        .fetch_optional(&mut *connection)
        .await
        .map_err(StorageError::from_sqlx)?;

        if let Some(id) = existing {
            sqlx::query(
                "UPDATE notification SET title = ?1, body = ?2, deep_link = ?3, sound = ?4 \
                 WHERE id = ?5",
            )
            .bind(entry.title.as_str())
            .bind(entry.body.as_ref().map(NotificationText::as_str))
            .bind(entry.deep_link.as_ref().map(DeepLink::as_str))
            .bind(entry.sound.as_deref())
            .bind(id)
            .execute(&mut *connection)
            .await
            .map_err(StorageError::from_sqlx)?;
            return Ok(Recorded::Updated(NotificationId(id)));
        }
    }

    let result = sqlx::query(
        "INSERT INTO notification (event, severity, title, body, deep_link, subject_kind, \
         subject_id, dedupe_key, group_key, sound, delivery_state, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'pending', ?11)",
    )
    .bind(&entry.event)
    .bind(entry.severity.as_str())
    .bind(entry.title.as_str())
    .bind(entry.body.as_ref().map(NotificationText::as_str))
    .bind(entry.deep_link.as_ref().map(DeepLink::as_str))
    .bind(entry.subject_kind.map(SubjectKind::as_str))
    .bind(entry.subject_id)
    .bind(entry.dedupe_key.as_deref())
    .bind(entry.group_key.as_deref())
    .bind(entry.sound.as_deref())
    .bind(entry.created_at)
    .execute(&mut *connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(Recorded::Inserted(NotificationId(
        result.last_insert_rowid(),
    )))
}

/// Applies one delivery outcome, reporting whether a row was there to change.
///
/// `native_id` and `delivered_at` are only ever set, never cleared: they are the record that
/// a toast happened, and the global rate limit counts them rather than the current state.
pub(crate) async fn apply_delivery(
    connection: &mut SqliteConnection,
    id: NotificationId,
    delivery: &Delivery,
) -> Result<bool, StorageError> {
    let (state, reason, native_id, delivered_at, error) = match delivery {
        Delivery::Pending => (DeliveryState::Pending, None, None, None, None),
        Delivery::Delivered { native_id, at } => (
            DeliveryState::Delivered,
            None,
            Some(i64::from(*native_id)),
            Some(*at),
            None,
        ),
        Delivery::Suppressed(reason) => (
            DeliveryState::Suppressed,
            Some(
                reason
                    .as_db_str()
                    .ok_or_else(|| StorageError::UnexpectedValue {
                        column: "notification.suppressed_reason",
                        found: format!("{reason:?}"),
                    })?,
            ),
            None,
            None,
            None,
        ),
        Delivery::Failed(message) => (
            DeliveryState::Failed,
            None,
            None,
            None,
            Some(message.as_str()),
        ),
    };

    let result = sqlx::query(
        "UPDATE notification SET delivery_state = ?1, suppressed_reason = ?2, \
         native_id = COALESCE(?3, native_id), delivered_at = COALESCE(?4, delivered_at), \
         delivery_error = ?5 WHERE id = ?6",
    )
    .bind(state.as_str())
    .bind(reason)
    .bind(native_id)
    .bind(delivered_at)
    .bind(error)
    .bind(id.0)
    .execute(&mut *connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(result.rows_affected() > 0)
}

/// One page of centre entries, newest first.
async fn list(
    pool: &SqlitePool,
    scope: AccountScope,
    cursor: Option<CentreCursor>,
    limit: u32,
) -> Result<Vec<CentreEntry>, StorageError> {
    let (cursor_at, cursor_id) = cursor.map_or((None, None), |cursor| {
        (Some(cursor.created_at), Some(cursor.id.0))
    });
    let rows = sqlx::query_as::<_, CentreRow>(
        "SELECT n.id, n.event, n.severity, n.title, n.body, n.deep_link, n.delivery_state, \
         n.suppressed_reason, n.native_id, n.delivered_at, n.delivery_error, n.created_at, \
         n.read_at, n.dismissed_at, n.subject_id, \
         (SELECT t2.kind FROM title t2 \
           WHERE (n.subject_kind = 'title' AND t2.id = n.subject_id) \
              OR (n.subject_kind = 'episode' AND t2.id = \
                    (SELECT e.title_id FROM episode e WHERE e.id = n.subject_id))) \
           AS subject_title_kind FROM notification n \
         WHERE n.dismissed_at IS NULL \
           AND (?1 = 0 OR n.subject_kind IS NULL OR n.subject_kind NOT IN ('title','episode') \
                OR EXISTS (SELECT 1 FROM title t \
                            WHERE t.adult = 0 AND t.content_rating IS NOT NULL \
                              AND ((n.subject_kind = 'title' AND t.id = n.subject_id) \
                                OR (n.subject_kind = 'episode' AND t.id = \
                                      (SELECT e.title_id FROM episode e WHERE e.id = n.subject_id))))) \
           AND (?2 IS NULL OR n.created_at < ?2 OR (n.created_at = ?2 AND n.id < ?3)) \
         ORDER BY n.created_at DESC, n.id DESC LIMIT ?4",
    )
    .bind(hidden_by_scope(scope))
    .bind(cursor_at)
    .bind(cursor_id)
    .bind(i64::from(limit.clamp(1, MAX_PAGE_ROWS)))
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    rows.into_iter().map(TryInto::try_into).collect()
}

/// The rows the two windows select, plus the global count the rate limit uses.
async fn recent(
    pool: &SqlitePool,
    scope: AccountScope,
    query: &RecentQuery,
) -> Result<RecentWindow, StorageError> {
    let rows = sqlx::query_as::<_, RecentRow>(
        "SELECT n.id, n.dedupe_key, n.group_key, n.delivery_state, n.suppressed_reason, \
         n.native_id, n.delivered_at, n.delivery_error, n.created_at FROM notification n \
         WHERE ((?1 IS NOT NULL AND n.dedupe_key = ?1 AND n.created_at >= ?2) \
             OR (?3 IS NOT NULL AND n.group_key = ?3 AND n.created_at >= ?4)) \
           AND n.created_at <= ?5 \
           AND (?6 = 0 OR n.subject_kind IS NULL OR n.subject_kind NOT IN ('title','episode') \
                OR EXISTS (SELECT 1 FROM title t \
                            WHERE t.adult = 0 AND t.content_rating IS NOT NULL \
                              AND ((n.subject_kind = 'title' AND t.id = n.subject_id) \
                                OR (n.subject_kind = 'episode' AND t.id = \
                                      (SELECT e.title_id FROM episode e WHERE e.id = n.subject_id)))))",
    )
    .bind(query.dedupe_key.as_deref())
    .bind(query.now - query.dedupe_window_millis)
    .bind(query.group_key.as_deref())
    .bind(query.now - query.burst_window_millis)
    .bind(query.now)
    .bind(hidden_by_scope(scope))
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;

    // Deliberately unscoped: ADR 0015 §4 makes the toast limit global, and a count narrowed
    // to one account would let that account exceed it.
    let delivered: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notification \
         WHERE delivered_at IS NOT NULL AND delivered_at >= ?1",
    )
    .bind(query.now - query.rate_limit_window_millis)
    .fetch_one(pool)
    .await
    .map_err(StorageError::from_sqlx)?;

    Ok(RecentWindow {
        rows: rows
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<_, _>>()?,
        delivered_in_window: usize::try_from(delivered).unwrap_or(usize::MAX),
    })
}

/// Marks one visible row read, answering whether it changed.
///
/// The scope is the same predicate the reads use, so an entry an account may not see is not
/// there to mark. Marking an already-read row leaves it alone.
pub(crate) async fn mark_read(
    connection: &mut SqliteConnection,
    scope: AccountScope,
    id: NotificationId,
    at: i64,
) -> Result<bool, StorageError> {
    let result = sqlx::query(
        "UPDATE notification SET read_at = ?1 \
         WHERE id = ?2 AND read_at IS NULL \
           AND (?3 = 0 OR subject_kind IS NULL OR subject_kind NOT IN ('title','episode') \
                OR EXISTS (SELECT 1 FROM title t WHERE t.adult = 0 AND t.content_rating IS NOT NULL \
                     AND ((subject_kind = 'title' AND t.id = subject_id) \
                       OR (subject_kind = 'episode' AND t.id = \
                             (SELECT e.title_id FROM episode e WHERE e.id = subject_id)))))",
    )
    .bind(at)
    .bind(id.0)
    .bind(hidden_by_scope(scope))
    .execute(&mut *connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(result.rows_affected() > 0)
}

/// Marks every unread visible row read, answering how many changed.
pub(crate) async fn mark_all_read(
    connection: &mut SqliteConnection,
    scope: AccountScope,
    at: i64,
) -> Result<u64, StorageError> {
    let result = sqlx::query(
        "UPDATE notification SET read_at = ?1 \
         WHERE read_at IS NULL \
           AND (?2 = 0 OR subject_kind IS NULL OR subject_kind NOT IN ('title','episode') \
                OR EXISTS (SELECT 1 FROM title t WHERE t.adult = 0 AND t.content_rating IS NOT NULL \
                     AND ((subject_kind = 'title' AND t.id = subject_id) \
                       OR (subject_kind = 'episode' AND t.id = \
                             (SELECT e.title_id FROM episode e WHERE e.id = subject_id)))))",
    )
    .bind(at)
    .bind(hidden_by_scope(scope))
    .execute(&mut *connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(result.rows_affected())
}

/// Dismisses one visible row, answering whether it changed. The row leaves `list`; its
/// delivery history stays, so a toast in flight cannot resurrect it.
pub(crate) async fn dismiss(
    connection: &mut SqliteConnection,
    scope: AccountScope,
    id: NotificationId,
    at: i64,
) -> Result<bool, StorageError> {
    let result = sqlx::query(
        "UPDATE notification SET dismissed_at = ?1 \
         WHERE id = ?2 AND dismissed_at IS NULL \
           AND (?3 = 0 OR subject_kind IS NULL OR subject_kind NOT IN ('title','episode') \
                OR EXISTS (SELECT 1 FROM title t WHERE t.adult = 0 AND t.content_rating IS NOT NULL \
                     AND ((subject_kind = 'title' AND t.id = subject_id) \
                       OR (subject_kind = 'episode' AND t.id = \
                             (SELECT e.title_id FROM episode e WHERE e.id = subject_id)))))",
    )
    .bind(at)
    .bind(id.0)
    .bind(hidden_by_scope(scope))
    .execute(&mut *connection)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(result.rows_affected() > 0)
}

/// One row's delivery state, when the scope may see the row.
async fn delivery(
    pool: &SqlitePool,
    scope: AccountScope,
    id: NotificationId,
) -> Result<Option<Delivery>, StorageError> {
    let row = sqlx::query_as::<_, DeliveryRow>(
        "SELECT n.delivery_state, n.suppressed_reason, n.native_id, n.delivered_at, \
         n.delivery_error FROM notification n WHERE n.id = ?2 \
           AND (?1 = 0 OR n.subject_kind IS NULL OR n.subject_kind NOT IN ('title','episode') \
                OR EXISTS (SELECT 1 FROM title t \
                            WHERE t.adult = 0 AND t.content_rating IS NOT NULL \
                              AND ((n.subject_kind = 'title' AND t.id = n.subject_id) \
                                OR (n.subject_kind = 'episode' AND t.id = \
                                      (SELECT e.title_id FROM episode e WHERE e.id = n.subject_id)))))",
    )
    .bind(hidden_by_scope(scope))
    .bind(id.0)
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    row.map(TryInto::try_into).transpose()
}

/// How many entries are held for the digest, since the last digest that was delivered.
///
/// The anchor is the newest *delivered* digest rather than the newest digest row, so a digest
/// held back by the rate limit leaves the events for the next attempt instead of consuming
/// them silently.
async fn held_for_digest(pool: &SqlitePool, scope: AccountScope) -> Result<usize, StorageError> {
    let held: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notification n \
         WHERE n.delivery_state = 'suppressed' AND n.suppressed_reason = 'quiet_hours' \
           AND n.created_at > COALESCE((SELECT MAX(d.created_at) FROM notification d \
                                         WHERE d.event = 'notification.digest' \
                                           AND d.delivered_at IS NOT NULL), 0) \
           AND (?1 = 0 OR n.subject_kind IS NULL OR n.subject_kind NOT IN ('title','episode') \
                OR EXISTS (SELECT 1 FROM title t \
                            WHERE t.adult = 0 AND t.content_rating IS NOT NULL \
                              AND ((n.subject_kind = 'title' AND t.id = n.subject_id) \
                                OR (n.subject_kind = 'episode' AND t.id = \
                                      (SELECT e.title_id FROM episode e WHERE e.id = n.subject_id)))))",
    )
    .bind(hidden_by_scope(scope))
    .fetch_one(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(usize::try_from(held).unwrap_or(usize::MAX))
}

/// The scope's visibility rule as a flag a query binds.
///
/// A flag rather than assembled SQL: the alternative is building the `WHERE` clause from a
/// bool, which is how string composition creeps into a repository.
const fn hidden_by_scope(scope: AccountScope) -> i64 {
    scope.is_restricted() as i64
}

/// The raw `notification` row, so the column list stays in one place.
#[derive(sqlx::FromRow)]
struct CentreRow {
    id: i64,
    event: String,
    severity: String,
    title: String,
    body: Option<String>,
    deep_link: Option<String>,
    delivery_state: String,
    suppressed_reason: Option<String>,
    native_id: Option<i64>,
    delivered_at: Option<i64>,
    delivery_error: Option<String>,
    created_at: i64,
    read_at: Option<i64>,
    dismissed_at: Option<i64>,
    subject_id: Option<i64>,
    subject_title_kind: Option<String>,
}

impl TryFrom<CentreRow> for CentreEntry {
    type Error = StorageError;

    fn try_from(row: CentreRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: NotificationId(row.id),
            event: row.event,
            severity: severity(&row.severity)?,
            title: row.title,
            body: row.body,
            deep_link: row.deep_link,
            delivery: delivery_of(
                &row.delivery_state,
                row.suppressed_reason.as_deref(),
                row.native_id,
                row.delivered_at,
                row.delivery_error.as_deref(),
            )?,
            created_at: row.created_at,
            read: row.read_at.is_some(),
            dismissed: row.dismissed_at.is_some(),
            subject_id: row.subject_id,
            title_kind: row.subject_title_kind.as_deref().and_then(TitleKind::parse),
        })
    }
}

/// The delivery columns of one row, without the rest of it.
#[derive(sqlx::FromRow)]
struct DeliveryRow {
    delivery_state: String,
    suppressed_reason: Option<String>,
    native_id: Option<i64>,
    delivered_at: Option<i64>,
    delivery_error: Option<String>,
}

impl TryFrom<DeliveryRow> for Delivery {
    type Error = StorageError;

    fn try_from(row: DeliveryRow) -> Result<Self, Self::Error> {
        delivery_of(
            &row.delivery_state,
            row.suppressed_reason.as_deref(),
            row.native_id,
            row.delivered_at,
            row.delivery_error.as_deref(),
        )
    }
}

/// The keys and the outcome of one row, for a policy decision.
#[derive(sqlx::FromRow)]
struct RecentRow {
    id: i64,
    dedupe_key: Option<String>,
    group_key: Option<String>,
    delivery_state: String,
    suppressed_reason: Option<String>,
    native_id: Option<i64>,
    delivered_at: Option<i64>,
    delivery_error: Option<String>,
    created_at: i64,
}

impl TryFrom<RecentRow> for RecentNotification {
    type Error = StorageError;

    fn try_from(row: RecentRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: NotificationId(row.id),
            dedupe_key: row.dedupe_key,
            group_key: row.group_key,
            delivery: delivery_of(
                &row.delivery_state,
                row.suppressed_reason.as_deref(),
                row.native_id,
                row.delivered_at,
                row.delivery_error.as_deref(),
            )?,
            created_at: row.created_at,
        })
    }
}

/// Parses the `severity` column, naming the column when it holds something else.
fn severity(value: &str) -> Result<peregrine_domain::notify::Severity, StorageError> {
    peregrine_domain::notify::Severity::parse(value).ok_or_else(|| StorageError::UnexpectedValue {
        column: "notification.severity",
        found: value.to_owned(),
    })
}

/// Rebuilds one [`Delivery`] from the four columns that store it.
///
/// The four spellings and the columns they own live in one function so a row cannot be read
/// as a `delivered` toast without the `native_id` and `delivered_at` that make it one.
fn delivery_of(
    state: &str,
    reason: Option<&str>,
    native_id: Option<i64>,
    delivered_at: Option<i64>,
    error: Option<&str>,
) -> Result<Delivery, StorageError> {
    let state = DeliveryState::parse(state).ok_or_else(|| StorageError::UnexpectedValue {
        column: "notification.delivery_state",
        found: state.to_owned(),
    })?;
    match state {
        DeliveryState::Pending => Ok(Delivery::Pending),
        DeliveryState::Delivered => {
            let native_id = native_id
                .and_then(|value| i32::try_from(value).ok())
                .ok_or_else(|| StorageError::UnexpectedValue {
                    column: "notification.native_id",
                    found: format!("{native_id:?}"),
                })?;
            let at = delivered_at.ok_or_else(|| StorageError::UnexpectedValue {
                column: "notification.delivered_at",
                found: "NULL".to_owned(),
            })?;
            Ok(Delivery::Delivered { native_id, at })
        }
        DeliveryState::Suppressed => {
            let reason = reason.and_then(SuppressionReason::parse).ok_or_else(|| {
                StorageError::UnexpectedValue {
                    column: "notification.suppressed_reason",
                    found: reason.unwrap_or("NULL").to_owned(),
                }
            })?;
            Ok(Delivery::Suppressed(reason))
        }
        DeliveryState::Failed => Ok(Delivery::Failed(error.unwrap_or_default().to_owned())),
    }
}

/// Flattens a storage failure into the port's own error.
fn centre_error(error: StorageError) -> CentreError {
    match error {
        StorageError::UnexpectedValue { column, found } => CentreError::Corrupt {
            message: format!("the column `{column}` holds `{found}`"),
        },
        other => CentreError::Unavailable {
            message: other.to_string(),
        },
    }
}

/// The writer answered a request it was not sent.
///
/// Unreachable by construction — the handle sends one request and reads its own answer — so a
/// mismatch is an internal invariant violation rather than a database condition.
fn unexpected(operation: &'static str, response: &WriteResponse) -> CentreError {
    CentreError::Corrupt {
        message: format!("`{operation}` was answered with `{response:?}`"),
    }
}

#[cfg(test)]
mod tests;
