//! Where a frame the renderer captured goes.
//!
//! **A port, because the crate that serves bytes may not name the crate that stores them.** The image
//! cache lives in `peregrine-sync` and its accounting rows in `peregrine-storage`, and neither is an
//! edge `peregrine-stream` has or should have (`AGENTS R6`, `arch-allowlist.toml`). So this crate
//! decides *that* a captured frame is stored and lets the composition root decide *where* — the same
//! shape `VolumeSpace` has in `peregrine-app`'s acquisition service.
//!
//! **Synchronous, and the caller blocks on it properly.** What an implementation does is write a file
//! and a row, which is blocking work; the route wraps the call in `spawn_blocking` rather than making
//! the trait async, because an `async fn` in a trait is not object-safe and this has to be a `dyn`.

use std::path::PathBuf;

/// Why a captured frame could not be stored.
#[derive(Debug, thiserror::Error)]
pub enum ThumbnailError {
    /// The bytes could not be written.
    #[error("the captured frame could not be written")]
    Write,
    /// The row that accounts for the blob could not be recorded.
    ///
    /// Distinct from [`Self::Write`] because the two leave different states behind: a blob with no row is
    /// a leak the cache wipe reaps, and a row with no blob is a row an eviction tolerates — so an
    /// implementation that can tell them apart should, and one that cannot answers this.
    #[error("the captured frame was written but not recorded")]
    Record,
    /// The frame was refused before anything was written.
    ///
    /// The one refusal an implementation owes: bytes that are not an image this build will serve must
    /// not reach the cache, because the cache's directory is exactly what the asset protocol is scoped
    /// to and a blob in it is a blob the webview can be told to render.
    #[error("the captured frame is not an image this build will store")]
    Refused,
}

/// Where a locally generated poster goes.
///
/// **A `store` call may block, and the route calls it on a blocking thread.** That is part of the
/// contract rather than an implementation detail: what an implementation does is write a file and a
/// database row, and a thread that is not the async runtime's is where that belongs (`AGENTS §5`). An
/// implementation that reaches for a runtime handle must therefore expect to be *off* the runtime — the
/// route's `spawn_blocking` is what guarantees it, and a caller that invoked this directly from an async
/// context would be breaking the contract rather than finding a bug in the sink.
pub trait ThumbnailSink: Send + Sync + 'static {
    /// Stores one title's poster frame and answers where it landed.
    ///
    /// # Errors
    ///
    /// [`ThumbnailError`] when the bytes are refused, cannot be written, or cannot be recorded.
    fn store(&self, title_id: i64, bytes: &[u8]) -> Result<PathBuf, ThumbnailError>;
}
