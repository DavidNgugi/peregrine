//! The one place input from outside the running process enters the application.
//!
//! Two things produce it: a second launch, whose arguments are handed over by
//! `tauri-plugin-single-instance`, and a `peregrine://` deep link, which is PG-E1-11 and
//! does not exist yet. Both are the same event — a list of arguments that came from
//! somewhere other than this process's own UI — so both will go through [`forward`], which
//! is why the single-instance callback calls it today even though the deep-link plugin is
//! not installed. Adding PG-E1-11 means calling `forward` from its handler, not inventing a
//! second path.
//!
//! **Not wired**: what [`forward`] does with the input. Routing `peregrine://title/...` or
//! a magnet URI into the app needs a handler that does not exist (PG-E1-11), and nothing
//! may be added to the library before the inspection report and the confirmation the threat
//! model requires (SECURITY.md T12), so there is nothing this phase could safely do with
//! the arguments. They are logged — at the argument *count*, never the values, because a
//! deep link can carry a magnet URI and a log line is read by the diagnostics export
//! (SECURITY.md T13) — and the seam is where PG-E1-11's handler will be called.

use tauri::Runtime;

/// Arguments from outside the running process, and the directory they arrived relative to.
///
/// The working directory is part of the payload because a path argument is commonly
/// relative to the directory the second launch happened in, and it is not recoverable
/// later: by the time a handler runs, the process's own working directory is the first
/// instance's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalInput {
    /// The raw argument vector, including the executable path, exactly as the second
    /// launch received it. Not parsed: a deep-link URI is one element of it, and which
    /// element is PG-E1-11's question.
    pub arguments: Vec<String>,
    /// The working directory the second launch was started from.
    pub working_directory: String,
}

impl ExternalInput {
    /// Builds the payload from what the single-instance plugin hands over.
    #[must_use]
    pub fn new(arguments: Vec<String>, working_directory: String) -> Self {
        Self {
            arguments,
            working_directory,
        }
    }

    /// The arguments after the executable path, which is the first element `std::env::args`
    /// yields and never an instruction.
    ///
    /// A launch with no arguments after the program name is not forwarded at all
    /// ([`forward`]), because focusing the existing window is the whole effect of a plain
    /// second launch.
    #[must_use]
    pub fn payload(&self) -> &[String] {
        self.arguments.get(1..).unwrap_or_default()
    }
}

/// Hands input from outside the process to the handler that routes it.
///
/// Returns whether there was anything to route. A plain second launch — no argument beyond
/// the executable — returns `false`, and the caller's only job is then to raise the window.
///
/// # Not yet wired
///
/// PG-E1-11 is the handler. Until it exists this records the shape of what arrived and
/// nothing else, and the arguments are deliberately not part of the log line: a deep link
/// can carry a magnet URI, and the diagnostics log tail is copied out of the app
/// (SECURITY.md T13).
pub fn forward<R: Runtime>(_app: &tauri::AppHandle<R>, input: &ExternalInput) -> bool {
    let payload = input.payload();
    if payload.is_empty() {
        return false;
    }
    tracing::info!(
        arguments = payload.len(),
        // Whether the launch carried a working directory is worth a line; the arguments
        // themselves are not.
        has_working_directory = !input.working_directory.is_empty(),
        "external input arrived and has no handler yet (PG-E1-11)"
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_drops_the_executable_path() {
        let input = ExternalInput::new(
            vec![
                "/Applications/Peregrine.app/Contents/MacOS/peregrine".to_owned(),
                "peregrine://title/tmdb/123".to_owned(),
            ],
            "/Users/someone".to_owned(),
        );
        assert_eq!(input.payload(), ["peregrine://title/tmdb/123"]);
    }

    #[test]
    fn a_plain_second_launch_has_no_payload() {
        let input = ExternalInput::new(vec!["/usr/local/bin/peregrine".to_owned()], String::new());
        assert_eq!(input.payload(), [] as [String; 0]);
    }

    /// A launch with no arguments at all is not something `std::env::args` produces — the
    /// program name is always there — but the payload accessor is the one place a hostile
    /// or truncated vector could index out of bounds, so it is pinned.
    #[test]
    fn an_empty_argument_vector_has_an_empty_payload() {
        let input = ExternalInput::new(Vec::new(), String::new());
        assert_eq!(input.payload(), [] as [String; 0]);
    }
}
