//! The asset protocol: the one path by which image bytes reach the webview (PG-E2-08).
//!
//! AGENTS R11 keeps blobs off IPC. A poster, a backdrop, a still, and a thumbnail
//! generated from a local media file are all bytes; the bytes travel over the `asset:`
//! protocol and IPC carries only the reference. This module is the Rust half of that
//! contract and it decides one thing: which directories the protocol may read. The
//! TypeScript half is `apps/desktop/src/lib/media.ts`, the only module that names
//! `convertFileSrc`, so a `<img src>` in a feature is a typed call rather than a URL
//! assembled by hand.
//!
//! # What is exposed, and why
//!
//! Exactly one directory: `<OS cache directory>/images`, created on first launch and
//! allowed recursively. It holds the content-addressed variants the image cache writes —
//! poster, backdrop, still, and local thumbnail, the tier 3 cache of
//! `docs/ARCHITECTURE.md` §8 — and nothing else is added to the scope, at runtime or in
//! `tauri.conf.json`.
//!
//! Three properties follow, and each has a test below rather than a promise here:
//!
//! * **The library root is not exposed.** A downloaded media file cannot be read through
//!   this protocol, so a compromised or mistaken renderer cannot turn the asset protocol
//!   into a file-read primitive over the user's library.
//! * **The scope is deny-by-default.** `app.security.assetProtocol` declares an empty
//!   static scope and `enable: true`; the only allowance is the one [`install`] makes, so
//!   a directory becomes readable only because this function allowed it.
//! * **A path outside the cache fails rather than being read.** Tauri's asset handler
//!   asks the same scope before it opens anything, and the scope resolves symlinks first,
//!   so a link planted inside the cache does not become a way out of it.
//!
//! The cache is disposable by design (`docs/ARCHITECTURE.md` §8: "a cache wipe is always
//! safe"), which is why it belongs under the OS cache directory and not beside the
//! database: a user or an OS cleaner may delete it, and the next launch recreates it.

use std::path::{Path, PathBuf};

use tauri::{Manager as _, Runtime};

/// The subdirectory of the OS cache directory that holds every image byte this app serves.
const IMAGE_CACHE_DIRECTORY: &str = "images";

/// Why the asset protocol could not be scoped.
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    /// The OS cache directory could not be resolved.
    #[error("the image cache directory could not be resolved")]
    CacheDirectory(#[source] tauri::Error),
    /// The cache directory could not be created.
    #[error("the image cache directory could not be created: {0}")]
    Create(#[source] std::io::Error),
    /// The cache directory could not be resolved to a real path.
    #[error("the image cache directory could not be resolved to a real path: {0}")]
    Resolve(#[source] std::io::Error),
    /// Tauri's scope refused the directory.
    #[error("the asset protocol scope refused the image cache directory: {0}")]
    Scope(#[source] tauri::Error),
}

/// The directory the asset protocol serves and the image cache writes into, before it exists.
///
/// `<OS cache directory>/images`, the tier-3 cache of `docs/ARCHITECTURE.md` §11: a cache wipe is
/// always safe, so it belongs where an OS cleaner may take it rather than beside the database.
///
/// Visible to the shell because a failure here is not the same failure as a scope that refuses: the
/// shell picks a fallback directory under its own data folder, and the protocol has to be asked
/// about *that* one. Two answers to "where do cached images live" is the defect this module exists
/// to prevent.
///
/// # Errors
///
/// [`MediaError::CacheDirectory`] when the OS will not name a cache directory.
pub(crate) fn image_cache_root<R: Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<PathBuf, MediaError> {
    Ok(app
        .path()
        .app_cache_dir()
        .map_err(MediaError::CacheDirectory)?
        .join(IMAGE_CACHE_DIRECTORY))
}

/// Creates `directory`, resolves it, and allows it recursively on the asset scope.
///
/// The path is canonicalized before it is allowed because the scope canonicalizes the
/// path it is *asked* about: on a platform where the cache directory is reached through a
/// symlink — `/var` on macOS, for instance — allowing the textual path would allow a
/// pattern no resolved path ever matches. Returns the resolved directory, which is what the runtime
/// is told to write into.
///
/// Called once from the shell's `setup`, before the document loads. A failure is reported and not
/// fatal: the app is still usable, images simply never arrive, and the in-app surfaces that depend
/// on them show their placeholder rather than crashing the window.
///
/// # Errors
///
/// [`MediaError`] names which of the three steps failed.
pub(crate) fn install<R: Runtime>(
    app: &tauri::AppHandle<R>,
    directory: &Path,
) -> Result<PathBuf, MediaError> {
    std::fs::create_dir_all(directory).map_err(MediaError::Create)?;
    let resolved = std::fs::canonicalize(directory).map_err(MediaError::Resolve)?;
    app.asset_protocol_scope()
        .allow_directory(&resolved, true)
        .map_err(MediaError::Scope)?;
    // Never the path itself: the diagnostics tail is exported and carries the user name
    // (SECURITY.md T13).
    tracing::debug!("the asset protocol was scoped to the image cache");
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mock application, which is what makes the scope assertable without an event loop.
    fn mock() -> tauri::App<tauri::test::MockRuntime> {
        tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("a mock application")
    }

    fn write(directory: &Path, name: &str) -> PathBuf {
        let path = directory.join(name);
        std::fs::write(&path, b"bytes").expect("the fixture writes");
        path
    }

    /// The scope serves the cache and nothing else: not a sibling directory, not the
    /// library root a caller might mistake for it, and not a traversal back out.
    #[test]
    fn only_the_image_cache_directory_is_readable_through_the_asset_protocol() {
        let cache = tempfile::tempdir().expect("a temporary cache");
        let library = tempfile::tempdir().expect("a temporary library root");
        let poster = write(cache.path(), "poster.webp");
        let media = write(library.path(), "movie.mkv");

        let app = mock();
        let resolved = install(app.handle(), cache.path()).expect("the cache is allowed");

        let scope = app.asset_protocol_scope();
        assert!(
            scope.is_allowed(&poster),
            "a cached poster is served by the asset protocol"
        );
        assert!(
            !scope.is_allowed(&media),
            "the library root must never be readable through the asset protocol"
        );
        // The two temporary directories are siblings, so this path resolves to a file
        // that exists: the refusal is the scope's, not the filesystem's.
        let traversal = resolved
            .join("..")
            .join(
                library
                    .path()
                    .file_name()
                    .expect("a named temporary directory"),
            )
            .join("movie.mkv");
        assert!(
            !scope.is_allowed(&traversal),
            "a traversal out of the cache is refused once it resolves"
        );
    }

    /// Nothing is readable before [`install`] has run.
    ///
    /// This is the deny-by-default half: the mock application's scope is the one the
    /// shipped config produces, and it allows no path at all until the cache is allowed.
    #[test]
    fn a_path_is_refused_before_the_cache_is_allowed() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let file = write(directory.path(), "poster.webp");

        let app = mock();
        assert!(
            !app.asset_protocol_scope().is_allowed(&file),
            "an empty scope must serve nothing"
        );
    }

    /// A path outside the cache is refused whether or not it exists: containment is decided
    /// by the scope's patterns, so a deleted file is not a way to reach its parent.
    #[test]
    fn a_path_outside_the_cache_is_refused_whether_or_not_it_exists() {
        let cache = tempfile::tempdir().expect("a temporary cache");
        let library = tempfile::tempdir().expect("a temporary library root");

        let app = mock();
        install(app.handle(), cache.path()).expect("the cache is allowed");

        let missing = library.path().join("never-created.webp");
        assert!(!app.asset_protocol_scope().is_allowed(&missing));
    }

    /// The scope resolves symlinks, so a link inside the cache is not a door out of it.
    #[cfg(unix)]
    #[test]
    fn a_symlink_inside_the_cache_cannot_reach_outside_it() {
        let cache = tempfile::tempdir().expect("a temporary cache");
        let outside = tempfile::tempdir().expect("a temporary directory outside the cache");
        let secret = write(outside.path(), "secret.bin");
        let link = cache.path().join("link.webp");
        std::os::unix::fs::symlink(&secret, &link).expect("the symlink is created");

        let app = mock();
        install(app.handle(), cache.path()).expect("the cache is allowed");

        assert!(
            !app.asset_protocol_scope().is_allowed(&link),
            "the scope canonicalizes, so a symlink out of the cache is refused"
        );
    }

    /// The shipped configuration enables the protocol and names no directory.
    ///
    /// Asserted against the file on disk rather than against the generated `Context`,
    /// because `AssetProtocolConfig`'s `ToTokens` emits `..Default::default()` and so every
    /// generated context reports `enable: false` regardless of the file — the flag is
    /// consumed by the CLI and the Cargo feature, not by the runtime. What matters is the
    /// pair: enabled, so the protocol exists, and with no static scope, so the runtime
    /// allowance is the only one there is.
    #[test]
    fn the_shipped_config_enables_the_protocol_and_names_no_directory() {
        use tauri::utils::config::FsScope;

        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json"))
            .expect("the config is readable");
        let parsed: tauri::utils::config::Config =
            serde_json::from_str(&text).expect("the config parses");
        let asset = &parsed.app.security.asset_protocol;
        assert!(
            asset.enable,
            "the asset protocol is what PG-E2-08 registers"
        );
        let allowed: &[PathBuf] = match &asset.scope {
            FsScope::AllowedPaths(paths) => paths,
            FsScope::Scope { allow, .. } => allow,
        };
        assert!(
            allowed.is_empty(),
            "the scope is filled at runtime and must not ship a directory: {allowed:?}"
        );
    }
}
