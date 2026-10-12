//! The notification service against a real centre and a fake OS.
//!
//! This is where `DATA_MODEL`'s I13, I14 and I15 become behaviour rather than intent, and it is
//! an integration test because the invariant is cross-crate: `peregrine-domain` owns the policy,
//! `peregrine-storage` owns the row, and only the two together can answer "was the reason
//! recorded?" or "did the scanner run before the insert?".
//!
//! The OS is the only fake that matters. `RecordingNotifier` is the seam PG-E1-14's real
//! adapter slots into, and it is what lets every test here run without a notification server,
//! without a network, and without a sleep: the clock is an injected value the test moves by hand
//! (AGENTS R14).

use std::sync::{Arc, Mutex};

use jiff::Timestamp;
use peregrine_app::notify::{
    Delivery, EventPreference, NewCentreEntry, NotificationCentre, NotificationPreferences,
    NotificationService, Preferences, QuietHours, Recorded, ServiceError, SubjectKind,
    SubjectNaming,
};
use peregrine_domain::centre::{CentreError, NotificationId};
use peregrine_domain::check_text;
use peregrine_domain::notify::{
    BURST_SUMMARY_THRESHOLD, DEDUPE_WINDOW_MILLIS, EnableReason, Focus, GENERIC_SUBJECT_LABEL,
    NotificationEvent, NotificationText, NotifyError, OsNotifier, Outgoing, Platform,
    RATE_LIMIT_PER_MINUTE, Severity, SuppressionReason,
};
use peregrine_domain::{AccountId, AccountScope};
use peregrine_storage::{Database, NotificationRepository};
use tempfile::TempDir;

/// A fixed instant every test starts from: injected, never the wall clock.
const NOW: i64 = 1_700_000_000_000;

fn instant(iso: &str) -> Timestamp {
    iso.parse().expect("an RFC 3339 instant")
}

fn ts(millis: i64) -> Timestamp {
    Timestamp::from_millisecond(millis).expect("a millisecond inside the representable range")
}

fn account() -> AccountScope {
    AccountScope::unrestricted(AccountId::new(1).expect("account one exists"))
}

/// The clock as a value the test owns and moves, so crossing a window costs no waiting.
#[derive(Debug, Clone)]
struct TestClock(Arc<Mutex<i64>>);

impl TestClock {
    fn new(at: i64) -> Self {
        Self(Arc::new(Mutex::new(at)))
    }

    fn set(&self, at: i64) {
        *self.0.lock().unwrap() = at;
    }

    fn advance(&self, millis: i64) {
        *self.0.lock().unwrap() += millis;
    }
}

impl peregrine_domain::Clock for TestClock {
    fn now(&self) -> Timestamp {
        ts(*self.0.lock().unwrap())
    }
}

/// The preferences store, which PG-E20-01's settings registry will implement for real.
#[derive(Debug, Clone)]
struct StoredPreferences(Arc<Mutex<Preferences>>);

impl StoredPreferences {
    fn new(preferences: Preferences) -> Self {
        Self(Arc::new(Mutex::new(preferences)))
    }

    fn stored(&self) -> Preferences {
        self.0.lock().unwrap().clone()
    }
}

impl NotificationPreferences for StoredPreferences {
    fn load(
        &self,
    ) -> impl std::future::Future<Output = Result<Preferences, peregrine_app::PreferencesError>> + Send
    {
        std::future::ready(Ok(self.stored()))
    }

    fn save(
        &self,
        preferences: &Preferences,
    ) -> impl std::future::Future<Output = Result<(), peregrine_app::PreferencesError>> + Send {
        *self.0.lock().unwrap() = preferences.clone();
        std::future::ready(Ok(()))
    }
}

/// The OS seam. PG-E1-14 owns the adapter that talks to `tauri-plugin-notification`; this is
/// the double every test here uses instead, so nothing in this file can reach an OS.
#[derive(Debug, Clone, Default)]
struct RecordingNotifier {
    sent: Arc<Mutex<Vec<Outgoing>>>,
    refuse: bool,
}

impl RecordingNotifier {
    fn refusing() -> Self {
        Self {
            sent: Arc::new(Mutex::new(Vec::new())),
            refuse: true,
        }
    }

    fn sent(&self) -> Vec<Outgoing> {
        self.sent.lock().unwrap().clone()
    }

    fn count(&self) -> usize {
        self.sent.lock().unwrap().len()
    }
}

impl OsNotifier for RecordingNotifier {
    fn notify(&self, notification: &Outgoing) -> Result<i32, NotifyError> {
        if self.refuse {
            return Err(NotifyError::Unavailable);
        }
        let mut sent = self.sent.lock().unwrap();
        sent.push(notification.clone());
        Ok(i32::try_from(sent.len()).expect("a handful of toasts"))
    }
}

/// A catalogue that names everything: the ADR 0015 §7 exception is tested separately.
#[derive(Debug, Clone, Copy, Default)]
struct NamesEverything;

impl SubjectNaming for NamesEverything {
    fn may_name(
        &self,
        _scope: AccountScope,
        _kind: SubjectKind,
        _id: i64,
    ) -> impl std::future::Future<Output = Result<bool, CentreError>> + Send {
        std::future::ready(Ok(true))
    }
}

/// A catalogue that answers `allowed`, and records what it was asked.
#[derive(Debug, Clone)]
struct RecordingNaming {
    allowed: bool,
    asked: Arc<Mutex<Vec<(SubjectKind, i64)>>>,
}

impl RecordingNaming {
    fn answering(allowed: bool) -> Self {
        Self {
            allowed,
            asked: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn asked(&self) -> Vec<(SubjectKind, i64)> {
        self.asked.lock().unwrap().clone()
    }
}

impl SubjectNaming for RecordingNaming {
    fn may_name(
        &self,
        _scope: AccountScope,
        kind: SubjectKind,
        id: i64,
    ) -> impl std::future::Future<Output = Result<bool, CentreError>> + Send {
        self.asked.lock().unwrap().push((kind, id));
        std::future::ready(Ok(self.allowed))
    }
}

type TestService<S> =
    NotificationService<NotificationRepository, StoredPreferences, RecordingNotifier, S>;

/// Everything a test needs, with the service already built over a real database.
struct Harness<S> {
    service: TestService<S>,
    centre: NotificationRepository,
    notifier: RecordingNotifier,
    preferences: StoredPreferences,
    clock: TestClock,
    naming: S,
    // Held so the database stays open for the test's lifetime.
    _database: Arc<Database>,
}

impl<S: SubjectNaming + Sync + Clone + 'static> Harness<S> {
    async fn start(directory: &TempDir, preferences: Preferences, naming: S) -> Self {
        let database = Arc::new(
            Database::open(directory.path().join("peregrine.sqlite"))
                .await
                .expect("a fresh database"),
        );
        let centre = NotificationRepository::new(Arc::clone(&database));
        let notifier = RecordingNotifier::default();
        let stored = StoredPreferences::new(preferences);
        let clock = TestClock::new(NOW);
        let service = Self::build(&centre, &stored, &notifier, &clock, naming.clone());
        Self {
            service,
            centre,
            notifier,
            preferences: stored,
            clock,
            naming,
            _database: database,
        }
    }

    /// A second service over the same database, for the restart case.
    fn restart(&self) -> TestService<S> {
        Self::build(
            &self.centre,
            &self.preferences,
            &self.notifier,
            &self.clock,
            self.naming.clone(),
        )
    }

    fn build(
        centre: &NotificationRepository,
        preferences: &StoredPreferences,
        notifier: &RecordingNotifier,
        clock: &TestClock,
        naming: S,
    ) -> TestService<S> {
        NotificationService::new(
            centre.clone(),
            preferences.clone(),
            notifier.clone(),
            naming,
            Box::new(clock.clone()),
            Platform::MacOs,
        )
    }

    /// Every row the centre holds, as the UI would read it.
    async fn rows(&self) -> Vec<peregrine_app::CentreEntry> {
        self.service
            .centre_entries(account(), None, 500)
            .await
            .expect("the centre answers")
    }

    /// The stored delivery state of one row, through the real repository.
    async fn stored(&self, id: NotificationId) -> Delivery {
        self.centre
            .delivery(account(), id)
            .await
            .expect("the centre answers")
            .expect("the row exists")
    }
}

/// The ADR 0015 §3 defaults, with consent given and every event switched on.
fn consented() -> Preferences {
    all_on(Preferences {
        consented: true,
        ..Preferences::default()
    })
}

fn all_on(mut preferences: Preferences) -> Preferences {
    preferences.events = NotificationEvent::ALL
        .into_iter()
        .map(|event| EventPreference {
            event,
            enabled: true,
            sound: false,
            reason: EnableReason::UserChoice,
        })
        .collect();
    preferences
}

/// Switches one event off, the way the settings UI would.
fn switch_off(preferences: &mut Preferences, event: NotificationEvent) {
    if let Some(row) = preferences.events.iter_mut().find(|row| row.event == event) {
        row.enabled = false;
        row.reason = EnableReason::UserChoice;
    }
}

/// The window the fixed instant falls inside: 1 700 000 000 000 ms is 22:13:20 UTC.
fn overnight() -> QuietHours {
    QuietHours::window(
        jiff::civil::Time::new(22, 0, 0, 0).expect("a valid hour"),
        jiff::civil::Time::new(7, 0, 0, 0).expect("a valid hour"),
    )
}

async fn harness(directory: &TempDir, preferences: Preferences) -> Harness<NamesEverything> {
    Harness::start(directory, preferences, NamesEverything).await
}

/// I13, the empty case: nothing happens, so nothing is written.
#[tokio::test]
async fn no_events_means_no_rows() {
    let directory = TempDir::new().unwrap();
    let harness = harness(&directory, consented()).await;

    assert_eq!(harness.rows().await, Vec::new());
    assert_eq!(harness.notifier.count(), 0);
    assert_eq!(
        harness
            .service
            .deliver_quiet_hours_digest(account(), Focus::default())
            .await
            .unwrap(),
        None,
        "a digest with nothing held is not a digest"
    );
    assert_eq!(harness.rows().await, Vec::new());
}

/// R31's "the centre is the record": the id handed to the OS is the id of a row that already
/// exists, so a process that dies between the two leaves a readable entry.
#[tokio::test]
async fn the_centre_row_exists_before_the_os_is_called() {
    let directory = TempDir::new().unwrap();
    let harness = harness(&directory, consented()).await;

    let outcome = harness
        .service
        .notify(
            account(),
            NotificationEvent::DownloadCompleted,
            7,
            Some("Alpha"),
            Focus::default(),
        )
        .await
        .unwrap();

    assert!(outcome.delivered());
    assert_eq!(outcome.aggregated, 1);
    assert_eq!(harness.stored(outcome.id).await, outcome.delivery);
    let sent = harness.notifier.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].notification_id, outcome.id.0,
        "the toast carries the row id, which cannot exist before the row does"
    );
    assert_eq!(harness.rows().await.len(), 1);
}

/// A missing notification server is an expected outcome, not a failed command: the entry
/// stands, and the failure is recorded on it.
#[tokio::test]
async fn an_os_failure_leaves_the_centre_entry_standing() {
    let directory = TempDir::new().unwrap();
    let database = Arc::new(
        Database::open(directory.path().join("peregrine.sqlite"))
            .await
            .unwrap(),
    );
    let centre = NotificationRepository::new(Arc::clone(&database));
    let notifier = RecordingNotifier::refusing();
    let preferences = StoredPreferences::new(consented());
    let clock = TestClock::new(NOW);
    let service = NotificationService::new(
        centre.clone(),
        preferences,
        notifier,
        NamesEverything,
        Box::new(clock),
        Platform::MacOs,
    );

    let outcome = service
        .notify(
            account(),
            NotificationEvent::DownloadFailed,
            7,
            Some("Alpha"),
            Focus::default(),
        )
        .await
        .expect("a refused toast is not a service error");

    assert_eq!(
        outcome.delivery,
        Delivery::Failed("no notification server is reachable on this system".to_owned())
    );
    assert_eq!(
        centre.delivery(account(), outcome.id).await.unwrap(),
        Some(outcome.delivery),
        "the failure is on the row, where the centre can show it"
    );
    let rows = service.centre_entries(account(), None, 10).await.unwrap();
    assert_eq!(rows.len(), 1);
}

/// I13 — path one: the user declined OS notifications. Every event is still a centre entry, and
/// the reason it was not toasted is on the row.
#[tokio::test]
async fn a_declined_user_records_permission_denied() {
    let directory = TempDir::new().unwrap();
    let harness = harness(
        &directory,
        Preferences {
            consented: false,
            ..Preferences::default()
        },
    )
    .await;

    let outcome = harness
        .service
        .notify(
            account(),
            NotificationEvent::DownloadCompleted,
            7,
            None,
            Focus::default(),
        )
        .await
        .unwrap();

    assert_eq!(
        outcome.delivery,
        Delivery::Suppressed(SuppressionReason::PermissionDenied)
    );
    assert_eq!(
        harness.stored(outcome.id).await,
        Delivery::Suppressed(SuppressionReason::PermissionDenied)
    );
    assert_eq!(harness.notifier.count(), 0, "R31: a denial is respected");
    assert_eq!(harness.rows().await.len(), 1, "and the centre still works");
}

/// I13 — path two: the event's own switch is off.
#[tokio::test]
async fn a_disabled_event_records_disabled() {
    let directory = TempDir::new().unwrap();
    let mut preferences = consented();
    switch_off(
        &mut preferences,
        NotificationEvent::SubtitleDownloadFinished,
    );
    let harness = harness(&directory, preferences).await;

    let outcome = harness
        .service
        .notify(
            account(),
            NotificationEvent::SubtitleDownloadFinished,
            7,
            None,
            Focus::default(),
        )
        .await
        .unwrap();

    assert_eq!(
        outcome.delivery,
        Delivery::Suppressed(SuppressionReason::Disabled)
    );
    assert_eq!(
        harness.stored(outcome.id).await,
        Delivery::Suppressed(SuppressionReason::Disabled)
    );
    assert_eq!(harness.notifier.count(), 0);
}

/// I13 — path three: quiet hours hold a normal event back.
#[tokio::test]
async fn a_quiet_hours_event_records_quiet_hours() {
    let directory = TempDir::new().unwrap();
    let harness = harness(
        &directory,
        Preferences {
            quiet_hours: Some(overnight()),
            ..consented()
        },
    )
    .await;

    let outcome = harness
        .service
        .notify(
            account(),
            NotificationEvent::DownloadCompleted,
            7,
            None,
            Focus::default(),
        )
        .await
        .unwrap();

    assert_eq!(
        outcome.delivery,
        Delivery::Suppressed(SuppressionReason::QuietHours)
    );
    assert_eq!(
        harness.stored(outcome.id).await,
        Delivery::Suppressed(SuppressionReason::QuietHours)
    );
    assert_eq!(harness.notifier.count(), 0);
    assert_eq!(
        harness
            .service
            .deliver_quiet_hours_digest(account(), Focus::default())
            .await
            .unwrap()
            .expect("the held event becomes a digest")
            .aggregated,
        1,
        "the digest stands for the one held event"
    );
}

/// I13 — path four: the window is focused on the screen the event is about.
#[tokio::test]
async fn a_focused_window_records_focus_suppressed() {
    let directory = TempDir::new().unwrap();
    let harness = harness(&directory, consented()).await;

    let outcome = harness
        .service
        .notify(
            account(),
            NotificationEvent::DownloadCompleted,
            7,
            None,
            Focus {
                window: true,
                already_on_screen: true,
            },
        )
        .await
        .unwrap();

    assert_eq!(
        outcome.delivery,
        Delivery::Suppressed(SuppressionReason::FocusSuppressed)
    );
    assert_eq!(
        harness.stored(outcome.id).await,
        Delivery::Suppressed(SuppressionReason::FocusSuppressed),
        "the in-app state already told the user; the row says why the toast did not"
    );
    assert_eq!(harness.notifier.count(), 0);
}

/// I13 — path five: the same event for the same subject inside the dedupe window.
#[tokio::test]
async fn a_repeat_inside_the_dedupe_window_records_duplicate() {
    let directory = TempDir::new().unwrap();
    let harness = harness(&directory, consented()).await;

    let first = harness
        .service
        .notify(
            account(),
            NotificationEvent::DownloadCompleted,
            7,
            None,
            Focus::default(),
        )
        .await
        .unwrap();
    harness.clock.advance(1_000);
    let repeat = harness
        .service
        .notify(
            account(),
            NotificationEvent::DownloadCompleted,
            7,
            None,
            Focus::default(),
        )
        .await
        .unwrap();

    assert_eq!(repeat.recorded, Recorded::Updated(first.id));
    assert_eq!(
        repeat.delivery,
        Delivery::Suppressed(SuppressionReason::Duplicate)
    );
    assert_eq!(
        harness.stored(first.id).await,
        Delivery::Suppressed(SuppressionReason::Duplicate),
        "the coalesced repeat is recorded, not silently dropped"
    );
    assert_eq!(harness.notifier.count(), 1, "one toast for the pair");
    assert_eq!(harness.rows().await.len(), 1, "and one centre entry");
}

/// I13 — path six: the global toast rate limit is spent.
#[tokio::test]
async fn the_global_rate_limit_records_rate_limited() {
    let directory = TempDir::new().unwrap();
    let harness = harness(&directory, consented()).await;

    let mut outcomes = Vec::new();
    for subject in 0..=RATE_LIMIT_PER_MINUTE {
        outcomes.push(
            harness
                .service
                .notify(
                    account(),
                    NotificationEvent::DownloadFailed,
                    i64::try_from(subject).unwrap(),
                    None,
                    Focus::default(),
                )
                .await
                .unwrap(),
        );
    }

    for (index, outcome) in outcomes.iter().enumerate() {
        if index < RATE_LIMIT_PER_MINUTE {
            assert!(outcome.delivered(), "toast {index} is inside the limit");
        } else {
            assert_eq!(
                outcome.delivery,
                Delivery::Suppressed(SuppressionReason::RateLimited)
            );
            assert_eq!(
                harness.stored(outcome.id).await,
                Delivery::Suppressed(SuppressionReason::RateLimited)
            );
        }
    }
    assert_eq!(harness.notifier.count(), RATE_LIMIT_PER_MINUTE);
    assert_eq!(
        harness.rows().await.len(),
        RATE_LIMIT_PER_MINUTE + 1,
        "the rate-limited event is still a centre entry"
    );
}

/// I13 — path seven: a burst folds the completions it stands for.
#[tokio::test]
async fn a_burst_folds_the_extra_rows_and_records_why() {
    let directory = TempDir::new().unwrap();
    let harness = harness(&directory, consented()).await;

    let labels = ["Alpha", "Bravo", "Charlie", "Delta", "Echo"];
    for (subject, label) in labels.iter().enumerate() {
        harness
            .service
            .notify(
                account(),
                NotificationEvent::DownloadCompleted,
                i64::try_from(subject).unwrap(),
                Some(label),
                Focus::default(),
            )
            .await
            .unwrap();
    }

    let rows = harness.rows().await;
    for folded in ["Delta", "Echo"] {
        let row = rows
            .iter()
            .find(|row| row.title == folded)
            .unwrap_or_else(|| panic!("`{folded}` was folded into the summary, not dropped"));
        assert_eq!(
            row.delivery,
            Delivery::Suppressed(SuppressionReason::Duplicate),
            "`{folded}` has a recorded reason"
        );
    }
}

/// I14 — a critical event ignores quiet hours and its own switch, and reaches the OS.
#[tokio::test]
async fn a_critical_event_notifies_during_quiet_hours_and_with_its_switch_off() {
    let directory = TempDir::new().unwrap();
    let mut preferences = Preferences {
        quiet_hours: Some(overnight()),
        ..consented()
    };
    switch_off(&mut preferences, NotificationEvent::TorrentBlocked);
    let harness = harness(&directory, preferences).await;

    let outcome = harness
        .service
        .notify(
            account(),
            NotificationEvent::TorrentBlocked,
            7,
            None,
            Focus::default(),
        )
        .await
        .unwrap();

    assert!(
        outcome.delivered(),
        "a security outcome is exempt from quiet hours and from its own switch"
    );
    assert_eq!(harness.notifier.count(), 1);
    assert_eq!(harness.stored(outcome.id).await, outcome.delivery);
}

/// I14 — consent, deduplication, focus and the rate limit still apply to a critical event.
#[tokio::test]
async fn a_critical_event_is_still_deduped() {
    let directory = TempDir::new().unwrap();
    let mut preferences = consented();
    switch_off(&mut preferences, NotificationEvent::TorrentBlocked);
    let harness = harness(&directory, preferences).await;

    let first = harness
        .service
        .notify(
            account(),
            NotificationEvent::TorrentBlocked,
            7,
            None,
            Focus::default(),
        )
        .await
        .unwrap();
    harness.clock.advance(1_000);
    let repeat = harness
        .service
        .notify(
            account(),
            NotificationEvent::TorrentBlocked,
            7,
            None,
            Focus::default(),
        )
        .await
        .unwrap();

    assert!(first.delivered());
    assert_eq!(
        repeat.delivery,
        Delivery::Suppressed(SuppressionReason::Duplicate)
    );
    assert_eq!(harness.notifier.count(), 1);
}

#[tokio::test]
async fn a_critical_event_is_still_rate_limited() {
    let directory = TempDir::new().unwrap();
    let harness = harness(&directory, consented()).await;

    for subject in 0..RATE_LIMIT_PER_MINUTE {
        let outcome = harness
            .service
            .notify(
                account(),
                NotificationEvent::DiskSpaceLow,
                i64::try_from(subject).unwrap(),
                None,
                Focus::default(),
            )
            .await
            .unwrap();
        assert!(outcome.delivered());
    }
    let refused = harness
        .service
        .notify(
            account(),
            NotificationEvent::DiskSpaceLow,
            99,
            None,
            Focus::default(),
        )
        .await
        .unwrap();

    assert_eq!(
        refused.delivery,
        Delivery::Suppressed(SuppressionReason::RateLimited),
        "the global limit is global, critical event or not"
    );
    assert_eq!(harness.notifier.count(), RATE_LIMIT_PER_MINUTE);
}

#[tokio::test]
async fn a_critical_event_is_still_focus_suppressed() {
    let directory = TempDir::new().unwrap();
    let mut preferences = Preferences {
        quiet_hours: Some(overnight()),
        ..consented()
    };
    switch_off(&mut preferences, NotificationEvent::TorrentBlocked);
    let harness = harness(&directory, preferences).await;

    let outcome = harness
        .service
        .notify(
            account(),
            NotificationEvent::TorrentBlocked,
            7,
            None,
            Focus {
                window: true,
                already_on_screen: true,
            },
        )
        .await
        .unwrap();

    assert_eq!(
        outcome.delivery,
        Delivery::Suppressed(SuppressionReason::FocusSuppressed),
        "R31 exempts critical events from quiet hours, never from OS focus modes"
    );
    assert_eq!(harness.notifier.count(), 0);
}

/// I13 and I14 together: a critical event held by nothing is delivered, so the digest that
/// follows must not count it.
#[tokio::test]
async fn a_critical_event_inside_quiet_hours_is_not_folded_into_the_digest() {
    let directory = TempDir::new().unwrap();
    let mixed = harness(
        &directory,
        Preferences {
            quiet_hours: Some(overnight()),
            ..consented()
        },
    )
    .await;

    mixed
        .service
        .notify(
            account(),
            NotificationEvent::DiskSpaceLow,
            1,
            None,
            Focus::default(),
        )
        .await
        .unwrap();
    for subject in 0..2 {
        mixed
            .service
            .notify(
                account(),
                NotificationEvent::DownloadCompleted,
                i64::from(subject),
                None,
                Focus::default(),
            )
            .await
            .unwrap();
    }

    let digest = mixed
        .service
        .deliver_quiet_hours_digest(account(), Focus::default())
        .await
        .unwrap()
        .expect("two events are waiting");
    assert_eq!(
        digest.aggregated, 2,
        "the critical event was delivered during the window, not held for the digest"
    );

    // A window whose only event was critical produces no digest at all.
    let other = TempDir::new().unwrap();
    let only_critical = harness(
        &other,
        Preferences {
            quiet_hours: Some(overnight()),
            ..consented()
        },
    )
    .await;
    only_critical
        .service
        .notify(
            account(),
            NotificationEvent::DiskSpaceLow,
            1,
            None,
            Focus::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        only_critical
            .service
            .deliver_quiet_hours_digest(account(), Focus::default())
            .await
            .unwrap(),
        None
    );
}

/// I14's quiet-hours boundary: the start instant is inside the window and the end instant is
/// not, so an event at 07:00:00 reaches the user instead of waiting for a digest that will
/// never come.
#[tokio::test]
async fn quiet_hours_holds_at_its_start_and_releases_at_its_end() {
    let directory = TempDir::new().unwrap();
    let harness = harness(
        &directory,
        Preferences {
            quiet_hours: Some(overnight()),
            ..consented()
        },
    )
    .await;

    harness
        .clock
        .set(instant("2023-11-14T22:00:00Z").as_millisecond());
    let at_start = harness
        .service
        .notify(
            account(),
            NotificationEvent::DownloadCompleted,
            1,
            None,
            Focus::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        at_start.delivery,
        Delivery::Suppressed(SuppressionReason::QuietHours),
        "the window's start is inside it"
    );

    harness
        .clock
        .set(instant("2023-11-15T07:00:00Z").as_millisecond());
    let at_end = harness
        .service
        .notify(
            account(),
            NotificationEvent::DownloadCompleted,
            2,
            None,
            Focus::default(),
        )
        .await
        .unwrap();
    assert!(
        at_end.delivered(),
        "the window's end is outside it, so the event is delivered rather than held"
    );
    assert_eq!(
        harness.notifier.count(),
        1,
        "only the event at the end reached the OS"
    );
}

/// The digest is not a user-facing switch, so its own catalogue default must not silence it —
/// but consent still must, because R31 puts the user's answer above everything.
#[tokio::test]
async fn a_digest_is_delivered_although_its_own_event_is_default_off() {
    let directory = TempDir::new().unwrap();
    let harness = harness(
        &directory,
        Preferences {
            quiet_hours: Some(overnight()),
            ..Preferences {
                consented: true,
                ..Preferences::default()
            }
        },
    )
    .await;

    let held = harness
        .service
        .notify(
            account(),
            NotificationEvent::DownloadCompleted,
            1,
            None,
            Focus::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        held.delivery,
        Delivery::Suppressed(SuppressionReason::QuietHours),
        "the download is on by default"
    );
    assert!(
        !NotificationEvent::NotificationDigest.default_enabled(),
        "the digest has no switch of its own"
    );

    let digest = harness
        .service
        .deliver_quiet_hours_digest(account(), Focus::default())
        .await
        .unwrap()
        .expect("the held event becomes a digest");
    assert!(
        digest.delivered(),
        "a digest delivers what the user already enabled"
    );
}

#[tokio::test]
async fn a_digest_still_respects_a_declined_user() {
    let directory = TempDir::new().unwrap();
    let harness = harness(
        &directory,
        Preferences {
            consented: false,
            ..Preferences::default()
        },
    )
    .await;

    // A held row cannot arise from a declined user — consent is checked before quiet hours —
    // so it is written directly, which is what makes the digest's own consent gate reachable.
    let entry = NewCentreEntry {
        event: NotificationEvent::DownloadCompleted.as_str().to_owned(),
        severity: Severity::Normal,
        title: NotificationText::checked("Download finished", 512).expect("safe copy"),
        body: None,
        deep_link: None,
        subject_kind: None,
        subject_id: None,
        dedupe_key: Some("download.completed:1".to_owned()),
        dedupe_window_millis: DEDUPE_WINDOW_MILLIS,
        group_key: None,
        sound: None,
        created_at: NOW,
    };
    let recorded = harness.centre.record(&entry).await.unwrap();
    harness
        .centre
        .mark_delivered(
            recorded.id(),
            &Delivery::Suppressed(SuppressionReason::QuietHours),
        )
        .await
        .unwrap();

    let digest = harness
        .service
        .deliver_quiet_hours_digest(account(), Focus::default())
        .await
        .unwrap()
        .expect("a row is waiting");
    assert_eq!(
        digest.delivery,
        Delivery::Suppressed(SuppressionReason::PermissionDenied),
        "the digest is not a way around a denial"
    );
    assert_eq!(harness.notifier.count(), 0);
}

/// Burst summary: N completions inside the window leave their rows, exactly one summary, and an
/// honest count. The count is checked on the outcome and in the summary's own copy.
#[tokio::test]
async fn a_burst_produces_one_summary_row_with_an_honest_count() {
    let directory = TempDir::new().unwrap();
    let harness = harness(&directory, consented()).await;

    let labels = ["Alpha", "Bravo", "Charlie", "Delta", "Echo"];
    let mut aggregated = Vec::new();
    let mut toasts = Vec::new();
    for (subject, label) in labels.iter().enumerate() {
        let outcome = harness
            .service
            .notify(
                account(),
                NotificationEvent::DownloadCompleted,
                i64::try_from(subject).unwrap(),
                Some(label),
                Focus::default(),
            )
            .await
            .unwrap();
        aggregated.push(outcome.aggregated);
        toasts.push(outcome.delivery);
    }

    // The threshold is "more than three completions", so the fourth is the first to be folded.
    assert_eq!(aggregated, vec![1, 1, 1, 4, 5]);
    assert_eq!(
        toasts[4], toasts[3],
        "the fifth completion refreshed the burst's summary instead of raising a second toast"
    );
    assert_eq!(
        harness.notifier.count(),
        4,
        "three completions and one summary reached the OS; the fifth completion did not"
    );

    let rows = harness.rows().await;
    let summaries: Vec<_> = rows
        .iter()
        .filter(|row| row.event == "download.completed.summary")
        .collect();
    assert_eq!(
        summaries.len(),
        1,
        "a burst raises one summary, however many completions arrive"
    );
    assert_eq!(
        summaries[0].title, "5 downloads finished",
        "the count is the completions the summary stands for"
    );
    assert_eq!(
        rows.len(),
        labels.len() + 1,
        "every completion kept its own row, and the burst added one"
    );
    assert_eq!(
        summaries[0].body.as_deref(),
        Some("Open the queue to see them"),
        "the summary's click opens the queue, not a title"
    );
    assert_eq!(summaries[0].deep_link.as_deref(), Some("peregrine://queue"));
}

/// I15: every event template, driven with each forbidden shape as its subject label, leaves no
/// path, secret, magnet, tracker URL, or infohash in any row.
///
/// The canaries are real strings and the roots are real filesystem paths, so the negative
/// control below (`the scanner refuses each canary`) is what stops this from passing because the
/// canaries were harmless.
#[tokio::test]
async fn no_event_template_leaks_a_forbidden_shape_into_a_row() {
    let directory = TempDir::new().unwrap();
    let root = directory.path().join("Peregrine");
    let canaries = [
        format!("{}/library/secret.mkv", root.display()),
        format!("{}/", directory.path().display()),
        "/Users/dave/Downloads/Peregrine/x.mkv".to_owned(),
        r"C:\Users\dave\Videos\x.mkv".to_owned(),
        "tmdb-api-key-4f9c2b1a8e7d6c5b".to_owned(),
        "Xk9Qm2Lp7Rt4Wz8Nv3Bd6Hs1".to_owned(),
        "magnet:?xt=urn:btih:C12FE1C06BBA254A4DE7DDD1AD9E1B0D9C6F0A4B".to_owned(),
        "C12FE1C06BBA254A4DE7DDD1AD9E1B0D9C6F0A4B".to_owned(),
        "http://tracker.example.com/announce?passkey=8f3a9c1d2e4b6a7c".to_owned(),
    ];
    for canary in &canaries {
        assert!(
            check_text(canary, 512).is_err(),
            "`{canary}` must be a shape the scanner refuses, or this test proves nothing"
        );
    }

    let harness = harness(&directory, consented()).await;
    for (round, canary) in canaries.iter().enumerate() {
        for (index, event) in NotificationEvent::ALL.into_iter().enumerate() {
            let subject = i64::try_from(round * 100 + index).unwrap();
            harness
                .service
                .notify(account(), event, subject, Some(canary), Focus::default())
                .await
                .unwrap_or_else(|error| {
                    panic!("`{event:?}` with a {canary} label must be centred, got {error}")
                });
        }
    }

    let rows = harness.rows().await;
    assert!(
        rows.len() >= NotificationEvent::ALL.len(),
        "the test must have rows to inspect, got {}",
        rows.len()
    );
    for row in &rows {
        for canary in &canaries {
            assert!(
                !row.title.contains(canary.as_str()),
                "`{}` reached a title",
                row.title
            );
            assert!(
                !row.body
                    .as_deref()
                    .is_some_and(|body| body.contains(canary.as_str())),
                "`{canary}` reached a body"
            );
        }
        if let Some(link) = &row.deep_link {
            assert!(
                link.starts_with("peregrine://"),
                "`{link}` is not a route this app owns, so a tracker URL had a path into a row"
            );
        }
    }
}

/// ADR 0015 §7: an adult-flagged title is never named, and the check is made before the copy is
/// built rather than trusted from the caller.
#[tokio::test]
async fn an_adult_flagged_title_is_never_named() {
    let directory = TempDir::new().unwrap();
    let naming = RecordingNaming::answering(false);
    let harness = Harness::start(&directory, consented(), naming).await;

    let outcome = harness
        .service
        .notify(
            account(),
            NotificationEvent::NewEpisodeAvailable,
            42,
            Some("Adult Show"),
            Focus::default(),
        )
        .await
        .unwrap();

    assert_eq!(
        harness.naming.asked(),
        vec![(SubjectKind::Title, 42)],
        "the catalogue is asked before the name is used"
    );
    assert_eq!(
        harness.stored(outcome.id).await,
        Delivery::Delivered {
            native_id: 1,
            at: NOW
        },
        "the event is still delivered; only the name is dropped"
    );
    let rows = harness.rows().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "New episode available");
    assert!(
        !rows[0].title.contains("Adult Show"),
        "an adult-flagged title never appears in notification text"
    );
    assert!(
        harness
            .notifier
            .sent()
            .iter()
            .all(|outgoing| !outgoing.title.contains("Adult Show")),
        "and never reaches the lock screen either"
    );
}

/// The dedupe window lives in the database, so a service rebuilt after a restart still knows the
/// event it just saw. This is the case that would fail if the window were in memory.
#[tokio::test]
async fn the_dedupe_window_survives_a_service_rebuild() {
    let directory = TempDir::new().unwrap();
    let harness = harness(&directory, consented()).await;

    let first = harness
        .service
        .notify(
            account(),
            NotificationEvent::DownloadCompleted,
            7,
            None,
            Focus::default(),
        )
        .await
        .unwrap();
    harness.clock.advance(1_000);

    let rebuilt = harness.restart();
    let repeat = rebuilt
        .notify(
            account(),
            NotificationEvent::DownloadCompleted,
            7,
            None,
            Focus::default(),
        )
        .await
        .unwrap();

    assert_eq!(
        repeat.recorded,
        Recorded::Updated(first.id),
        "a window held in memory would have inserted a second row here"
    );
    assert_eq!(
        repeat.delivery,
        Delivery::Suppressed(SuppressionReason::Duplicate)
    );
    assert_eq!(harness.rows().await.len(), 1);
    assert_eq!(harness.notifier.count(), 1);
}

/// The burst window's boundary, and the rate limit's, come from the domain's constants: a test
/// that pinned different numbers would let the two drift.
#[test]
fn the_windows_the_service_uses_are_the_documented_ones() {
    assert_eq!(BURST_SUMMARY_THRESHOLD, 3);
    assert_eq!(RATE_LIMIT_PER_MINUTE, 4);
    assert_eq!(DEDUPE_WINDOW_MILLIS, 60_000);
}

/// A preference write is normalised: a stored critical event cannot be off, so the settings UI
/// can show why its toggle does not move.
#[tokio::test]
async fn storing_a_disabled_critical_event_keeps_it_on() {
    let directory = TempDir::new().unwrap();
    let harness = harness(&directory, consented()).await;
    let mut wanted = consented();
    switch_off(&mut wanted, NotificationEvent::TorrentBlocked);

    harness.service.set_preferences(&wanted).await.unwrap();

    let stored = harness.preferences.stored();
    let row = stored
        .event(NotificationEvent::TorrentBlocked)
        .expect("the catalogue is complete");
    assert!(row.enabled, "a security outcome cannot be switched off");
    assert_eq!(row.reason, EnableReason::CannotDisable);
    assert!(
        stored.setting(NotificationEvent::TorrentBlocked).enabled,
        "and the planner sees it as on"
    );
}

/// I15's drop-don't-refuse rule: a label is decoration, so an unsafe one costs the name and
/// not the notification. The download still reaches the centre, with the catalogue's own copy.
#[tokio::test]
async fn an_unsafe_label_is_dropped_rather_than_failing_the_event() {
    let directory = TempDir::new().unwrap();
    let harness = harness(&directory, consented()).await;

    let outcome = harness
        .service
        .notify(
            account(),
            NotificationEvent::DownloadCompleted,
            7,
            Some("/Users/dave/library/secret.mkv"),
            Focus::default(),
        )
        .await
        .expect("a label must never be the thing that fails an event");

    assert!(
        outcome.delivered(),
        "the download is the notification; the name was only decoration"
    );
    let rows = harness.rows().await;
    assert_eq!(
        rows[0].title, GENERIC_SUBJECT_LABEL,
        "a refused label becomes the catalogue's generic string, not the caller's text"
    );
    assert!(
        !rows[0].title.contains("/Users/dave/library"),
        "the refused label must not appear anywhere"
    );
}

/// The two service errors a caller can see are both typed, so an IPC layer can map them without
/// parsing a string.
#[test]
fn every_service_error_names_its_cause() {
    let unsafe_content = ServiceError::Unsafe(peregrine_domain::redaction::RedactionError::Path);
    assert!(unsafe_content.to_string().contains("refused"));
    let centre = ServiceError::Centre(CentreError::Unavailable {
        message: "the writer stopped".to_owned(),
    });
    assert!(centre.to_string().contains("the writer stopped"));
    let preferences = ServiceError::Preferences(peregrine_app::PreferencesError::Corrupt {
        message: "no quiet hours".to_owned(),
    });
    assert!(preferences.to_string().contains("no quiet hours"));
}
