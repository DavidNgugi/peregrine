//! Repository tests for the notification centre.
//!
//! These are the storage half of `DATA_MODEL`'s I13, I14's storage neutrality, I25/I26's scope
//! rule, and the two mechanisms the service leans on: the dedupe window and the burst summary's
//! longer one. The policy itself is `peregrine-domain`'s and its service wiring is
//! `peregrine-app`'s; what is proven here is that a row stores and returns what the schema says
//! it may, and that no read leaves the scope (AGENTS R34).

use std::sync::Arc;

use peregrine_domain::centre::{
    CentreCursor, CentreEntry, Delivery, NewCentreEntry, NotificationCentre, NotificationId,
    RecentQuery, Recorded, SubjectKind, SubjectNaming,
};
use peregrine_domain::notify::{
    BURST_WINDOW_MILLIS, DEDUPE_WINDOW_MILLIS, MAX_TEXT_BYTES, NotificationText, Severity,
    SuppressionReason,
};
use peregrine_domain::{AccountId, AccountScope, DeepLink};
use sqlx::sqlite::SqliteConnection;
use tempfile::TempDir;

use crate::database::{Database, open_writer_connection};
use crate::notify::NotificationRepository;

/// A fixed instant: injected, never the wall clock (AGENTS R14).
const NOW: i64 = 1_700_000_000_000;

async fn database(directory: &TempDir) -> Database {
    Database::open(directory.path().join("peregrine.sqlite"))
        .await
        .unwrap()
}

async fn repository(directory: &TempDir) -> (NotificationRepository, Arc<Database>) {
    let database = Arc::new(database(directory).await);
    (NotificationRepository::new(Arc::clone(&database)), database)
}

fn any_account() -> AccountScope {
    AccountScope::unrestricted(AccountId::new(1).expect("account one exists"))
}

fn restricted_account() -> AccountScope {
    AccountScope::restricted(AccountId::new(2).expect("account two exists"))
}

fn text(value: &str) -> NotificationText {
    NotificationText::checked(value, MAX_TEXT_BYTES).expect("the fixture's copy is safe")
}

fn link() -> DeepLink {
    peregrine_domain::notify::deep_link("downloads", &[]).expect("a known route")
}

fn entry(event: &str, subject_id: i64, at: i64) -> NewCentreEntry {
    NewCentreEntry {
        event: event.to_owned(),
        severity: Severity::Normal,
        title: text("Download finished"),
        body: Some(text("Open the library to see it")),
        deep_link: Some(link()),
        subject_kind: Some(SubjectKind::Download),
        subject_id: Some(subject_id),
        dedupe_key: Some(format!("{event}:{subject_id}")),
        dedupe_window_millis: DEDUPE_WINDOW_MILLIS,
        group_key: Some(event.to_owned()),
        sound: Some("Ping".to_owned()),
        created_at: at,
    }
}

fn query(now: i64, dedupe_key: Option<&str>, group_key: Option<&str>) -> RecentQuery {
    RecentQuery {
        now,
        dedupe_key: dedupe_key.map(str::to_owned),
        dedupe_window_millis: DEDUPE_WINDOW_MILLIS,
        group_key: group_key.map(str::to_owned),
        burst_window_millis: BURST_WINDOW_MILLIS,
        rate_limit_window_millis: DEDUPE_WINDOW_MILLIS,
    }
}

/// A writer connection to an already-migrated database, for fixture rows the repository has no
/// public way to create (`title`'s adult flag and rating belong to the provider sync).
async fn connection(directory: &TempDir) -> SqliteConnection {
    open_writer_connection(&directory.path().join("peregrine.sqlite"))
        .await
        .unwrap()
}

/// Inserts a title with the two columns a scope reads, and returns its id.
async fn insert_title(
    connection: &mut SqliteConnection,
    name: &str,
    adult: i64,
    rating: Option<&str>,
) -> i64 {
    let result = sqlx::query(
        "INSERT INTO title (kind, subtype, name, sort_name, content_rating, adult, \
         metadata_state, created_at, updated_at) \
         VALUES ('show', 'standard', ?1, ?2, ?3, ?4, 'complete', 0, 0)",
    )
    .bind(name)
    .bind(name.to_lowercase())
    .bind(rating)
    .bind(adult)
    .execute(&mut *connection)
    .await
    .unwrap();
    result.last_insert_rowid()
}

/// Inserts one episode of `title_id` and returns its id.
async fn insert_episode(connection: &mut SqliteConnection, title_id: i64) -> i64 {
    let season =
        sqlx::query("INSERT INTO season (title_id, season_number, updated_at) VALUES (?1, 1, 0)")
            .bind(title_id)
            .execute(&mut *connection)
            .await
            .unwrap()
            .last_insert_rowid();
    sqlx::query(
        "INSERT INTO episode (title_id, season_id, season_number, episode_number, updated_at) \
         VALUES (?1, ?2, 1, 1, 0)",
    )
    .bind(title_id)
    .bind(season)
    .execute(&mut *connection)
    .await
    .unwrap()
    .last_insert_rowid()
}

/// A centre entry about a title, which is the subject a scope can hide.
fn title_entry(title_id: i64, at: i64) -> NewCentreEntry {
    NewCentreEntry {
        event: "episode.available".to_owned(),
        severity: Severity::Normal,
        title: text("New episode available"),
        body: None,
        deep_link: Some(
            peregrine_domain::notify::deep_link("title", &["tmdb", &title_id.to_string()])
                .expect("a known route"),
        ),
        subject_kind: Some(SubjectKind::Title),
        subject_id: Some(title_id),
        dedupe_key: Some(format!("episode.available:{title_id}")),
        dedupe_window_millis: DEDUPE_WINDOW_MILLIS,
        group_key: Some("episode.available".to_owned()),
        sound: None,
        created_at: at,
    }
}

fn ids(entries: &[CentreEntry]) -> Vec<i64> {
    entries.iter().map(|entry| entry.id.0).collect()
}

#[tokio::test]
async fn an_entry_round_trips_through_the_centre() {
    let directory = TempDir::new().unwrap();
    let (centre, _database) = repository(&directory).await;
    let scope = any_account();

    let recorded = centre
        .record(&entry("download.completed", 7, NOW))
        .await
        .unwrap();
    let Recorded::Inserted(id) = recorded else {
        panic!("the first write is an insert, got {recorded:?}");
    };

    let listed = centre.list(scope, None, 10).await.unwrap();
    assert_eq!(listed.len(), 1);
    let row = &listed[0];
    assert_eq!(row.id, id);
    assert_eq!(row.event, "download.completed");
    assert_eq!(row.severity, Severity::Normal);
    assert_eq!(row.title, "Download finished");
    assert_eq!(row.body.as_deref(), Some("Open the library to see it"));
    assert_eq!(row.deep_link.as_deref(), Some("peregrine://downloads"));
    assert_eq!(row.created_at, NOW);
    assert_eq!(
        row.delivery,
        Delivery::Pending,
        "a new row is pending: the outcome arrives through mark_delivered (I13)"
    );
    assert!(!row.read);
    assert!(!row.dismissed);

    assert_eq!(
        centre.delivery(scope, id).await.unwrap(),
        Some(Delivery::Pending)
    );
    assert_eq!(
        centre
            .delivery(scope, NotificationId(id.0 + 1))
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn an_empty_centre_lists_nothing() {
    let directory = TempDir::new().unwrap();
    let (centre, _database) = repository(&directory).await;
    assert_eq!(
        centre.list(any_account(), None, 10).await.unwrap(),
        Vec::new()
    );
    assert!(
        centre
            .list(
                any_account(),
                Some(CentreCursor {
                    created_at: NOW,
                    id: NotificationId(9)
                }),
                10
            )
            .await
            .unwrap()
            .is_empty(),
        "a cursor below every row has nothing below it"
    );
    assert_eq!(
        centre
            .recent(any_account(), &query(NOW, None, None))
            .await
            .unwrap()
            .delivered_in_window,
        0
    );
    assert_eq!(centre.held_for_digest(any_account()).await.unwrap(), 0);
}

#[tokio::test]
async fn a_repeat_inside_its_dedupe_window_refreshes_the_row() {
    let directory = TempDir::new().unwrap();
    let (centre, _database) = repository(&directory).await;
    let scope = any_account();

    let Recorded::Inserted(id) = centre
        .record(&entry("download.completed", 7, NOW))
        .await
        .unwrap()
    else {
        panic!("the first write is an insert");
    };
    centre
        .mark_delivered(
            id,
            &Delivery::Delivered {
                native_id: 41,
                at: NOW,
            },
        )
        .await
        .unwrap();

    let mut repeat = entry("download.completed", 7, NOW + 1_000);
    repeat.title = text("A download finished");
    let recorded = centre.record(&repeat).await.unwrap();

    assert_eq!(
        recorded,
        Recorded::Updated(id),
        "a repeat inside the window refreshes the entry instead of adding one (ADR 0015 §4)"
    );
    let listed = centre.list(scope, None, 10).await.unwrap();
    assert_eq!(listed.len(), 1, "a coalesced repeat is not a second row");
    assert_eq!(
        listed[0].title, "A download finished",
        "the copy is refreshed"
    );
    assert_eq!(
        listed[0].created_at, NOW,
        "the window is anchored at the first occurrence, so a repeating event \
         cannot coalesce forever"
    );
    assert_eq!(
        centre.delivery(scope, id).await.unwrap(),
        Some(Delivery::Delivered {
            native_id: 41,
            at: NOW
        }),
        "a refresh must not erase the toast that happened"
    );
    assert_eq!(
        centre
            .recent(scope, &query(NOW + 1_000, None, None))
            .await
            .unwrap()
            .delivered_in_window,
        1,
        "the global rate limit counts the delivery that really happened"
    );
}

/// The window's boundary: the start of the window is inclusive and one millisecond past it is a
/// new row. An off-by-one here is a notification the user receives twice a minute.
#[tokio::test]
async fn the_dedupe_window_boundary_is_inclusive_at_its_start() {
    let directory = TempDir::new().unwrap();
    let (centre, _database) = repository(&directory).await;

    centre
        .record(&entry("download.completed", 7, NOW))
        .await
        .unwrap();

    let at_the_edge = centre
        .record(&entry("download.completed", 7, NOW + DEDUPE_WINDOW_MILLIS))
        .await
        .unwrap();
    assert!(
        matches!(at_the_edge, Recorded::Updated(_)),
        "the window's start is inside it"
    );

    let past_the_edge = centre
        .record(&entry(
            "download.completed",
            7,
            NOW + DEDUPE_WINDOW_MILLIS + 1,
        ))
        .await
        .unwrap();
    assert!(
        matches!(past_the_edge, Recorded::Inserted(_)),
        "a millisecond past the window is a new entry"
    );
    assert_eq!(centre.list(any_account(), None, 10).await.unwrap().len(), 2);
}

/// The burst summary coalesces over the longer burst window, which is why the window is a
/// field of the entry rather than a constant on the port.
#[tokio::test]
async fn a_summary_refresh_is_found_over_the_burst_window() {
    let directory = TempDir::new().unwrap();
    let (centre, _database) = repository(&directory).await;

    let mut summary = entry("download.completed.summary", 0, NOW);
    summary.dedupe_window_millis = BURST_WINDOW_MILLIS;
    summary.dedupe_key = Some("download.completed.summary".to_owned());
    let Recorded::Inserted(id) = centre.record(&summary).await.unwrap() else {
        panic!("the first summary is an insert");
    };

    let mut later = summary.clone();
    later.created_at = NOW + DEDUPE_WINDOW_MILLIS + 1;
    assert_eq!(
        centre.record(&later).await.unwrap(),
        Recorded::Updated(id),
        "ninety seconds later is still inside the burst window and the same summary"
    );
}

/// I13's last guard: the schema itself refuses a suppressed row with no reason, so the
/// invariant survives a repository bug rather than depending on one.
#[tokio::test]
async fn the_schema_refuses_a_suppressed_row_with_no_reason() {
    let directory = TempDir::new().unwrap();
    drop(database(&directory).await);
    let mut connection = connection(&directory).await;

    let refused = sqlx::query(
        "INSERT INTO notification (event, severity, title, delivery_state, created_at) \
         VALUES ('download.completed', 'normal', 'Download finished', 'suppressed', ?1)",
    )
    .bind(NOW)
    .execute(&mut connection)
    .await;
    assert!(
        refused.is_err(),
        "I13's CHECK must refuse a suppression with no reason"
    );

    let accepted = sqlx::query(
        "INSERT INTO notification (event, severity, title, delivery_state, suppressed_reason, \
         created_at) VALUES ('download.completed', 'normal', 'Download finished', 'suppressed', \
         'duplicate', ?1)",
    )
    .bind(NOW)
    .execute(&mut connection)
    .await;
    assert!(accepted.is_ok(), "the same row with a reason is legal");
}

#[tokio::test]
async fn every_suppression_reason_round_trips_through_a_row() {
    let directory = TempDir::new().unwrap();
    let (centre, _database) = repository(&directory).await;
    let scope = any_account();

    let reasons = [
        SuppressionReason::Disabled,
        SuppressionReason::PermissionDenied,
        SuppressionReason::QuietHours,
        SuppressionReason::Duplicate,
        SuppressionReason::FocusSuppressed,
        SuppressionReason::RateLimited,
    ];
    for (index, reason) in reasons.into_iter().enumerate() {
        let index = i64::try_from(index).unwrap();
        let Recorded::Inserted(id) = centre
            .record(&entry("download.failed", index, NOW + index))
            .await
            .unwrap()
        else {
            panic!("each reason is written to its own row");
        };
        assert!(
            centre
                .mark_delivered(id, &Delivery::Suppressed(reason))
                .await
                .unwrap()
        );
        assert_eq!(
            centre.delivery(scope, id).await.unwrap(),
            Some(Delivery::Suppressed(reason)),
            "{reason:?} must round-trip, and its stored spelling is what I13 is about"
        );
        assert!(
            reason.as_db_str().is_some_and(|value| !value.is_empty()),
            "{reason:?} must have a non-empty recorded reason"
        );
    }
}

#[tokio::test]
async fn a_failed_delivery_keeps_the_message_and_a_delivered_one_its_native_id() {
    let directory = TempDir::new().unwrap();
    let (centre, _database) = repository(&directory).await;
    let scope = any_account();

    let Recorded::Inserted(first) = centre
        .record(&entry("download.completed", 1, NOW))
        .await
        .unwrap()
    else {
        panic!("an insert");
    };
    assert!(
        centre
            .mark_delivered(
                first,
                &Delivery::Failed("no notification server".to_owned())
            )
            .await
            .unwrap()
    );
    assert_eq!(
        centre.delivery(scope, first).await.unwrap(),
        Some(Delivery::Failed("no notification server".to_owned())),
        "an OS failure is recorded on the row, not returned as an error"
    );

    let Recorded::Inserted(second) = centre
        .record(&entry("download.completed", 2, NOW + 1))
        .await
        .unwrap()
    else {
        panic!("an insert");
    };
    centre
        .mark_delivered(
            second,
            &Delivery::Delivered {
                native_id: 7,
                at: NOW + 2,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        centre.delivery(scope, second).await.unwrap(),
        Some(Delivery::Delivered {
            native_id: 7,
            at: NOW + 2
        })
    );
    assert!(
        !centre
            .mark_delivered(NotificationId(9_999), &Delivery::Pending)
            .await
            .unwrap(),
        "an entry dismissed while its toast was in flight must not resurrect itself"
    );
}

#[tokio::test]
async fn pages_are_keyset_paged_newest_first_and_unaffected_by_new_rows() {
    let directory = TempDir::new().unwrap();
    let (centre, _database) = repository(&directory).await;
    let scope = any_account();

    let mut written = Vec::new();
    for index in 0..5 {
        let Recorded::Inserted(id) = centre
            .record(&entry("download.completed", index, NOW + index))
            .await
            .unwrap()
        else {
            panic!("an insert");
        };
        written.push(id);
    }

    let first = centre.list(scope, None, 2).await.unwrap();
    assert_eq!(ids(&first), vec![written[4].0, written[3].0]);

    // A row arriving between the two reads must not appear in the second page, nor push
    // anything out of it: that is the whole reason the cursor is a value rather than an offset.
    centre
        .record(&entry("download.completed", 99, NOW + 99))
        .await
        .unwrap();
    let last = first.last().expect("a page of two");
    let cursor = CentreCursor {
        created_at: last.created_at,
        id: last.id,
    };
    let second = centre.list(scope, Some(cursor), 2).await.unwrap();
    assert_eq!(ids(&second), vec![written[2].0, written[1].0]);

    let last = second.last().expect("a page of two");
    let third = centre
        .list(
            scope,
            Some(CentreCursor {
                created_at: last.created_at,
                id: last.id,
            }),
            2,
        )
        .await
        .unwrap();
    assert_eq!(ids(&third), vec![written[0].0]);
}

/// Rows sharing a millisecond — which a burst produces — need the id to order and to page.
#[tokio::test]
async fn rows_sharing_one_millisecond_page_by_their_id() {
    let directory = TempDir::new().unwrap();
    let (centre, _database) = repository(&directory).await;
    let scope = any_account();

    let mut written = Vec::new();
    for index in 0..3 {
        let Recorded::Inserted(id) = centre
            .record(&entry("download.completed", index, NOW))
            .await
            .unwrap()
        else {
            panic!("an insert");
        };
        written.push(id);
    }

    let first = centre.list(scope, None, 2).await.unwrap();
    assert_eq!(ids(&first), vec![written[2].0, written[1].0]);
    let last = first.last().expect("a page of two");
    let second = centre
        .list(
            scope,
            Some(CentreCursor {
                created_at: last.created_at,
                id: last.id,
            }),
            2,
        )
        .await
        .unwrap();
    assert_eq!(
        ids(&second),
        vec![written[0].0],
        "the tie is broken by id, so no row is read twice or skipped"
    );
}

#[tokio::test]
async fn a_dismissed_row_leaves_the_centre() {
    let directory = TempDir::new().unwrap();
    let (centre, database) = repository(&directory).await;
    let scope = any_account();

    let Recorded::Inserted(id) = centre
        .record(&entry("download.completed", 7, NOW))
        .await
        .unwrap()
    else {
        panic!("an insert");
    };
    assert_eq!(centre.list(scope, None, 10).await.unwrap().len(), 1);

    sqlx::query("UPDATE notification SET dismissed_at = ?1 WHERE id = ?2")
        .bind(NOW + 1)
        .bind(id.0)
        .execute(database.readers())
        .await
        .unwrap();
    assert_eq!(centre.list(scope, None, 10).await.unwrap(), Vec::new());
}

/// I25/I26: a restricted scope sees neither a notification about an adult-flagged title nor one
/// about a title with no certification, and cannot tell either from a row that does not exist.
#[tokio::test]
async fn a_restricted_scope_hides_notifications_about_hidden_subjects() {
    let directory = TempDir::new().unwrap();
    let (centre, _database) = repository(&directory).await;
    drop(database(&directory).await);
    let mut connection = connection(&directory).await;

    let rated = insert_title(&mut connection, "Rated Show", 0, Some("PG")).await;
    let unrated = insert_title(&mut connection, "Unrated Show", 0, None).await;
    let adult = insert_title(&mut connection, "Adult Show", 1, Some("X")).await;

    let mut written = Vec::new();
    for (index, title) in [rated, unrated, adult].into_iter().enumerate() {
        let recorded = centre
            .record(&title_entry(title, NOW + i64::try_from(index).unwrap()))
            .await
            .unwrap();
        written.push(recorded.id());
    }
    // A notification with no content subject is not gated by a title's flags.
    let neutral = centre
        .record(&entry("download.completed", 7, NOW + 9))
        .await
        .unwrap();

    let unrestricted = centre.list(any_account(), None, 10).await.unwrap();
    assert_eq!(
        unrestricted.len(),
        4,
        "an unrestricted scope sees every entry"
    );

    let restricted = centre.list(restricted_account(), None, 10).await.unwrap();
    assert_eq!(
        ids(&restricted),
        vec![neutral.id().0, written[0].0],
        "only the rated title's entry and the subject-less one remain"
    );
    assert_eq!(
        centre
            .delivery(restricted_account(), written[2])
            .await
            .unwrap(),
        None,
        "a hidden entry reads exactly like an absent one"
    );
    assert_eq!(
        centre
            .delivery(restricted_account(), NotificationId(9_999))
            .await
            .unwrap(),
        None
    );
}

/// The episode join is the second way a notification can point at content: an episode's
/// visibility is its title's.
#[tokio::test]
async fn a_restricted_scope_hides_notifications_about_an_episode_of_a_hidden_title() {
    let directory = TempDir::new().unwrap();
    let (centre, _database) = repository(&directory).await;
    drop(database(&directory).await);
    let mut connection = connection(&directory).await;

    let visible_title = insert_title(&mut connection, "Rated Show", 0, Some("PG")).await;
    let adult_title = insert_title(&mut connection, "Adult Show", 1, Some("X")).await;
    let visible_episode = insert_episode(&mut connection, visible_title).await;
    let adult_episode = insert_episode(&mut connection, adult_title).await;

    let mut episode_entry = title_entry(visible_episode, NOW);
    episode_entry.subject_kind = Some(SubjectKind::Episode);
    let visible = centre.record(&episode_entry).await.unwrap().id();

    let mut hidden_entry = title_entry(adult_episode, NOW + 1);
    hidden_entry.subject_kind = Some(SubjectKind::Episode);
    let hidden = centre.record(&hidden_entry).await.unwrap().id();

    let restricted = centre.list(restricted_account(), None, 10).await.unwrap();
    assert_eq!(ids(&restricted), vec![visible.0]);
    assert_eq!(
        centre.delivery(restricted_account(), hidden).await.unwrap(),
        None
    );
    assert_eq!(centre.list(any_account(), None, 10).await.unwrap().len(), 2);
}

/// ADR 0015 §7: an adult-flagged title is never named, whatever the scope. The rating is the
/// second half of the restricted rule (I27).
#[tokio::test]
async fn only_a_rated_non_adult_title_may_be_named() {
    let directory = TempDir::new().unwrap();
    let (centre, _database) = repository(&directory).await;
    drop(database(&directory).await);
    let mut connection = connection(&directory).await;

    let rated = insert_title(&mut connection, "Rated Show", 0, Some("PG")).await;
    let unrated = insert_title(&mut connection, "Unrated Show", 0, None).await;
    let adult = insert_title(&mut connection, "Adult Show", 1, Some("X")).await;
    let episode_of_adult = insert_episode(&mut connection, adult).await;

    assert!(
        centre
            .may_name(any_account(), SubjectKind::Title, rated)
            .await
            .unwrap(),
        "a rated title is named"
    );
    assert!(
        !centre
            .may_name(any_account(), SubjectKind::Title, adult)
            .await
            .unwrap(),
        "an adult-flagged title is never named, whatever the settings say"
    );
    assert!(
        centre
            .may_name(any_account(), SubjectKind::Title, unrated)
            .await
            .unwrap(),
        "an unrestricted scope may name a title that has no rating yet"
    );
    assert!(
        !centre
            .may_name(restricted_account(), SubjectKind::Title, unrated)
            .await
            .unwrap(),
        "a restricted scope fails closed on an unrated title"
    );
    assert!(
        !centre
            .may_name(restricted_account(), SubjectKind::Episode, episode_of_adult)
            .await
            .unwrap(),
        "an episode is named only when its title may be"
    );
    assert!(
        !centre
            .may_name(any_account(), SubjectKind::Title, 9_999)
            .await
            .unwrap(),
        "a subject that resolves to nothing is not nameable"
    );
    assert!(
        centre
            .may_name(any_account(), SubjectKind::Download, 7)
            .await
            .unwrap(),
        "a download is not gated by a title's flags"
    );
}

/// The digest counts the events quiet hours held, and nothing else: a critical event was
/// delivered rather than held, and a rate-limited one was not waiting for a digest.
#[tokio::test]
async fn held_for_digest_counts_quiet_hours_rows_since_the_last_delivered_digest() {
    let directory = TempDir::new().unwrap();
    let (centre, _database) = repository(&directory).await;
    let scope = any_account();

    let Recorded::Inserted(held) = centre
        .record(&entry("download.completed", 1, NOW))
        .await
        .unwrap()
    else {
        panic!("an insert");
    };
    centre
        .mark_delivered(held, &Delivery::Suppressed(SuppressionReason::QuietHours))
        .await
        .unwrap();

    let Recorded::Inserted(limited) = centre
        .record(&entry("download.completed", 2, NOW + 1))
        .await
        .unwrap()
    else {
        panic!("an insert");
    };
    centre
        .mark_delivered(
            limited,
            &Delivery::Suppressed(SuppressionReason::RateLimited),
        )
        .await
        .unwrap();

    let Recorded::Inserted(critical) = centre.record(&entry("disk.low", 0, NOW + 2)).await.unwrap()
    else {
        panic!("an insert");
    };
    centre
        .mark_delivered(
            critical,
            &Delivery::Delivered {
                native_id: 3,
                at: NOW + 2,
            },
        )
        .await
        .unwrap();

    assert_eq!(
        centre.held_for_digest(scope).await.unwrap(),
        1,
        "only the quiet-hours row is waiting for a digest"
    );

    let mut digest = entry("notification.digest", 0, NOW + 10);
    digest.dedupe_key = Some("notification.digest".to_owned());
    let recorded = centre.record(&digest).await.unwrap();
    centre
        .mark_delivered(
            recorded.id(),
            &Delivery::Delivered {
                native_id: 4,
                at: NOW + 10,
            },
        )
        .await
        .unwrap();

    assert_eq!(
        centre.held_for_digest(scope).await.unwrap(),
        0,
        "a delivered digest stands for the events it covered"
    );

    let Recorded::Inserted(later) = centre
        .record(&entry("download.completed", 3, NOW + 20))
        .await
        .unwrap()
    else {
        panic!("an insert");
    };
    centre
        .mark_delivered(later, &Delivery::Suppressed(SuppressionReason::QuietHours))
        .await
        .unwrap();
    assert_eq!(
        centre.held_for_digest(scope).await.unwrap(),
        1,
        "the next window's held events are counted"
    );
}

/// A digest held back by the rate limit leaves its events for the next attempt rather than
/// consuming them: only a delivered digest advances the boundary.
#[tokio::test]
async fn a_rate_limited_digest_does_not_consume_the_held_events() {
    let directory = TempDir::new().unwrap();
    let (centre, _database) = repository(&directory).await;
    let scope = any_account();

    let Recorded::Inserted(held) = centre
        .record(&entry("download.completed", 1, NOW))
        .await
        .unwrap()
    else {
        panic!("an insert");
    };
    centre
        .mark_delivered(held, &Delivery::Suppressed(SuppressionReason::QuietHours))
        .await
        .unwrap();

    let mut digest = entry("notification.digest", 0, NOW + 5);
    digest.dedupe_key = Some("notification.digest".to_owned());
    let recorded = centre.record(&digest).await.unwrap();
    centre
        .mark_delivered(
            recorded.id(),
            &Delivery::Suppressed(SuppressionReason::RateLimited),
        )
        .await
        .unwrap();

    assert_eq!(
        centre.held_for_digest(scope).await.unwrap(),
        1,
        "the events are still waiting"
    );
}

/// The windows the planner reads: a dedupe key inside its window, a group key inside the longer
/// one, and the global toast count.
#[tokio::test]
async fn recent_selects_by_both_keys_and_counts_deliveries_globally() {
    let directory = TempDir::new().unwrap();
    let (centre, _database) = repository(&directory).await;
    let scope = any_account();

    let Recorded::Inserted(first) = centre
        .record(&entry("download.completed", 1, NOW))
        .await
        .unwrap()
    else {
        panic!("an insert");
    };
    centre
        .mark_delivered(
            first,
            &Delivery::Delivered {
                native_id: 1,
                at: NOW,
            },
        )
        .await
        .unwrap();

    // Outside the dedupe window but inside the burst window, and grouped with the first.
    let Recorded::Inserted(second) = centre
        .record(&entry(
            "download.completed",
            2,
            NOW + DEDUPE_WINDOW_MILLIS + 1,
        ))
        .await
        .unwrap()
    else {
        panic!("an insert");
    };
    centre
        .mark_delivered(second, &Delivery::Suppressed(SuppressionReason::QuietHours))
        .await
        .unwrap();

    let at_the_edge = NOW + DEDUPE_WINDOW_MILLIS;
    let dedupe_only = centre
        .recent(
            scope,
            &query(at_the_edge, Some("download.completed:1"), None),
        )
        .await
        .unwrap();
    assert_eq!(
        dedupe_only.rows.len(),
        1,
        "only the row inside its dedupe key's window is selected"
    );
    assert_eq!(dedupe_only.rows[0].id, first);
    assert_eq!(
        dedupe_only.delivered_in_window, 1,
        "the rate limit counts the delivered row across the installation"
    );

    let after = NOW + DEDUPE_WINDOW_MILLIS + 1_000;
    let burst = centre
        .recent(scope, &query(after, None, Some("download.completed")))
        .await
        .unwrap();
    assert_eq!(
        burst.rows.len(),
        2,
        "the group key's longer window still selects the row that has left its dedupe \
         window, which is why the two keys are separate fields"
    );
    let suppressed = burst
        .rows
        .iter()
        .find(|row| row.id == second)
        .expect("the second row shares the group key");
    assert_eq!(
        suppressed.delivery,
        Delivery::Suppressed(SuppressionReason::QuietHours),
        "a group row's outcome travels to the planner"
    );
}
