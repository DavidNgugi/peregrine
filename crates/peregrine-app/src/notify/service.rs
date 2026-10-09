//! The service that turns an event into a centre row and, maybe, a toast.
//!
//! The sequence in [`NotificationService::notify`] is the requirement, so it is
//! spelled out there rather than hidden in a helper: render (which scans), record
//! the centre row, consult the policy, and only then touch the OS. A process that
//! dies at any point after the record leaves a readable entry.

use peregrine_domain::notify::{
    self, BURST_WINDOW_MILLIS, DEDUPE_WINDOW_MILLIS, NotificationEvent, NotificationPayload,
    NotifyError, OsNotifier, Outgoing, Planned, PlannerRequest, PlannerState, Platform,
    RATE_LIMIT_WINDOW_MILLIS, Severity, SuppressionReason,
};
use peregrine_domain::ports::{Clock, IdGenerator};
use peregrine_domain::redaction::RedactionError;
use tracing::{debug, info, warn};

use crate::notify::{
    CentreCursor, CentreEntry, CentreError, Delivery, EventPreference, NewCentreEntry,
    NotificationCentre, NotificationId, NotificationPreferences, Outcome, Preferences,
    PreferencesError, RecentNotification, RecentQuery, Recorded,
};

/// Everything that can stop a notification, or fail while one is being made.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServiceError {
    /// The event's own data could not be rendered without leaking something.
    #[error("the notification was refused: {0}")]
    Unsafe(#[from] RedactionError),

    /// The centre could not be read or written.
    #[error(transparent)]
    Centre(#[from] CentreError),

    /// The user's preferences could not be read or written.
    #[error(transparent)]
    Preferences(#[from] PreferencesError),
}

/// The notification service: the centre is the record, the OS is best effort.
pub struct NotificationService<C, P, N> {
    centre: C,
    preferences: P,
    notifier: N,
    clock: Box<dyn Clock + Send + Sync>,
    ids: Box<dyn IdGenerator + Send + Sync>,
    platform: Platform,
}

impl<C, P, N> NotificationService<C, P, N>
where
    C: NotificationCentre + Sync,
    P: NotificationPreferences + Sync,
    N: OsNotifier + Sync,
{
    /// Builds the service from its ports.
    ///
    /// The clock and the identifier source are injected rather than read here, so
    /// every test in this module runs on a fixed instant (AGENTS R14).
    #[must_use]
    pub fn new(
        centre: C,
        preferences: P,
        notifier: N,
        clock: Box<dyn Clock + Send + Sync>,
        ids: Box<dyn IdGenerator + Send + Sync>,
        platform: Platform,
    ) -> Self {
        Self {
            centre,
            preferences,
            notifier,
            clock,
            ids,
            platform,
        }
    }

    /// The platform this service resolves sound values for.
    #[must_use]
    pub const fn platform(&self) -> Platform {
        self.platform
    }

    /// Handles one event end to end.
    ///
    /// # Errors
    ///
    /// Fails only when nothing could be recorded: an unsafe payload
    /// ([`ServiceError::Unsafe`]) or a centre that could not be reached. An OS
    /// failure is not an error — it is recorded on the row, because R31 makes the
    /// centre the record and the toast the bonus.
    pub async fn notify(
        &self,
        event: NotificationEvent,
        subject_id: i64,
        subject_label: Option<&str>,
    ) -> Result<Outcome, ServiceError> {
        let preferences = self.preferences.load().await?;
        let now = self.clock.now();
        let now_millis = now.as_millisecond();

        // 1. Render. This scans every string and refuses rather than repairs, so
        //    nothing below can see a path, a secret, or a magnet.
        let payload = self.render(event, subject_id, subject_label, &preferences)?;

        // 2. Record the centre row *before* any OS call. On the `Updated` path
        //    this refreshes the existing row instead of adding a second one.
        let entry = NewCentreEntry {
            event: event.as_str().to_owned(),
            severity: match event.severity() {
                Severity::Critical => "critical",
                Severity::Normal => "normal",
            },
            title: payload.title().to_owned(),
            body: payload.body().map(str::to_owned),
            deep_link: Some(payload.deep_link().to_owned()),
            subject_kind: subject_kind(event),
            subject_id: Some(subject_id),
            dedupe_key: Some(dedupe_key(event, subject_id)),
            group_key: Some(event.as_str().to_owned()),
            sound: payload.sound().map(str::to_owned),
            delivery: Delivery::Pending,
            created_at: now_millis,
        };
        let recorded = self.centre.record(&entry).await?;
        let id = recorded.id();

        // 3. Decide. A row refreshed inside the dedupe window is the one case the
        //    policy already knows about before it looks at the counters.
        let mut burst = None;
        let decision = if matches!(recorded, Recorded::Updated(_)) {
            Planned::CentreOnly {
                reason: SuppressionReason::Duplicate,
            }
        } else {
            let counts = self.recent(event, subject_id, now_millis).await?;
            burst = Some(counts.burst_size);
            notify::plan(
                &PlannerRequest {
                    event,
                    subject_id,
                    recent_matching_keys: counts.matching_keys,
                    delivered_in_window: counts.delivered_in_window,
                    burst_size: counts.burst_size,
                },
                &planner_state(event, &preferences, now),
            )
        };

        match decision {
            Planned::Deliver => {
                let delivery = self.deliver(id, &payload, now_millis).await;
                Ok(Outcome {
                    id,
                    recorded,
                    delivery,
                })
            }
            Planned::CentreOnly { reason } => {
                debug!(
                    event = event.as_str(),
                    id = id.0,
                    reason = reason.as_db_str().unwrap_or("delivery_failed"),
                    "notification centred without an OS call"
                );
                let delivery = Delivery::Suppressed(reason);
                self.centre.mark_delivered(id, &delivery).await?;
                Ok(Outcome {
                    id,
                    recorded,
                    delivery,
                })
            }
            Planned::DeliverSummary => {
                let burst_size =
                    u32::try_from(burst.unwrap_or(notify::BURST_SUMMARY_THRESHOLD + 1))
                        .unwrap_or(u32::MAX);
                let summary = self.summary_payload(event, burst_size);
                // The summary's group key is its own ULID from the injected id
                // source: a summary must not be counted as another completion in
                // the burst window, and the id is the only per-summary value the
                // service has (AGENTS R14 keeps the source injected rather than
                // reaching for randomness here).
                let summary_group = format!("{}:{}", event.as_str(), self.ids.next_id());
                let summary_entry =
                    summary_entry(event, subject_id, &summary, now_millis, &summary_group);
                let summary_recorded = self.centre.record(&summary_entry).await?;
                let summary_delivery = self
                    .deliver(summary_recorded.id(), &summary, now_millis)
                    .await;
                // The individual rows say what happened to *them*: they were folded
                // into the summary rather than shown, which is a duplicate by the
                // same rule that coalesces a repeat inside the dedupe window.
                let folded = Delivery::Suppressed(SuppressionReason::Duplicate);
                self.centre.mark_delivered(id, &folded).await?;
                info!(
                    event = event.as_str(),
                    id = id.0,
                    summary_id = summary_recorded.id().0,
                    burst_size,
                    "burst folded into one summary toast"
                );
                Ok(Outcome {
                    id: summary_recorded.id(),
                    recorded: summary_recorded,
                    delivery: summary_delivery,
                })
            }
        }
    }

    /// The rows the centre shows, newest first.
    ///
    /// # Errors
    ///
    /// [`ServiceError::Centre`] when the repository cannot answer.
    pub async fn centre_entries(
        &self,
        cursor: Option<CentreCursor>,
        limit: u32,
    ) -> Result<Vec<CentreEntry>, ServiceError> {
        Ok(self.centre.list(cursor, limit).await?)
    }

    /// Reads the stored preferences, or the catalogue defaults.
    ///
    /// # Errors
    ///
    /// [`ServiceError::Preferences`] when the store cannot answer.
    pub async fn preferences(&self) -> Result<Preferences, ServiceError> {
        Ok(self.preferences.load().await?)
    }

    /// Stores the user's choices, refusing to disable a critical event.
    ///
    /// # Errors
    ///
    /// [`ServiceError::Preferences`] when the store cannot answer.
    pub async fn set_preferences(&self, preferences: &Preferences) -> Result<(), ServiceError> {
        let normalised = normalise(preferences);
        self.preferences.save(&normalised).await?;
        info!(
            consented = normalised.consented,
            sounds = normalised.sounds_enabled,
            "notification preferences stored"
        );
        Ok(())
    }

    /// Renders one event without storing or emitting it.
    fn render(
        &self,
        event: NotificationEvent,
        subject_id: i64,
        subject_label: Option<&str>,
        preferences: &Preferences,
    ) -> Result<peregrine_domain::notify::Payload, ServiceError> {
        let payload = NotificationPayload {
            event,
            subject_id,
            subject_label: subject_label.map(str::to_owned),
            burst_size: 1,
            hide_titles: preferences.hide_titles,
            sound_enabled: preferences.sound(event),
            platform: self.platform,
            custom_sound: None,
        };
        let rendered = payload.render()?;
        debug!(
            event = event.as_str(),
            subject = subject_id,
            "notification rendered and scanned"
        );
        Ok(rendered)
    }

    /// The windowed counts a decision needs.
    async fn recent(
        &self,
        event: NotificationEvent,
        subject_id: i64,
        now: i64,
    ) -> Result<Counts, ServiceError> {
        let rows = self
            .centre
            .recent(&RecentQuery {
                now,
                dedupe_key: Some(dedupe_key(event, subject_id)),
                dedupe_window_millis: DEDUPE_WINDOW_MILLIS,
                group_key: Some(event.as_str().to_owned()),
                burst_window_millis: BURST_WINDOW_MILLIS,
                rate_limit_window_millis: RATE_LIMIT_WINDOW_MILLIS,
            })
            .await?;
        Ok(Counts::of(&rows, now))
    }

    /// Attempts one OS delivery and records the outcome on the row.
    async fn deliver(
        &self,
        id: NotificationId,
        payload: &peregrine_domain::notify::Payload,
        now_millis: i64,
    ) -> Delivery {
        let outgoing = Outgoing::from_payload(id.0, payload);
        let delivery = match self.notifier.notify(&outgoing) {
            Ok(native_id) => {
                debug!(id = id.0, native_id, "notification delivered to the OS");
                Delivery::Delivered {
                    native_id,
                    at: now_millis,
                }
            }
            Err(error) => {
                // Not an error return: the centre row is the record, and a missing
                // notification daemon is an expected outcome, not a failed command.
                warn!(
                    id = id.0,
                    reason = notifier_reason(&error),
                    "OS delivery failed; the centre entry stands"
                );
                Delivery::Failed(error.to_string())
            }
        };
        if let Err(error) = self.centre.mark_delivered(id, &delivery).await {
            warn!(id = id.0, %error, "the delivery outcome could not be recorded");
        }
        delivery
    }

    /// The summary payload for a burst.
    fn summary_payload(
        &self,
        event: NotificationEvent,
        burst_size: u32,
    ) -> peregrine_domain::notify::Payload {
        let payload = NotificationPayload {
            event,
            // The summary's deep link is the queue, not a title, so the subject id
            // is only a placeholder for the route builder.
            subject_id: 0,
            subject_label: None,
            burst_size,
            hide_titles: true,
            sound_enabled: false,
            platform: self.platform,
            custom_sound: None,
        };
        match payload.render() {
            Ok(rendered) => rendered,
            Err(error) => {
                // `render` can only fail on text the catalogue itself produced, so
                // reaching here means the catalogue's copy broke a scanner rule.
                // That is a bug in this crate's own constants, not user input, and
                // the notification has already been recorded as a centre row.
                unreachable!("the catalogue's own copy broke the scanner: {error}")
            }
        }
    }
}

/// The summary's centre row.
fn summary_entry(
    event: NotificationEvent,
    subject_id: i64,
    payload: &peregrine_domain::notify::Payload,
    now_millis: i64,
    group_key: &str,
) -> NewCentreEntry {
    NewCentreEntry {
        event: format!("{}.summary", event.as_str()),
        severity: "normal",
        title: payload.title().to_owned(),
        body: payload.body().map(str::to_owned),
        deep_link: Some(payload.deep_link().to_owned()),
        subject_kind: None,
        subject_id: Some(subject_id),
        dedupe_key: None,
        group_key: Some(group_key.to_owned()),
        sound: payload.sound().map(str::to_owned),
        delivery: Delivery::Pending,
        created_at: now_millis,
    }
}

/// The policy state for this event, from the preferences and the clock.
fn planner_state(
    event: NotificationEvent,
    preferences: &Preferences,
    now: jiff::Timestamp,
) -> PlannerState {
    let setting = preferences.setting(event);
    if !preferences.consented {
        info!(
            event = event.as_str(),
            "the user has not consented to OS notifications"
        );
    }
    PlannerState {
        consented: preferences.consented,
        enabled: setting.enabled,
        quiet_hours: preferences
            .quiet_hours
            .map(|window| (window.as_window(), now)),
        focus: notify::Focus {
            // No window service exists yet — the shell that reports focus arrives
            // with `PG-E1-14` — so the app cannot claim the window has attention.
            // This is deliberately conservative: assuming "unfocused" cannot
            // swallow a toast, while a wrong `true` would.
            window: false,
            already_on_screen: false,
        },
    }
}

/// The dedupe key from ADR 0015 §4: event plus subject.
fn dedupe_key(event: NotificationEvent, subject_id: i64) -> String {
    format!("{}:{}", event.as_str(), subject_id)
}

/// The windowed counts a decision is made from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Counts {
    matching_keys: usize,
    delivered_in_window: usize,
    burst_size: usize,
}

impl Counts {
    /// Counts the three windows from the rows the repository returned.
    fn of(rows: &[RecentNotification], now: i64) -> Self {
        let mut matching_keys = 0_usize;
        let mut delivered_in_window = 0_usize;
        let mut burst_size = 0_usize;
        for row in rows {
            let age = now - row.created_at;
            if age <= DEDUPE_WINDOW_MILLIS && row.dedupe_key.is_some() {
                matching_keys += 1;
            }
            if age <= RATE_LIMIT_WINDOW_MILLIS && matches!(row.delivery, Delivery::Delivered { .. })
            {
                delivered_in_window += 1;
            }
            if age <= BURST_WINDOW_MILLIS
                && matches!(row.delivery, Delivery::Pending | Delivery::Delivered { .. })
            {
                burst_size += 1;
            }
        }
        Self {
            matching_keys,
            delivered_in_window,
            burst_size,
        }
    }
}

/// The subject kind the schema's constraint accepts, when the event has one.
fn subject_kind(event: NotificationEvent) -> Option<&'static str> {
    match event {
        NotificationEvent::NewEpisodeAvailable | NotificationEvent::SubtitleDownloadFinished => {
            Some("title")
        }
        NotificationEvent::DownloadCompleted
        | NotificationEvent::DownloadFailed
        | NotificationEvent::TorrentBlocked => Some("download"),
        NotificationEvent::UpdateAvailable => Some("update"),
        NotificationEvent::DiskSpaceLow
        | NotificationEvent::LibraryScanFinished
        | NotificationEvent::CastSessionEnded
        | NotificationEvent::NotificationDigest => None,
    }
}

/// Forces the catalogue's invariants onto a caller's preferences.
///
/// A critical event is stored as enabled with [`EnableReason::CannotDisable`], so
/// the settings UI can show why its toggle does not move (`PG-E20-13`).
fn normalise(preferences: &Preferences) -> Preferences {
    let mut normalised = preferences.clone();
    normalised.events = normalised
        .events
        .iter()
        .map(|row| {
            if row.event.severity() == Severity::Critical {
                EventPreference {
                    enabled: true,
                    reason: peregrine_domain::notify::EnableReason::CannotDisable,
                    ..row.clone()
                }
            } else {
                row.clone()
            }
        })
        .collect();
    normalised
}

/// The log-safe name for a delivery failure.
fn notifier_reason(error: &NotifyError) -> &'static str {
    match error {
        NotifyError::Unavailable => "unavailable",
        NotifyError::Refused { .. } => "refused",
    }
}
