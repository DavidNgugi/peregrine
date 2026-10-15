//! One playback session: which library file, where it is being served, and how it ends.
//!
//! **A session is the unit the UI is written against, and that is the point of this module** (ADR
//! [0023](../../../docs/adr/0023-in-app-video-surface.md) §4). What a screen needs to know is "is
//! something playing, what is it, and where do the bytes come from" — never which element is drawing
//! them. Today the answer to the last question is a loopback URL a `<video>` element follows; when the
//! mpv adapter lands (`PG-E16-02`, `PG-E16-03`) it answers with a surface instead, and no screen
//! changes. That is why the URL is a *field of the session* rather than something a screen builds from
//! a port number.
//!
//! **It owns the listener, so it owns the lifetime.** The socket is opened by [`PlaybackSession::open`]
//! and closed by [`PlaybackSession::close`] or by the value being dropped, whichever happens first, so
//! a session that is replaced cannot leave a server behind.

use std::sync::Arc;

use peregrine_domain::{SafePath, SessionToken};
use peregrine_stream::{Capture, MediaServer, StreamError, ThumbnailSink};
use thiserror::Error;

/// Why a session could not be opened.
#[derive(Debug, Error)]
pub enum PlaybackError {
    /// The file could not be served, which is a `404`-shaped fact arriving early and with a reason.
    ///
    /// Transparent rather than wrapped: [`StreamError`]'s variants already say what a user would need to
    /// know — gone, not a file, unreadable — and a second layer of wording would only add a sentence
    /// that says "playback failed" over the top of the one that says why.
    #[error(transparent)]
    Serve(#[from] StreamError),
}

/// One file, being served to this machine for as long as the session lasts.
pub struct PlaybackSession {
    media_file_id: i64,
    title_id: i64,
    url: String,
    /// Where a captured poster frame is posted, or `None` when the caller had nowhere to store one.
    thumbnail_url: Option<String>,
    server: MediaServer,
}

impl PlaybackSession {
    /// Opens a session for one of a title's files.
    ///
    /// `file` arrives as a [`SafePath`] — only `PathGuard` makes one — so a session cannot be opened for
    /// a path that has not been shown to be inside a configured root, and `token` is minted by a
    /// [`peregrine_domain::TokenSource`] rather than here, because randomness belongs to an adapter
    /// (`AGENTS R14`).
    ///
    /// `posters` is where a frame the renderer captures is stored. `None` opens a session that plays and
    /// keeps no poster, which is what a caller with no image cache gets: the session still serves the
    /// file, and the route that would store a frame is not offered (`ADR 0023`'s capture).
    ///
    /// # Errors
    ///
    /// [`PlaybackError::Serve`] when the file is gone, is not a file, cannot be read, or the loopback
    /// listener cannot be bound.
    pub async fn open(
        media_file_id: i64,
        title_id: i64,
        file: &SafePath,
        token: SessionToken,
        posters: Option<Arc<dyn ThumbnailSink>>,
    ) -> Result<Self, PlaybackError> {
        // The title is attached to the capture here rather than by the caller, because a caller that
        // supplied a sink for one title and a session for another would be a poster filed under the wrong
        // film — a defect nothing downstream could detect.
        let capture = posters.map(|sink| Capture { title_id, sink });
        let server = MediaServer::start(file, token, capture).await?;
        let url = server.url();
        let thumbnail_url = (server.has_capture()).then(|| server.thumbnail_url());
        Ok(Self {
            media_file_id,
            title_id,
            url,
            thumbnail_url,
            server,
        })
    }

    /// Where the renderer points its media element.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Where the renderer posts a frame it captured, or `None` when this session keeps no poster.
    ///
    /// Built by the core so the renderer never assembles one: the token is in both URLs, and a renderer
    /// that spliced one path segment into another would be a second place the route is spelled.
    #[must_use]
    pub fn thumbnail_url(&self) -> Option<&str> {
        self.thumbnail_url.as_deref()
    }

    /// Which of the title's files this session is serving.
    ///
    /// The id rather than the path, because it is what the watch screen's route is built from and what
    /// a later "remember where I stopped" write would key on (`PG-E16-01`'s `MediaSource`).
    #[must_use]
    pub const fn media_file_id(&self) -> i64 {
        self.media_file_id
    }

    /// The title this session belongs to.
    #[must_use]
    pub const fn title_id(&self) -> i64 {
        self.title_id
    }

    /// The token a request to [`PlaybackSession::url`] must carry.
    ///
    /// Exposed for the one caller that needs it — a frame capture posting its result back to the same
    /// listener — and for the tests that assert the URL and the token agree.
    #[must_use]
    pub const fn token(&self) -> &SessionToken {
        self.server.token()
    }

    /// Stops the listener and waits for it to close. Answers whether it stopped cleanly.
    pub async fn close(self) -> bool {
        self.server.shutdown().await
    }
}

#[expect(
    clippy::missing_fields_in_debug,
    reason = "the two fields it leaves out are a media URL and a capture URL, and both carry the \
              session token: a Debug line that printed them would be a bearer credential in a log \
              (SECURITY.md T13). What it prints is what a maintainer needs to correlate a report."
)]
impl std::fmt::Debug for PlaybackSession {
    /// Prints the two ids and the port, and never the path or the token: an id is what a maintainer
    /// correlates a bug report with, and the other two are values a log must not carry (SECURITY.md
    /// T13).
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PlaybackSession")
            .field("media_file_id", &self.media_file_id)
            .field("title_id", &self.title_id)
            .field("server", &self.server)
            .finish()
    }
}

#[cfg(test)]
mod tests;
