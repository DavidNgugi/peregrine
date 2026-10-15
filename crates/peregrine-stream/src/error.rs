//! Why a session could not be served.

use std::io;
use std::path::PathBuf;

use thiserror::Error;

/// What went wrong opening or running a media session's listener.
#[derive(Debug, Error)]
pub enum StreamError {
    /// The file a session was opened for is not there.
    ///
    /// A refusal at open rather than a `404` later, because by then nobody is reading the body: the
    /// user pressed Play, and "the file is gone" is a fact that belongs beside the control they
    /// pressed. `media_file.missing_since` is what a *scan* saw, not what is true now, so this check
    /// is the one that can tell the two apart (ADR 0023).
    #[error("the file a session was opened for is not on disk")]
    Missing {
        /// The path that was refused.
        path: PathBuf,
    },
    /// The path a session was opened for is not a file.
    ///
    /// A directory is not an empty file, and answering one as a `200` with no bytes would be a client
    /// told it had a zero-length video rather than a request refused for a reason.
    #[error("the path a session was opened for is not a file")]
    NotAFile {
        /// The path that was refused.
        path: PathBuf,
    },
    /// The file exists but could not be opened for reading.
    #[error("the file could not be opened for reading")]
    Open {
        /// The path that was refused.
        path: PathBuf,
        /// What the operating system said.
        #[source]
        source: io::Error,
    },
    /// A captured frame could not be stored.
    ///
    /// The sink is the caller's — this crate cannot name the image cache — so the message is the sink's
    /// own, and it is deliberately opaque here: what a reader needs is that the poster was not saved,
    /// and what a maintainer needs is in the log.
    #[error("the captured frame could not be stored")]
    Thumbnail {
        /// What the sink said.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The body a capture posted was larger than this build will read.
    #[error("that captured frame is larger than this build will store")]
    ThumbnailTooLarge,

    /// The loopback listener could not be bound.
    #[error("the loopback listener could not be bound")]
    Bind {
        /// What the operating system said.
        #[source]
        source: io::Error,
    },
    /// The listener stopped with an error rather than because it was shut down.
    #[error("the loopback listener stopped with an error")]
    Serve {
        /// What the operating system said.
        #[source]
        source: io::Error,
    },
}
