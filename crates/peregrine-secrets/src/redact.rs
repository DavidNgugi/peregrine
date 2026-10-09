//! The filter that removes registered secret values from formatted events.
//!
//! A secret can reach a log line by accident from anywhere — an interpolated
//! `expose_secret()`, a provider error message, a request body that was
//! serialized for debugging — so the removal happens at the one place every
//! emitted event passes through: after the event is formatted and before a byte
//! is written. Redacting there rather than at the write call means the removal
//! sees the same string the file receives, including the JSON escaping the
//! formatter applies.

use std::borrow::Cow;
use std::fmt;
use std::sync::{LazyLock, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::FmtContext;
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::registry::LookupSpan;
use zeroize::Zeroizing;

/// What replaces a registered value.
const REDACTED: &str = "[REDACTED]";

/// Every value registered through [`crate::Secret::new`].
///
/// The set is process-wide rather than a handle threaded to each call site: a
/// value can be logged from any module, and the layer every event passes through
/// is the only component that sees them all. Each entry is a second copy of the
/// value in memory, so each is held in a [`Zeroizing`] buffer and wiped when the
/// registry is dropped.
static REGISTRY: LazyLock<RwLock<Vec<Zeroizing<String>>>> =
    LazyLock::new(|| RwLock::new(Vec::new()));

/// Registers `value` so that no future event can contain it.
///
/// Called by [`crate::Secret::new`]. An empty value is ignored: it matches at
/// every position, so registering it would replace whole lines rather than
/// remove a secret.
pub(crate) fn register(value: &str) {
    if value.is_empty() {
        return;
    }

    let mut values = write_lock();
    let raw = Zeroizing::new(value.to_owned());
    let offset = offset_for(&values, raw.len());
    values.insert(offset, raw);

    // A JSON line carries the escaped form, so a secret containing a quote, a
    // backslash, or a control character appears escaped and the raw value alone
    // would not match it.
    let escaped = json_escaped(value);
    if escaped != value {
        let escaped = Zeroizing::new(escaped);
        let offset = offset_for(&values, escaped.len());
        values.insert(offset, escaped);
    }
}

/// Replaces every registered value in `line` with the marker.
///
/// Borrows when nothing matches, so the common case does not allocate.
pub(crate) fn scrub(line: &str) -> Cow<'_, str> {
    let values = read_lock();
    let mut scrubbed: Cow<'_, str> = Cow::Borrowed(line);
    for value in values.iter() {
        if scrubbed.contains(value.as_str()) {
            scrubbed = Cow::Owned(scrubbed.replace(value.as_str(), REDACTED));
        }
    }
    scrubbed
}

/// The position that keeps `values` ordered longest first.
///
/// Longest first is what stops a registered prefix from leaving the tail of a
/// longer registered secret in the line.
fn offset_for(values: &[Zeroizing<String>], length: usize) -> usize {
    values.partition_point(|entry| entry.len() >= length)
}

/// The form `serde_json` writes for `value`, without the surrounding quotes.
///
/// Mirrors the default escape table `serde_json` uses for strings, which is what
/// `tracing-subscriber`'s JSON formatter serializes a field value through.
fn json_escaped(value: &str) -> String {
    use std::fmt::Write as _;

    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\u{8}' => escaped.push_str("\\b"),
            '\t' => escaped.push_str("\\t"),
            '\n' => escaped.push_str("\\n"),
            '\u{c}' => escaped.push_str("\\f"),
            '\r' => escaped.push_str("\\r"),
            control if control < ' ' => {
                // Writing into a `String` cannot fail.
                let _ = write!(escaped, "\\u{:04x}", control as u32);
            }
            other => escaped.push(other),
        }
    }
    escaped
}

/// The registry for reading; a poisoned lock still holds a usable set.
fn read_lock() -> RwLockReadGuard<'static, Vec<Zeroizing<String>>> {
    REGISTRY.read().unwrap_or_else(PoisonError::into_inner)
}

/// The registry for writing; a poisoned lock still holds a usable set.
fn write_lock() -> RwLockWriteGuard<'static, Vec<Zeroizing<String>>> {
    REGISTRY.write().unwrap_or_else(PoisonError::into_inner)
}

/// A [`FormatEvent`] wrapper that removes every registered secret from the
/// formatted event.
///
/// The application builds it over its JSON formatter:
/// `RedactingFormat::new(Format::default().json())`.
pub struct RedactingFormat<F> {
    inner: F,
}

impl<F> RedactingFormat<F> {
    /// Wraps `inner` so its output is scrubbed before it reaches the writer.
    pub fn new(inner: F) -> Self {
        Self { inner }
    }
}

impl<S, N, F> FormatEvent<S, N> for RedactingFormat<F>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> FormatFields<'writer> + 'static,
    F: FormatEvent<S, N>,
{
    fn format_event(
        &self,
        context: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let mut line = String::new();
        self.inner
            .format_event(context, Writer::new(&mut line), event)?;
        writer.write_str(&scrub(&line))
    }
}
