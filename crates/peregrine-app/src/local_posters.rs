//! Posters this machine generated from the film itself.
//!
//! **The fallback a library is most often actually in.** A fresh scan of a folder full of MP4s produces
//! titles no provider has described — PG-E5-06 calls them stubs — and a grid of grey rectangles is the
//! honest answer that looks like a broken one. The film is right there and its own decoder can show a
//! frame of it, so the tile can have a picture before any provider is configured at all.
//!
//! # Why this is a port rather than a call
//!
//! The renderer captures the frame, because decoding is the webview's job (ADR
//! [0023](../../../docs/adr/0023-in-app-video-surface.md)); it posts the JPEG to its own session's
//! listener; and `peregrine-stream` hands the bytes to a [`ThumbnailSink`] so that the crate serving
//! bytes never has to name the crate storing them. This module is the composition root's
//! implementation of that port, and the reason `peregrine-stream` has no dependency on the cache.
//!
//! # The two halves agree by construction
//!
//! [`LocalPosters::store`] writes the blob under [`ImageKey::for_local_title`] and [`cached`] looks for
//! the same key, so a poster cannot be written where nothing will look for it. Both go through one
//! function that builds the key, which is the same discipline `artwork::cached_image` follows for a
//! provider's blob.

use std::path::PathBuf;
use std::sync::Arc;

use peregrine_domain::Clock;
use peregrine_storage::{Database, ImageCacheEntry};
use peregrine_stream::{ThumbnailError, ThumbnailSink};
use peregrine_sync::{ImageCache, ImageKey, ImageVariant};

/// The slot a locally generated poster occupies in the image cache's provider column.
///
/// Not a `peregrine_domain::Provider` and deliberately not made one: that enum is the closed set of
/// metadata services, and a "local" member would put something no service serves into a type whose whole
/// job is naming services. The column has no constraint on it (migration 0007), so the word lives here,
/// where the only writer that needs it is.
const LOCAL_PROVIDER: &str = "local";

/// Where the poster generated from one title's own file is, or `None` when there is none.
///
/// A filesystem `stat` rather than a row read, for the reason `artwork::cached_image` gives: a row whose
/// blob an eviction removed is a path the asset protocol refuses, which renders as a broken poster and
/// reports nothing.
#[must_use]
pub fn cached(cache: &ImageCache, title_id: i64) -> Option<PathBuf> {
    let key = ImageKey::for_local_title(title_id, ImageVariant::LocalPoster);
    cache.contains(&key).then(|| cache.disk_path(&key))
}

/// The image cache, with the accounting rows that make its cap aware of generated posters.
///
/// **Held rather than passed**, because the store happens on a capture route with no other context, and
/// because the clock has to be the injected one: a row's timestamps are what the eviction order is
/// computed from, and a wall clock read here would be the one place in the tree that disagrees with
/// `AGENTS R14`.
pub struct LocalPosters {
    cache: Arc<ImageCache>,
    database: Arc<Database>,
    clock: Arc<dyn Clock + Send + Sync>,
    /// The runtime, so the blocking store can write its row.
    ///
    /// Captured at construction because this type is built on the runtime and then used from a
    /// `spawn_blocking` thread, where `Handle::current` is not available. `block_on` is legal there and
    /// illegal on a worker thread, which is exactly the distinction this field relies on — and why
    /// [`ThumbnailSink::store`]'s contract says the caller must be off the runtime.
    runtime: tokio::runtime::Handle,
}

impl LocalPosters {
    /// Builds the sink over the cache, the database, and the injected clock.
    ///
    /// # Panics
    ///
    /// If called outside a Tokio runtime. That is a construction error rather than a runtime condition —
    /// the composition root is the only caller and it is async by definition — and a fallible signature
    /// would push an impossible error into `Runtime::start`.
    #[must_use]
    pub fn new(
        cache: Arc<ImageCache>,
        database: Arc<Database>,
        clock: Arc<dyn Clock + Send + Sync>,
    ) -> Self {
        Self {
            cache,
            database,
            clock,
            runtime: tokio::runtime::Handle::current(),
        }
    }
}

impl ThumbnailSink for LocalPosters {
    fn store(&self, title_id: i64, bytes: &[u8]) -> Result<PathBuf, ThumbnailError> {
        let key = ImageKey::for_local_title(title_id, ImageVariant::LocalPoster);
        // Interactive work — a file write and a `rename` — and it is on a blocking thread already, so this
        // is where the cache's synchronous API belongs.
        let stored = self
            .cache
            .store(&key, bytes)
            .map_err(|_| ThumbnailError::Write)?;
        let at = self.clock.now().as_millisecond();

        // **The row is written after the bytes are published**, which is the order `image_cache` documents
        // and the reason for it: a crash between the two loses a file rather than recording a path with
        // nothing behind it. The cost is a blob with no row, which is a leak the cache wipe reaps.
        let entry = ImageCacheEntry {
            key: key.as_str().to_owned(),
            provider: LOCAL_PROVIDER.to_owned(),
            // The title's id, which is the "path" the key was hashed from: the two must read as one pair
            // or the row is bookkeeping for a blob nothing can find.
            path: title_id.to_string(),
            variant: ImageVariant::LocalPoster.as_str().to_owned(),
            disk_path: stored.disk_path().to_string_lossy().into_owned(),
            bytes: i64::try_from(stored.bytes()).unwrap_or(i64::MAX),
            // Unknown rather than invented: the cache counts bytes and does not decode, so a width here
            // would be a claim about bytes this layer has not looked at.
            width: None,
            height: None,
            created_at: at,
            last_used_at: at,
            hits: 0,
            // Not pinned: a generated poster is one play away from being regenerated, so it is exactly the
            // kind of blob the cap should be able to take back (`PG-E6-10`).
            pinned: false,
        };
        self.runtime
            .block_on(self.database.record_image(entry))
            .map_err(|_| ThumbnailError::Record)?;
        Ok(stored.disk_path().to_path_buf())
    }
}

impl std::fmt::Debug for LocalPosters {
    /// Prints nothing about either half: the cache's root is a path and the database's is a secret-free
    /// but still private location, and neither belongs in a log line (SECURITY.md T13).
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LocalPosters")
    }
}
