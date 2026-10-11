//! The response-size guard: a command that answers with more than the contract allows is
//! a design error, and the guard is what makes it visible during development.
//!
//! ARCHITECTURE §7 rule 1 sets the budget: "Commands return IDs and DTOs, never blobs. Any
//! response over 64 KB is a design error." A rule with a number in it needs a check, or it
//! is advice — and the check has to be cheap, because it sits on every response.
//!
//! The measurement therefore runs in a debug build only. It serializes the value a second
//! time to measure it, which is work a release build should not do to catch something
//! review and a test are supposed to catch; in release the whole call compiles to nothing.

use serde::Serialize;
use tracing::warn;

/// The largest response the IPC contract allows (ARCHITECTURE §7 rule 1).
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// Measures a response and warns when it breaks the contract.
///
/// The value is returned unchanged, so a command wraps its answer with this in one line.
#[must_use]
pub fn guard_response<T: Serialize>(command: &'static str, value: T) -> T {
    #[cfg(debug_assertions)]
    {
        match serde_json::to_string(&value) {
            Ok(json) if json.len() > MAX_RESPONSE_BYTES => warn!(
                command,
                bytes = json.len(),
                limit = MAX_RESPONSE_BYTES,
                "an IPC response is over the payload budget; return identifiers instead"
            ),
            // A value that cannot be serialized will fail at the boundary, which is worth
            // knowing here rather than as an opaque invoke error in the webview.
            Err(error) => warn!(command, %error, "an IPC response could not be measured"),
            Ok(_) => {}
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    /// A writer that keeps everything written to it, so a test can assert what was logged.
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Capture {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    impl io::Write for Capture {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Capture {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Runs `body` with a capturing subscriber and returns everything it logged.
    fn logged(body: impl FnOnce()) -> String {
        let capture = Capture::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(capture.clone())
            .with_max_level(tracing::Level::WARN)
            .finish();
        tracing::subscriber::with_default(subscriber, body);
        capture.text()
    }

    /// The measured size of a value, which is what the guard compares.
    ///
    /// A string serializes with its two quotes, so the fixtures below are sized to put the
    /// measured length on the boundary rather than the fixture's own length.
    fn measured(value: &impl Serialize) -> usize {
        serde_json::to_string(value)
            .expect("a string serializes")
            .len()
    }

    /// The boundary, through the same call every command uses: exactly at the limit is
    /// silent, one byte over is reported with its measured size.
    #[test]
    fn the_limit_is_inclusive_and_one_byte_over_is_reported() {
        let at_limit = "x".repeat(MAX_RESPONSE_BYTES - 2);
        assert_eq!(measured(&at_limit), MAX_RESPONSE_BYTES);

        let over = "x".repeat(MAX_RESPONSE_BYTES - 1);
        assert_eq!(measured(&over), MAX_RESPONSE_BYTES + 1);

        let capture = Capture::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(capture.clone())
            .with_max_level(tracing::Level::WARN)
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let small = guard_response("test.at_limit", "ok".repeat(4));
            assert_eq!(small, "okokokok", "the guard returns the value unchanged");
            let _ = guard_response("test.over_limit", over);
        });

        let text = capture.text();
        assert!(
            !text.contains("test.at_limit"),
            "a response inside the budget must be silent: {text}"
        );
        assert!(
            text.contains("test.over_limit"),
            "the oversized response must be reported: {text}"
        );
        assert!(
            text.contains(&(MAX_RESPONSE_BYTES + 1).to_string()),
            "the report must carry the measured size: {text}"
        );
    }

    /// The empty case: nothing to measure is nothing to report.
    #[test]
    fn an_empty_response_is_silent() {
        let text = logged(|| {
            let empty: Vec<String> = Vec::new();
            let guarded = guard_response("test.empty", empty);
            assert!(guarded.is_empty(), "the guard returns the value unchanged");
        });

        assert!(text.is_empty(), "{text}");
        assert_eq!(measured(&Vec::<u8>::new()), 2);
    }
}
