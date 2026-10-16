//! The event bus: the topics the backend emits, coalesced to a bounded rate.
//!
//! ARCHITECTURE §7 rule 4 makes an event a namespaced, versioned topic, and rule 5
//! makes the high-frequency ones coalesced: a producer may publish at any rate, and
//! the bus delivers at most one update per topic and key per tick, with the counters
//! to prove it. That is what keeps a torrent's 1 kHz stats stream from becoming a 1 kHz
//! IPC stream, and [`EventStats`] is how the UI learns that updates were discarded
//! rather than silently rendering stale numbers (rule 5 and PG-E2-11).
//!
//! The bus is the producer's whole interface: [`EventBus::publish`] never blocks, never
//! fails, and never awaits, so a job can publish from inside its work without a channel
//! to fill up. Delivery is the sink's business — `peregrine-ipc` supplies the Tauri
//! sink, and tests supply a recording one.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use tracing::warn;

/// How often each topic's pending updates are delivered.
///
/// ARCHITECTURE §14 budgets IPC progress at 4 Hz per job, so a tick at the same period
/// is the coarsest cadence that still meets the budget. The first tick is one period
/// away rather than immediate, so a one-second window holds at most four deliveries
/// rather than five.
const TICK: Duration = Duration::from_millis(250);

/// How many distinct `(topic, key)` updates may wait for one tick.
///
/// A key is a live subject — a job, a title, a device — so this bounds the map by the
/// number of things the app is working on. Past it the newest update for a *new* subject
/// is discarded and counted, rather than growing without limit; an update for a key
/// already present is never discarded, because that is the value the UI is waiting for.
const PENDING_CAPACITY: usize = 4_096;

/// One namespaced, versioned event stream.
///
/// The name is validated in the constructor, which is `const`, so a malformed topic
/// fails the build rather than the delivery:
///
/// ```compile_fail
/// # use peregrine_app::events::Topic;
/// // No namespace separator.
/// const BAD: Topic = Topic::new("progress", 1);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Topic {
    name: &'static str,
    version: u16,
}

impl Topic {
    /// Declares a topic.
    ///
    /// # Panics
    ///
    /// Panics when `name` is not `namespace://name` with a lowercase namespace and a
    /// lowercase, digit-or-underscore event name, or when `version` is zero. Used in a
    /// `const`, the panic is a compile error.
    #[must_use]
    pub const fn new(name: &'static str, version: u16) -> Self {
        assert!(
            is_namespaced(name),
            "an event topic must be `namespace://name`"
        );
        assert!(version > 0, "an event topic version starts at one");
        Self { name, version }
    }

    /// The wire name, for example `job://progress`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        self.name
    }

    /// The payload schema version for this topic.
    #[must_use]
    pub const fn version(self) -> u16 {
        self.version
    }
}

/// Whether `name` is `namespace://event`, in lowercase ASCII.
const fn is_namespaced(name: &str) -> bool {
    let bytes = name.as_bytes();
    let mut index = 0;
    while index < bytes.len() && bytes[index].is_ascii_lowercase() {
        index += 1;
    }
    if index == 0 || index + 3 >= bytes.len() {
        return false;
    }
    if bytes[index] != b':' || bytes[index + 1] != b'/' || bytes[index + 2] != b'/' {
        return false;
    }
    index += 3;
    if !bytes[index].is_ascii_lowercase() {
        return false;
    }
    while index < bytes.len() {
        let byte = bytes[index];
        if !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_') {
            return false;
        }
        index += 1;
    }
    true
}

/// One job's progress through its current attempt.
pub const JOB_PROGRESS: Topic = Topic::new("job://progress", 1);

/// One job's lifecycle phase.
pub const JOB_STATE: Topic = Topic::new("job://state", 1);

/// One download's transfer statistics.
///
/// Moved here from `acquisition.rs` when the manifest became the gate for the generated
/// TypeScript: a topic the backend publishes has to be a topic the UI can name, or the Downloads
/// screen can only poll. It is a move, not a redesign.
pub const DOWNLOAD_STATS: Topic = Topic::new("download://stats", 1);

/// A notification was recorded in the centre.
///
/// A trigger to re-read, never a source of truth: the payload carries no copy, so a subscriber
/// cannot render a row the centre has not answered with.
pub const NOTIFICATION_RAISED: Topic = Topic::new("notification://raised", 1);

/// Every topic this build emits, declared once.
///
/// `cargo xtask gen-ipc` writes this list into the generated TypeScript, so a topic the
/// backend emits is a topic the UI can name, and the drift gate fails the build when the
/// two disagree. `topics_are_pinned` fails when one is renamed, added, or renumbered.
pub const TOPICS: &[Topic] = &[JOB_PROGRESS, JOB_STATE, DOWNLOAD_STATS, NOTIFICATION_RAISED];

/// One value on one topic.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// The stream this belongs to.
    pub topic: Topic,
    /// What the update is about.
    ///
    /// Two events with the same topic and key coalesce: the later one replaces the
    /// earlier, so a progress stream delivers its newest value and not a backlog.
    pub key: String,
    /// The payload, already shaped for the UI. Never a secret (ARCHITECTURE §12).
    pub payload: Value,
}

impl Event {
    /// Builds an event on `topic` about `key`.
    #[must_use]
    pub fn new(topic: Topic, key: impl Into<String>, payload: Value) -> Self {
        Self {
            topic,
            key: key.into(),
            payload,
        }
    }
}

/// Where a batch is delivered.
///
/// The bus is transport-agnostic on purpose: `peregrine-ipc` owns the Tauri emitter and
/// a test owns a recording sink, and neither has to know about the other.
pub trait EventSink: Send + Sync + 'static {
    /// Delivers one event, reporting whether the transport accepted it.
    fn emit(&self, event: &Event) -> Result<(), EventSinkError>;
}

/// Why a sink could not deliver an event.
///
/// The message is for the log, not for the user: a dropped event is a degraded live
/// view, and the UI learns about it from [`EventStats`] rather than from an error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct EventSinkError {
    /// What the transport said.
    pub message: String,
}

impl EventSinkError {
    /// Records a transport failure.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// What the bus has done since it started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EventStats {
    /// Updates accepted from producers.
    pub published: u64,
    /// Updates replaced by a newer one for the same topic and key before delivery.
    pub coalesced: u64,
    /// Updates the sink accepted.
    pub delivered: u64,
    /// Updates the sink refused.
    pub failed: u64,
    /// Updates discarded because too many subjects were already waiting.
    pub dropped: u64,
}

impl EventStats {
    /// Whether this process is not showing everything it produced.
    ///
    /// The UI renders a degraded live indicator from this rather than pretending the
    /// numbers it has are current.
    #[must_use]
    pub const fn is_degraded(&self) -> bool {
        self.dropped > 0 || self.failed > 0
    }
}

/// The coalescing event bus.
///
/// One task owns the tick; producers only take a lock, insert, and return.
pub struct EventBus {
    inner: Arc<Inner>,
    shutdown: CancellationToken,
    engine: Mutex<Option<JoinHandle<()>>>,
}

impl EventBus {
    /// Starts a bus that delivers to `sink`.
    ///
    /// # Panics
    ///
    /// Panics when called outside a Tokio runtime, because the tick is a spawned task.
    #[must_use]
    pub fn new(sink: Arc<dyn EventSink>) -> Self {
        let inner = Arc::new(Inner {
            pending: Mutex::new(HashMap::new()),
            sink,
            counters: Counters::default(),
        });
        let shutdown = CancellationToken::new();
        // The ticker is built here rather than inside the task so its origin is the
        // moment the bus exists. A ticker created by the task would start when the task
        // is first polled — up to a scheduling delay later — and the delivered rate would
        // depend on when the runtime happened to get around to it.
        let mut ticker = tokio::time::interval_at(Instant::now() + TICK, TICK);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let engine = tokio::spawn(run(Arc::clone(&inner), shutdown.clone(), ticker));
        Self {
            inner,
            shutdown,
            engine: Mutex::new(Some(engine)),
        }
    }

    /// Accepts one update.
    ///
    /// Never blocks, never fails, and never awaits: a producer that is faster than the
    /// tick overwrites its own previous update, which is what coalescing means.
    pub fn publish(&self, event: Event) {
        let counters = &self.inner.counters;
        counters.published.fetch_add(1, Ordering::Relaxed);

        // A poisoned lock means a producer panicked inside the map insert, which is a
        // straight-line insert with no user code in it; recovering the map is better
        // than dropping every later event on the floor.
        let mut pending = match self.inner.pending.lock() {
            Ok(pending) => pending,
            Err(poisoned) => poisoned.into_inner(),
        };
        let key = (event.topic, event.key.clone());
        if pending.contains_key(&key) {
            counters.coalesced.fetch_add(1, Ordering::Relaxed);
        } else if pending.len() >= PENDING_CAPACITY {
            counters.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        pending.insert(key, event);
    }

    /// A consistent-enough snapshot of the counters.
    ///
    /// Each counter is read once, so a concurrent publish can land between two of them;
    /// every field is monotonic, which is all a degraded indicator needs.
    #[must_use]
    pub fn stats(&self) -> EventStats {
        self.inner.counters.snapshot()
    }

    /// Whether any update has been discarded or refused.
    #[must_use]
    pub fn is_degraded(&self) -> bool {
        self.stats().is_degraded()
    }

    /// Delivers what is pending and stops the tick.
    ///
    /// Idempotent, and safe to call from a shared reference: the application holds one
    /// `Arc<EventBus>` for both publishing and shutdown.
    pub async fn shutdown(&self) {
        self.shutdown.cancel();
        let engine = self.engine.lock().ok().and_then(|mut slot| slot.take());
        if let Some(engine) = engine {
            let _ = engine.await;
        }
    }
}

impl Drop for EventBus {
    fn drop(&mut self) {
        // Cancelling is what ends the task; without it the task would hold `Inner` and
        // the map for the life of the process. `Drop` cannot await, so the join is
        // detached and `shutdown` is the way to observe the final delivery.
        self.shutdown.cancel();
        let engine = self.engine.lock().ok().and_then(|mut slot| slot.take());
        if let Some(engine) = engine
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            runtime.spawn(async move {
                let _ = engine.await;
            });
        }
    }
}

/// What the bus task owns together with the producers.
struct Inner {
    pending: Mutex<HashMap<(Topic, String), Event>>,
    sink: Arc<dyn EventSink>,
    counters: Counters,
}

impl Inner {
    /// Delivers every pending update, newest per key.
    fn flush(&self) {
        let mut batch: Vec<Event> = {
            let mut pending = match self.pending.lock() {
                Ok(pending) => pending,
                Err(poisoned) => poisoned.into_inner(),
            };
            pending.drain().map(|(_, event)| event).collect()
        };
        // `HashMap` iteration order is arbitrary, so the batch is ordered by topic and
        // key before delivery. Without this, two runs of the same test deliver the same
        // events in different orders and nothing about delivery is reproducible.
        batch.sort_by(|left, right| {
            (left.topic.name(), &left.key).cmp(&(right.topic.name(), &right.key))
        });

        for event in batch {
            match self.sink.emit(&event) {
                Ok(()) => {
                    self.counters.delivered.fetch_add(1, Ordering::Relaxed);
                }
                Err(error) => {
                    self.counters.failed.fetch_add(1, Ordering::Relaxed);
                    warn!(
                        topic = event.topic.name(),
                        key = %event.key,
                        %error,
                        "an event could not be delivered"
                    );
                }
            }
        }
    }
}

/// Delivers every pending update once per tick until the bus is stopped.
///
/// The tick is the only clock in the bus: a producer's rate never enters into it, which
/// is what makes the delivered rate a property of the subscriber's budget rather than of
/// whichever job happens to be running.
async fn run(inner: Arc<Inner>, shutdown: CancellationToken, mut ticker: tokio::time::Interval) {
    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => break,
            _ = ticker.tick() => inner.flush(),
        }
    }
    // The final delivery: whatever arrived between the last tick and the shutdown is
    // sent before the sink goes away, so stopping the app does not lose the last state.
    inner.flush();
}

/// The bus's counters.
#[derive(Debug, Default)]
struct Counters {
    published: AtomicU64,
    coalesced: AtomicU64,
    delivered: AtomicU64,
    failed: AtomicU64,
    dropped: AtomicU64,
}

impl Counters {
    fn snapshot(&self) -> EventStats {
        EventStats {
            published: self.published.load(Ordering::Relaxed),
            coalesced: self.coalesced.load(Ordering::Relaxed),
            delivered: self.delivered.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::RecordingSink;
    use serde_json::json;

    fn bus() -> (Arc<EventBus>, Arc<RecordingSink>) {
        let sink = Arc::new(RecordingSink::default());
        (
            Arc::new(EventBus::new(Arc::clone(&sink) as Arc<dyn EventSink>)),
            sink,
        )
    }

    fn progress(key: &str, permille: u64) -> Event {
        Event::new(JOB_PROGRESS, key, json!({ "progressPermille": permille }))
    }

    /// The names and versions are the UI's contract, so a rename is a deliberate edit
    /// here rather than an accident in a producer.
    #[test]
    fn topics_are_pinned() {
        let rendered: Vec<(&str, u16)> = TOPICS
            .iter()
            .map(|topic| (topic.name(), topic.version()))
            .collect();
        assert_eq!(
            rendered,
            vec![
                ("job://progress", 1),
                ("job://state", 1),
                ("download://stats", 1),
                ("notification://raised", 1),
            ],
            "the event manifest changed; regenerate the bindings in the same commit"
        );

        for topic in TOPICS {
            assert!(
                is_namespaced(topic.name()),
                "{topic:?} is not a namespaced topic"
            );
        }
    }

    #[test]
    fn the_manifest_has_no_duplicate_topic() {
        let mut names: Vec<&str> = TOPICS.iter().map(|topic| topic.name()).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "two topics share a name");
    }

    /// The construction rule, exercised without the compile-fail harness: the
    /// `compile_fail` doctest above pins the const path, and this pins the predicate.
    #[test]
    fn topic_names_must_be_lowercase_and_namespaced() {
        for rejected in [
            "",
            "job",
            "job:progress",
            "job:/progress",
            "job//progress",
            "job://",
            "job://Progress",
            "Job://progress",
            "job://pro gress",
            "job://pro/gress",
            "://progress",
        ] {
            assert!(!is_namespaced(rejected), "{rejected:?} must be rejected");
        }
        for accepted in [
            "job://progress",
            "download://stats",
            "library://changed",
            "notification://raised",
            "job://progress_v2",
        ] {
            assert!(is_namespaced(accepted), "{accepted:?} must be accepted");
        }
    }

    /// PG-E2-05: a thousand updates a second become at most four deliveries a second.
    #[tokio::test(start_paused = true)]
    async fn a_thousand_updates_a_second_deliver_at_four_hertz() {
        let (bus, sink) = bus();

        // One second of a 1 kHz producer, on a paused clock so the test is exact rather
        // than a race against the machine.
        for tick in 0..1_000 {
            bus.publish(progress("job-1", tick));
            tokio::time::advance(Duration::from_millis(1)).await;
            tokio::task::yield_now().await;
        }

        let delivered = sink.delivered();
        assert!(
            delivered.len() <= 4,
            "one second must hold at most four deliveries, got {}",
            delivered.len()
        );
        assert!(
            !delivered.is_empty(),
            "the UI must still receive the stream, not silence"
        );

        // The last delivery is the newest value, not an early one: coalescing keeps the
        // most recent update rather than a backlog.
        let stats = bus.stats();
        assert_eq!(stats.published, 1_000);
        assert_eq!(
            usize::try_from(stats.delivered).unwrap(),
            delivered.len(),
            "every accepted delivery must be counted"
        );
        assert!(
            stats.coalesced >= 900,
            "almost every update must be recorded as coalesced, got {stats:?}"
        );
        assert_eq!(stats.dropped, 0, "keys are reused, so nothing is dropped");

        let last = delivered.last().expect("a delivery happened");
        assert_eq!(
            last.payload,
            json!({ "progressPermille": 999 }),
            "the delivered value must be the newest one"
        );

        bus.shutdown().await;
    }

    /// Coalescing is per topic and key, not global: two jobs both stay live.
    #[tokio::test(start_paused = true)]
    async fn updates_coalesce_per_key_and_not_across_keys() {
        let (bus, sink) = bus();
        bus.publish(progress("job-1", 10));
        bus.publish(progress("job-1", 20));
        bus.publish(progress("job-2", 30));
        bus.publish(Event::new(
            JOB_STATE,
            "job-1",
            json!({ "phase": "running" }),
        ));

        tokio::time::advance(TICK).await;
        tokio::task::yield_now().await;

        let delivered = sink.delivered();
        assert_eq!(delivered.len(), 3, "one per (topic, key): {delivered:#?}");
        let progress: Vec<&Event> = delivered
            .iter()
            .filter(|event| event.topic == JOB_PROGRESS)
            .collect();
        assert_eq!(progress.len(), 2);
        assert_eq!(
            progress[0].payload,
            json!({ "progressPermille": 20 }),
            "the newest value for a key wins"
        );
        assert_eq!(bus.stats().coalesced, 1);

        bus.shutdown().await;
    }

    /// Shutting down delivers what is pending: stopping the app must not lose the last
    /// state, and the tick must not still be running afterwards.
    #[tokio::test]
    async fn shutdown_delivers_the_pending_batch_and_stops() {
        let (bus, sink) = bus();
        bus.publish(progress("job-1", 42));

        bus.shutdown().await;
        assert_eq!(
            sink.delivered().len(),
            1,
            "the final flush must deliver what was pending"
        );

        // Idempotent, and publishing after shutdown is accepted and simply never
        // delivered: the bus is not a channel that can be closed on a producer.
        bus.shutdown().await;
        bus.publish(progress("job-1", 43));
        assert_eq!(bus.stats().published, 2);
    }

    /// PG-E2-11's signal: a refused delivery is counted and the bus reports degraded.
    #[tokio::test(start_paused = true)]
    async fn a_refused_delivery_is_counted_as_degraded() {
        let sink = Arc::new(RecordingSink::refusing());
        let bus = Arc::new(EventBus::new(Arc::clone(&sink) as Arc<dyn EventSink>));

        bus.publish(progress("job-1", 1));
        assert!(!bus.is_degraded(), "nothing has been attempted yet");

        tokio::time::advance(TICK).await;
        tokio::task::yield_now().await;

        let stats = bus.stats();
        assert_eq!(stats.delivered, 0);
        assert_eq!(stats.failed, 1);
        assert!(
            bus.is_degraded(),
            "a refused event must surface as a degraded live view"
        );

        bus.shutdown().await;
    }

    /// The capacity guard: a flood of distinct subjects is bounded, and the discard is
    /// counted rather than silent.
    #[tokio::test]
    async fn more_subjects_than_the_capacity_drops_and_counts() {
        let (bus, _sink) = bus();
        for index in 0..(PENDING_CAPACITY + 10) {
            bus.publish(progress(&format!("job-{index}"), 1));
        }

        let stats = bus.stats();
        assert_eq!(stats.dropped, 10);
        assert_eq!(
            stats.coalesced, 0,
            "each key is distinct, so nothing coalesced"
        );
        assert!(stats.is_degraded());

        bus.shutdown().await;
    }

    /// The empty case: a bus that never receives anything delivers nothing and does not
    /// panic on shutdown.
    #[tokio::test(start_paused = true)]
    async fn an_idle_bus_delivers_nothing() {
        let (bus, sink) = bus();
        tokio::time::advance(TICK * 3).await;
        tokio::task::yield_now().await;

        assert!(
            sink.delivered().is_empty(),
            "an idle bus must deliver nothing"
        );
        assert_eq!(bus.stats(), EventStats::default());
        bus.shutdown().await;
    }
}
