//! The one place input from outside the running process enters the application.
//!
//! Two things produce it, and both arrive as text this process did not write:
//!
//! * a second launch, whose argument vector `tauri-plugin-single-instance` hands over
//!   (PG-E1-04), and this process's own vector on a cold start (PG-E1-11);
//! * a `peregrine://` URL the OS hands over, by `tauri-plugin-deep-link` on macOS,
//!   where Launch Services delivers it as an Apple Event that never appears in `argv`.
//!
//! Both funnel into [`forward`] and [`forward_urls`], which are the same router, so
//! there is exactly one path from "something asked us to open a link" to "the link was
//! classified".
//!
//! # What may be logged
//!
//! A deep link can carry a magnet URI, and the diagnostics log tail is copied out of the
//! application (SECURITY.md T13). So a refusal never logs the refused text, the parsed
//! [`ExternalLink`] is logged as its *kind*, and the only value that reaches an event
//! payload is the canonical `peregrine://title/...` link this application built itself.
//! A magnet reaches neither: it is validated, refused for the reason the threat model
//! gives, and dropped ([`peregrine_domain::ExternalLink::Magnet`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use peregrine_domain::{ExternalLink, deeplink};
use tauri::{Emitter as _, Manager as _, Runtime};

/// The event a routed deep link reaches the webview on.
///
/// A plain Tauri event rather than a typed bus topic: what arrives is a URL the OS or a
/// web page supplied, and the UI's routing table (PG-E7-02) is what will consume it.
const DEEP_LINK_EVENT: &str = "external://deep-link";

/// Arguments from outside the running process, and the directory they arrived relative to.
///
/// The working directory is part of the payload because a path argument is commonly
/// relative to the directory the second launch happened in, and it is not recoverable
/// later: by the time a handler runs, the process's own working directory is the first
/// instance's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalInput {
    /// The raw argument vector, including the executable path, exactly as the second
    /// launch received it.
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

/// Routes the links in an argument vector. See [`forward`].
pub fn forward<R: Runtime>(app: &tauri::AppHandle<R>, input: &ExternalInput) -> bool {
    let payload = input.payload();
    if payload.is_empty() {
        return false;
    }
    route_all(app, payload.iter().map(String::as_str));
    true
}

/// Routes the URLs the OS handed over while this process was running or starting it.
///
/// A separate entry point rather than an [`ExternalInput`]: a Launch Services open is not
/// an argument vector, and pretending it is would mean an executable path that does not
/// exist and a working directory that was never used.
pub fn forward_urls<R: Runtime>(app: &tauri::AppHandle<R>, urls: &[String]) -> bool {
    if urls.is_empty() {
        return false;
    }
    route_all(app, urls.iter().map(String::as_str));
    true
}

/// Classifies every candidate and hands each one to its outcome.
///
/// One bad argument does not stop the others: a shell can pass a flag beside a URL, and a
/// launch that carried one usable link should still open it.
fn route_all<'a, R: Runtime>(app: &tauri::AppHandle<R>, candidates: impl Iterator<Item = &'a str>) {
    let mut routed = 0_u32;
    let mut refused = 0_u32;
    for candidate in candidates {
        match deeplink::parse(candidate) {
            Ok(ExternalLink::Title(link)) => {
                routed = routed.saturating_add(1);
                tracing::info!(route = "title", "an external deep link routed");
                delivery(app).deliver(app, link.as_str());
            }
            Ok(ExternalLink::Magnet) => {
                // SECURITY.md T12: a magnet may not be added before a confirmation that
                // shows the inspection report, and no torrent engine exists to produce
                // one (PG-E10). The infohash is not logged, stored, or emitted.
                refused = refused.saturating_add(1);
                tracing::info!(
                    route = "magnet",
                    "a magnet deep link was refused: adding one needs the inspection gate (SECURITY.md T12) and no torrent engine is composed in yet"
                );
            }
            Err(reason) => {
                refused = refused.saturating_add(1);
                // The reason's `Display` is a fixed sentence; it never quotes the input
                // (SECURITY.md T13).
                tracing::debug!(%reason, "an argument from outside the process was not a deep link");
            }
        }
    }

    if routed > 0 || refused > 0 {
        tracing::debug!(routed, refused, "external input was classified");
    }
}

/// The delivery state, so a link that arrives before the webview has loaded is not lost.
///
/// The window is created before `setup` runs but its document loads after it, and an event
/// emitted into a webview with no listener is gone. A cold start is exactly the case where
/// the link arrives first: the process was launched *with* the URL, so `setup` sees it
/// before there is anything listening. Holding the last link until the first page load is
/// what makes the cold and warm paths behave the same.
#[derive(Default)]
pub(crate) struct Delivery {
    page_loaded: AtomicBool,
    before_load: Mutex<Option<String>>,
}

impl Delivery {
    /// Sends the link now, and keeps it until the first page load if the page has not
    /// loaded yet.
    fn deliver<R: Runtime>(&self, app: &tauri::AppHandle<R>, link: &str) {
        if !self.page_loaded.load(Ordering::Relaxed) {
            self.remember(link);
        }
        emit(app, link);
    }

    /// Releases whatever arrived before the page was ready.
    pub(crate) fn flush<R: Runtime>(&self, app: &tauri::AppHandle<R>) {
        self.page_loaded.store(true, Ordering::Relaxed);
        let pending = self
            .before_load
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(link) = pending {
            emit(app, &link);
        }
    }

    fn remember(&self, link: &str) {
        let mut slot = self
            .before_load
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *slot = Some(link.to_owned());
    }

    /// The link waiting for the first page load, for the tests.
    #[cfg(test)]
    fn pending(&self) -> Option<String> {
        self.before_load
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// The delivery state, which `setup` installs before any input can arrive.
fn delivery<R: Runtime>(app: &tauri::AppHandle<R>) -> tauri::State<'_, Delivery> {
    app.state::<Delivery>()
}

/// Announces the link to the webview.
///
/// A failure is logged and not propagated: the window may be on its way down, and a link
/// that cannot be delivered is not a reason to stop routing the rest of a launch.
fn emit<R: Runtime>(app: &tauri::AppHandle<R>, link: &str) {
    if let Err(error) = app.emit(DEEP_LINK_EVENT, link) {
        tracing::warn!(%error, "a routed deep link could not be announced to the window");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock() -> tauri::App<tauri::test::MockRuntime> {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("a mock application");
        app.manage(Delivery::default());
        app
    }

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

    /// The cold-start half of PG-E1-11: the link is in this process's own argument vector,
    /// and the window has not loaded a document yet, so it has to be held rather than
    /// emitted into nothing.
    #[test]
    fn a_cold_start_title_link_is_held_until_the_page_loads() {
        let app = mock();
        let input = ExternalInput::new(
            vec![
                "/usr/local/bin/peregrine".to_owned(),
                "peregrine://title/tmdb/123".to_owned(),
            ],
            String::new(),
        );

        assert!(forward(app.handle(), &input));
        let delivery = app.state::<Delivery>();
        assert_eq!(
            delivery.pending().as_deref(),
            Some("peregrine://title/tmdb/123"),
            "the canonical link is what waits for the window"
        );
    }

    /// A magnet is classified and refused: nothing is emitted, nothing is remembered, and
    /// no part of the URI is kept (SECURITY.md T12, T13).
    #[test]
    fn a_magnet_link_is_refused_and_never_reaches_the_window() {
        let app = mock();
        let input = ExternalInput::new(
            vec![
                "/usr/local/bin/peregrine".to_owned(),
                "peregrine://magnet?xt=urn:btih:0123456789abcdef0123456789abcdef01234567"
                    .to_owned(),
            ],
            String::new(),
        );

        assert!(forward(app.handle(), &input), "the argument was classified");
        assert_eq!(
            app.state::<Delivery>().pending(),
            None,
            "a magnet must not become an event payload"
        );
    }

    #[test]
    fn an_argument_that_is_not_a_link_is_refused_quietly() {
        let app = mock();
        let input = ExternalInput::new(
            vec![
                "/usr/local/bin/peregrine".to_owned(),
                "--no-tray".to_owned(),
            ],
            String::new(),
        );

        assert!(forward(app.handle(), &input));
        assert_eq!(app.state::<Delivery>().pending(), None);
    }

    /// The warm path: the page is loaded, so the link goes straight out and is not left
    /// waiting for a page load that has already happened.
    #[test]
    fn a_link_arriving_after_the_page_loaded_is_not_held() {
        let app = mock();
        app.state::<Delivery>().flush(app.handle());

        assert!(forward_urls(
            app.handle(),
            &["peregrine://title/tmdb/77".to_owned()]
        ));
        assert_eq!(app.state::<Delivery>().pending(), None);
    }

    #[test]
    fn a_link_that_arrives_before_the_page_loads_is_released_once() {
        let app = mock();
        let delivery = app.state::<Delivery>();
        delivery.remember("peregrine://title/tmdb/1");

        delivery.flush(app.handle());
        assert_eq!(
            delivery.pending(),
            None,
            "the link is released, not repeated"
        );
    }

    #[test]
    fn no_arguments_is_not_something_to_route() {
        let app = mock();
        assert!(!forward(
            app.handle(),
            &ExternalInput::new(Vec::new(), String::new())
        ));
        assert!(!forward_urls(app.handle(), &[]));
    }
}
