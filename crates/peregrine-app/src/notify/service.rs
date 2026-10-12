//! The service that turns an event into a centre row and, maybe, a toast.
//!
//! The sequence in [`NotificationService::notify`] is the requirement, so it is
//! spelled out there rather than hidden in a helper: render (which scans), record
//! the centre row, consult the policy, and only then touch the OS. A process that
//! dies at any point after the record leaves a readable entry.
//!
//! The policy itself is [`peregrine_domain::notify::plan`], unchanged and unhidden: this module
//! supplies the state it reads (consent, enablement, quiet hours, focus, the windowed counts)
//! and then carries out its answer. No branch here decides whether a toast is allowed
//! (`DATA_MODEL` I14), and no suppression is silent: every path that does not deliver records
//! its reason on the row it already wrote (I13).

use peregrine_domain::AccountScope;
use peregrine_domain::centre::{
    CentreCursor, CentreEntry, CentreError, Delivery, NewCentreEntry, NotificationCentre,
    NotificationId, RecentQuery, Recorded, SubjectKind, SubjectNaming,
};
use peregrine_domain::notify::{
    self, BURST_WINDOW_MILLIS, DEDUPE_WINDOW_MILLIS, Focus, NotificationEvent, NotificationPayload,
    NotifyError, OsNotifier, Outgoing, Payload, Planned, PlannerRequest, PlannerState,
    RATE_LIMIT_WINDOW_MILLIS, Severity, SuppressionReason,
};
use peregrine_domain::ports::Clock;
use peregrine_domain::redaction::RedactionError;
use tracing::{debug, info, warn};

use crate::notify::{
    NotificationPreferences, Outcome, Preferences, PreferencesError, RecentWindow,
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
pub struct NotificationService<C, P, N, S> {
    centre: C,
    preferences: P,
    notifier: N,
    naming: S,
    clock: Box<dyn Clock + Send + Sync>,
    platform: notify::Platform,
}

impl<C, P, N, S> NotificationService<C, P, N, S>
where
    C: NotificationCentre + Sync,
    P: NotificationPreferences + Sync,
    N: OsNotifier + Sync,
    S: SubjectNaming + Sync,
{
    /// Builds the service from its ports.
    ///
    /// The clock is injected rather than read here, so every test runs on a fixed instant and
    /// can move between windows without sleeping (AGENTS R14). Focus is not a port because it
    /// is a fact about one event rather than a service: the caller that knows which window is
    /// on top passes it to [`Self::notify`], and the decision about what to do with it stays in
    /// the planner.
    #[must_use]
    pub fn new(
        centre: C,
        preferences: P,
        notifier: N,
        naming: S,
        clock: Box<dyn Clock + Send + Sync>,
        platform: notify::Platform,
    ) -> Self {
        Self {
            centre,
            preferences,
            notifier,
            naming,
            clock,
            platform,
        }
    }

    /// The platform this service resolves sound values for.
    #[must_use]
    pub const fn platform(&self) -> notify::Platform {
        self.platform
    }

    /// Handles one event end to end.
    ///
    /// `scope` is the account the event is happening for. It reaches the catalogue question the
    /// render needs — may this subject be named? — and the centre reads; it is a parameter
    /// rather than ambient state so a caller cannot forget it (AGENTS R34).
    ///
    /// `focus` says whether the app has the user's attention and whether the screen the event
    /// is about is the one on top. It is a value, like the instant, rather than something this
    /// service reaches for: the shell knows it and the planner decides with it.
    ///
    /// # Errors
    ///
    /// Fails only when nothing could be recorded: an unsafe payload ([`ServiceError::Unsafe`])
    /// or a centre that could not be reached. An OS failure is not an error — it is recorded on
    /// the row, because R31 makes the centre the record and the toast the bonus.
    pub async fn notify(
        &self,
        scope: AccountScope,
        event: NotificationEvent,
        subject_id: i64,
        subject_label: Option<&str>,
        focus: Focus,
    ) -> Result<Outcome, ServiceError> {
        let preferences = self.preferences.load().await?;
        let now = self.clock.now();
        let now_millis = now.as_millisecond();

        // 1. Render. The label is dropped unless the catalogue says the subject may be named,
        //    and `NotificationPayload::render` then scans every remaining string. Refusing
        //    rather than repairing is what makes a path, a secret, or a magnet a caller bug
        //    instead of a lock-screen leak (I15).
        let label = self
            .nameable_label(scope, event, subject_id, subject_label)
            .await?;
        let payload = self.render(event, subject_id, label, &preferences)?;

        let dedupe_key = dedupe_key(event, subject_id);
        let group_key = event.as_str().to_owned();

        // 2. Record the centre row *before* any OS call. The repository coalesces a repeat
        //    inside the dedupe window into the existing row rather than adding a second one
        //    (ADR 0015 §4), and the row it refreshes keeps its delivery history.
        let entry = NewCentreEntry {
            event: group_key.clone(),
            severity: event.severity(),
            title: payload.title_text().clone(),
            body: payload.body_text().cloned(),
            deep_link: Some(payload.deep_link_value().clone()),
            subject_kind: subject_kind(event),
            subject_id: Some(subject_id),
            dedupe_key: Some(dedupe_key.clone()),
            dedupe_window_millis: DEDUPE_WINDOW_MILLIS,
            group_key: Some(group_key.clone()),
            sound: payload.sound().map(str::to_owned),
            created_at: now_millis,
        };
        let recorded = self.centre.record(&entry).await?;
        let id = recorded.id();

        // 3. A refreshed row is the one case the policy does not have to look at: the repeat
        //    was coalesced, and I13 wants that recorded on the row rather than left implicit.
        if matches!(recorded, Recorded::Updated(_)) {
            return self
                .centred(id, recorded, SuppressionReason::Duplicate, 1)
                .await;
        }

        // 4. Decide. The policy is the only function that chooses between these outcomes. The
        //    counts deliberately exclude the row just written: it is this event, not a previous
        //    one, and counting it would make every first occurrence look like a repeat.
        let window = self
            .recent(scope, &dedupe_key, &group_key, now_millis)
            .await?;
        let counts = Counts::of(&window, id, &dedupe_key, &group_key);
        let completions = counts.burst_size + 1;
        let decision = notify::plan(
            &PlannerRequest {
                event,
                subject_id,
                recent_matching_keys: counts.matching_keys,
                delivered_in_window: counts.delivered_in_window,
                burst_size: completions,
            },
            &planner_state(event, &preferences, now, focus),
        );

        match decision {
            Planned::Deliver => {
                let delivery = self.deliver(id, &payload, now_millis).await;
                Ok(Outcome {
                    id,
                    recorded,
                    delivery,
                    aggregated: 1,
                })
            }
            Planned::CentreOnly { reason } => self.centred(id, recorded, reason, 1).await,
            Planned::DeliverSummary => {
                self.summarise(scope, id, event, completions, now_millis)
                    .await
            }
        }
    }

    /// Rolls a burst into one summary entry, and returns the summary's outcome.
    ///
    /// The summary's own dedupe key is what keeps a burst to one toast: while the burst lasts,
    /// a later completion refreshes the existing summary's count instead of raising another
    /// (ADR 0015 §4: "not five toasts"). The individual rows are marked as folded, which is the
    /// recorded reason I13 asks for.
    async fn summarise(
        &self,
        scope: AccountScope,
        id: NotificationId,
        event: NotificationEvent,
        burst_size: usize,
        now_millis: i64,
    ) -> Result<Outcome, ServiceError> {
        let aggregated = u32::try_from(burst_size).unwrap_or(u32::MAX);
        let summary = self.summary_payload(event, aggregated);
        let key = summary_key(event);
        let entry = NewCentreEntry {
            event: key.clone(),
            severity: Severity::Normal,
            title: summary.title_text().clone(),
            body: summary.body_text().cloned(),
            deep_link: Some(summary.deep_link_value().clone()),
            // The summary stands for several subjects, so it points at none of them: a click
            // opens the queue, and no single subject's visibility decides whether it exists.
            subject_kind: None,
            subject_id: None,
            dedupe_key: Some(key.clone()),
            dedupe_window_millis: BURST_WINDOW_MILLIS,
            // Distinct from the completion rows' group key, so the summary is never counted as
            // another completion by the next burst check.
            group_key: Some(key),
            sound: summary.sound().map(str::to_owned),
            created_at: now_millis,
        };

        let summary_recorded = self.centre.record(&entry).await?;
        let summary_id = summary_recorded.id();

        // The triggering completion was folded rather than shown.
        self.centre
            .mark_delivered(id, &Delivery::Suppressed(SuppressionReason::Duplicate))
            .await?;

        if matches!(summary_recorded, Recorded::Inserted(_)) {
            let delivery = self.deliver(summary_id, &summary, now_millis).await;
            info!(
                event = event.as_str(),
                id = id.0,
                summary_id = summary_id.0,
                burst_size,
                "burst folded into one summary toast"
            );
            Ok(Outcome {
                id: summary_id,
                recorded: summary_recorded,
                delivery,
                aggregated,
            })
        } else {
            // The burst already toasted its summary; this refresh only updates the count. The
            // row keeps the delivery it had, and the outcome reports that rather than inventing
            // an attempt that did not happen — the record and the answer cannot disagree.
            debug!(
                event = event.as_str(),
                summary_id = summary_id.0,
                burst_size,
                "the burst's summary row was refreshed, not toasted again"
            );
            let delivery = self
                .centre
                .delivery(scope, summary_id)
                .await?
                .unwrap_or(Delivery::Pending);
            Ok(Outcome {
                id: summary_id,
                recorded: summary_recorded,
                delivery,
                aggregated,
            })
        }
    }

    /// Delivers the events quiet hours held back, as one digest.
    ///
    /// The caller invokes this when the window closes. There is no digest when nothing is
    /// waiting, and the count is the events the scope may see — a critical event was delivered
    /// during the window rather than held, so it is never folded in (I14).
    ///
    /// # Errors
    ///
    /// The preference and centre failures of [`Self::notify`].
    pub async fn deliver_quiet_hours_digest(
        &self,
        scope: AccountScope,
        focus: Focus,
    ) -> Result<Option<Outcome>, ServiceError> {
        let held = self.centre.held_for_digest(scope).await?;
        if held == 0 {
            return Ok(None);
        }

        let preferences = self.preferences.load().await?;
        let now = self.clock.now();
        let now_millis = now.as_millisecond();
        let aggregated = u32::try_from(held).unwrap_or(u32::MAX);
        let payload = self.digest_payload(&preferences, aggregated)?;

        let entry = NewCentreEntry {
            event: NotificationEvent::NotificationDigest.as_str().to_owned(),
            severity: Severity::Normal,
            title: payload.title_text().clone(),
            body: payload.body_text().cloned(),
            deep_link: Some(payload.deep_link_value().clone()),
            subject_kind: None,
            subject_id: None,
            dedupe_key: Some(NotificationEvent::NotificationDigest.as_str().to_owned()),
            dedupe_window_millis: DEDUPE_WINDOW_MILLIS,
            group_key: None,
            sound: payload.sound().map(str::to_owned),
            created_at: now_millis,
        };
        let recorded = self.centre.record(&entry).await?;
        let id = recorded.id();

        if matches!(recorded, Recorded::Updated(_)) {
            return self
                .centred(id, recorded, SuppressionReason::Duplicate, aggregated)
                .await
                .map(Some);
        }

        // The digest is not a user-facing switch — ADR 0015 §3 gives it no settings row — so
        // its enablement is derived: it delivers events the user already enabled, which is why
        // they were held rather than suppressed as disabled. Quiet hours is deliberately absent
        // because the window it stands for has closed; consent, focus, and the rate limit all
        // still apply, because `plan` is still the only decider.
        let state = PlannerState {
            consented: preferences.consented,
            enabled: true,
            quiet_hours: None,
            focus,
        };
        // The digest has no dedupe or group key to count, so this read answers only with the
        // global toast count the rate limit needs.
        let window = self.recent(scope, "", "", now_millis).await?;
        let request = PlannerRequest {
            event: NotificationEvent::NotificationDigest,
            subject_id: 0,
            recent_matching_keys: 0,
            delivered_in_window: window.delivered_in_window,
            burst_size: 0,
        };
        match notify::plan(&request, &state) {
            // The digest event does not aggregate, so `plan` cannot answer with a summary for
            // it. Delivering it is the fallback if that property ever changes, because the
            // alternative — recording a suppression reason that did not happen — would put a
            // false statement in the record.
            Planned::Deliver | Planned::DeliverSummary => {
                let delivery = self.deliver(id, &payload, now_millis).await;
                info!(id = id.0, held, "quiet hours ended with a digest");
                Ok(Some(Outcome {
                    id,
                    recorded,
                    delivery,
                    aggregated,
                }))
            }
            Planned::CentreOnly { reason } => self
                .centred(id, recorded, reason, aggregated)
                .await
                .map(Some),
        }
    }

    /// The centre entries the scope may see, newest first.
    ///
    /// # Errors
    ///
    /// [`ServiceError::Centre`] when the repository cannot answer.
    pub async fn centre_entries(
        &self,
        scope: AccountScope,
        cursor: Option<CentreCursor>,
        limit: u32,
    ) -> Result<Vec<CentreEntry>, ServiceError> {
        Ok(self.centre.list(scope, cursor, limit).await?)
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

    /// Records that this event reached the centre without an OS call, and why.
    async fn centred(
        &self,
        id: NotificationId,
        recorded: Recorded,
        reason: SuppressionReason,
        aggregated: u32,
    ) -> Result<Outcome, ServiceError> {
        let delivery = Delivery::Suppressed(reason);
        self.centre.mark_delivered(id, &delivery).await?;
        debug!(
            id = id.0,
            reason = reason.as_db_str().unwrap_or("delivery_failed"),
            "notification centred without an OS call"
        );
        Ok(Outcome {
            id,
            recorded,
            delivery,
            aggregated,
        })
    }

    /// The subject label, when the catalogue says the subject may be named.
    ///
    /// ADR 0015 §7 bans an adult-flagged title from notification text outright, and a
    /// restricted scope must not carry a name it may not see (I26). The label is decoration, so
    /// it is dropped rather than being allowed to fail the event: "Download finished" is still
    /// the notification the user asked for.
    async fn nameable_label(
        &self,
        scope: AccountScope,
        event: NotificationEvent,
        subject_id: i64,
        subject_label: Option<&str>,
    ) -> Result<Option<String>, ServiceError> {
        let (Some(label), Some(kind)) = (subject_label, subject_kind(event)) else {
            return Ok(None);
        };
        let allowed = self.naming.may_name(scope, kind, subject_id).await?;
        Ok(allowed.then(|| label.to_owned()))
    }

    /// Renders one event without storing or emitting it.
    fn render(
        &self,
        event: NotificationEvent,
        subject_id: i64,
        subject_label: Option<String>,
        preferences: &Preferences,
    ) -> Result<Payload, ServiceError> {
        let payload = NotificationPayload {
            event,
            subject_id,
            subject_label,
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
        scope: AccountScope,
        dedupe_key: &str,
        group_key: &str,
        now: i64,
    ) -> Result<RecentWindow, ServiceError> {
        Ok(self
            .centre
            .recent(
                scope,
                &RecentQuery {
                    now,
                    dedupe_key: (!dedupe_key.is_empty()).then(|| dedupe_key.to_owned()),
                    dedupe_window_millis: DEDUPE_WINDOW_MILLIS,
                    group_key: (!group_key.is_empty()).then(|| group_key.to_owned()),
                    burst_window_millis: BURST_WINDOW_MILLIS,
                    rate_limit_window_millis: RATE_LIMIT_WINDOW_MILLIS,
                },
            )
            .await?)
    }

    /// Attempts one OS delivery and records the outcome on the row.
    async fn deliver(&self, id: NotificationId, payload: &Payload, now_millis: i64) -> Delivery {
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
    fn summary_payload(&self, event: NotificationEvent, burst_size: u32) -> Payload {
        let payload = NotificationPayload {
            event,
            // The summary's deep link is the queue, not a title, so the subject id is only a
            // placeholder for the route builder.
            subject_id: 0,
            subject_label: None,
            burst_size,
            hide_titles: true,
            sound_enabled: false,
            platform: self.platform,
            custom_sound: None,
        };
        render_or_bug(&payload)
    }

    /// The digest payload: generic copy, no title, and the held count.
    fn digest_payload(
        &self,
        preferences: &Preferences,
        held: u32,
    ) -> Result<Payload, ServiceError> {
        let payload = NotificationPayload {
            event: NotificationEvent::NotificationDigest,
            subject_id: 0,
            subject_label: None,
            burst_size: held,
            hide_titles: true,
            sound_enabled: preferences.sound(NotificationEvent::NotificationDigest),
            platform: self.platform,
            custom_sound: None,
        };
        Ok(payload.render()?)
    }
}

/// Renders catalogue copy, which the scanner has already accepted.
///
/// A failure here is this crate's own constant breaking a scanner rule, not user input: the
/// notification has already been recorded, and the alternative to a loud abort is a silently
/// missing toast.
fn render_or_bug(payload: &NotificationPayload) -> Payload {
    match payload.render() {
        Ok(rendered) => rendered,
        Err(error) => unreachable!("the catalogue's own copy broke the scanner: {error}"),
    }
}

/// The policy state for this event, from the preferences and the clock.
fn planner_state(
    event: NotificationEvent,
    preferences: &Preferences,
    now: jiff::Timestamp,
    focus: Focus,
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
        focus,
    }
}

/// The dedupe key from ADR 0015 §4: event plus subject.
fn dedupe_key(event: NotificationEvent, subject_id: i64) -> String {
    format!("{}:{}", event.as_str(), subject_id)
}

/// The key a burst summary coalesces and counts by.
///
/// Distinct from the event's own key so the summary is never mistaken for another completion
/// of the burst it stands for.
fn summary_key(event: NotificationEvent) -> String {
    format!("{}.summary", event.as_str())
}

/// The windowed counts a decision is made from.
///
/// The two keys are counted separately from the rows both windows returned: a row can be inside
/// the burst window and outside the dedupe window, and which window selected it is what
/// distinguishes "a repeat" from "the fourth completion". `current` is the row this event just
/// wrote, and it is skipped: it is the event being decided about, not a previous occurrence of
/// it, and the burst count has to be able to answer "how many others, so far".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Counts {
    matching_keys: usize,
    delivered_in_window: usize,
    burst_size: usize,
}

impl Counts {
    fn of(
        window: &RecentWindow,
        current: NotificationId,
        dedupe_key: &str,
        group_key: &str,
    ) -> Self {
        let mut matching_keys = 0_usize;
        let mut burst_size = 0_usize;
        for row in &window.rows {
            if row.id == current {
                continue;
            }
            if row.dedupe_key.as_deref() == Some(dedupe_key) {
                matching_keys += 1;
            }
            if row.group_key.as_deref() == Some(group_key) {
                burst_size += 1;
            }
        }
        Self {
            matching_keys,
            delivered_in_window: window.delivered_in_window,
            burst_size,
        }
    }
}

/// The subject kind the schema's constraint accepts, when the event has one.
fn subject_kind(event: NotificationEvent) -> Option<SubjectKind> {
    match event {
        NotificationEvent::NewEpisodeAvailable | NotificationEvent::SubtitleDownloadFinished => {
            Some(SubjectKind::Title)
        }
        NotificationEvent::DownloadCompleted
        | NotificationEvent::DownloadFailed
        | NotificationEvent::TorrentBlocked => Some(SubjectKind::Download),
        NotificationEvent::UpdateAvailable => Some(SubjectKind::Update),
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
                crate::notify::EventPreference {
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
