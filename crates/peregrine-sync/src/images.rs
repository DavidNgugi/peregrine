//! The image cache: content-addressed blobs for poster and backdrop bytes (PG-E6-09).
//!
//! `docs/ARCHITECTURE.md` §11 calls this the tier-3 cache — the bytes themselves, as opposed to
//! the `image_ref` rows that describe them — and §5 gives it to this crate with `peregrine-sync`'s
//! other scheduled work. The directory it lives in is the one the shell's asset protocol is
//! scoped to (PG-E2-08), and nothing else may write there.
//!
//! # The key is a hash of the *identity*, not of the bytes
//!
//! `docs/DATA_MODEL.md` fixes the key: `image_cache.key` is `sha256(provider|path|variant)`, and
//! [`ImageKey`] is exactly that, lowercase hex. The name "content-addressed" is loose here and
//! worth stating plainly: two providers serving byte-identical artwork get two blobs, and there is
//! no de-duplication by content. What the key buys is a filesystem-safe name derived from data
//! that never needs escaping, and a name that two processes fetching the same image agree on, so
//! the second one finds a hit instead of writing a competitor.
//!
//! That is also why the *provider's* variant list is not enumerated here. TMDB's sizes come from
//! `/3/configuration` at runtime, so a table of them in this crate would be a second copy of
//! something the provider owns; what this crate decides is which variants are worth keeping,
//! which is a cache policy and is `docs/research/data-sources.md` §"Recommended cache strategy"'s
//! answer: a `w342` poster and a `w780` backdrop, with everything else proxied.
//!
//! # A crash leaves no partial file
//!
//! Bytes are written to a sibling temporary file, flushed to the device, and then *renamed* onto
//! the final name. A rename within one directory is atomic, so an observer — including the app
//! after a power loss — sees either no file or the whole file, and a half-written blob can never
//! be served as a poster. What is deliberately not done is fsyncing the *directory*: without it a
//! crash can lose the rename, so the image is fetched again on the next attempt, which is a cost
//! this cache can afford and a partial file is not.
//!
//! The temporary name is the final one plus this process's id and a counter, which is unique
//! without a random source — `cargo xtask check-determinism` has nothing to object to, and two
//! writers in one process cannot collide.

use std::fmt;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use peregrine_domain::catalogue::ImageKind;
use peregrine_domain::id::Provider;
use sha2::{Digest as _, Sha256};
use thiserror::Error;

/// The suffix a partly written blob has, which is what keeps it out of every lookup.
const PARTIAL_SUFFIX: &str = "partial";

/// Which size of an image the cache keeps.
///
/// The three provider variants are the ones `docs/research/data-sources.md` recommends — a `w342`
/// poster is about 63 KB and a `w780` backdrop about 291 KB — and every other size the provider offers
/// is proxied through rather than stored. Every variant kept is a variant evicted later, and the byte
/// cap is a user setting (PG-E6-10).
///
/// **[`Self::LocalPoster`] is not a provider variant at all**, which is why it is last and why
/// `for_kind` does not answer it: it holds a frame this machine generated from a file the library
/// holds, for a title no provider has drawn a poster for. `docs/ARCHITECTURE.md` §17 already promised
/// the cache would hold "local-thumbnail variants"; this is that promise, and it is a variant rather
/// than a second cache because the accounting, the eviction order, and the asset-protocol scope are
/// the same for both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ImageVariant {
    /// A grid poster.
    PosterW342,
    /// A detail-page backdrop.
    BackdropW780,
    /// One episode's own wide image.
    StillW300,
    /// A streaming service's logo.
    LogoW154,
    /// A poster this machine generated from a frame of the title's own file.
    ///
    /// Sized by whatever the capture produced rather than by a provider's ladder, so there is no width
    /// in the name: the name is part of the cache key and nothing more, and a "size" here would be a
    /// claim about bytes a renderer chose.
    LocalPoster,
}

impl ImageVariant {
    /// Every variant the cache keeps.
    pub const ALL: [Self; 5] = [
        Self::PosterW342,
        Self::BackdropW780,
        Self::StillW300,
        Self::LogoW154,
        Self::LocalPoster,
    ];

    /// The provider's own name for the size, which is part of the cache key.
    ///
    /// These are TMDB's, because TMDB is the provider this cache was designed against; another
    /// provider's equivalent sizes arrive with its adapter and get their own variants rather than
    /// being coerced into these.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PosterW342 => "w342",
            Self::BackdropW780 => "w780",
            Self::StillW300 => "w300",
            Self::LogoW154 => "w154",
            Self::LocalPoster => "local",
        }
    }

    /// The variant the cache keeps for one kind of picture.
    ///
    /// The fetch path (`cache_image`) and the read path that answers a screen with a path already
    /// on disk both need this, and a second copy would be a cache that writes one blob and looks
    /// for another.
    #[must_use]
    pub const fn for_kind(kind: ImageKind) -> Self {
        match kind {
            ImageKind::Poster => Self::PosterW342,
            ImageKind::Backdrop => Self::BackdropW780,
            ImageKind::Still => Self::StillW300,
            ImageKind::Logo => Self::LogoW154,
        }
    }
}

/// The key one image is stored under: `sha256(provider|path|variant)`, lowercase hex.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ImageKey(String);

impl ImageKey {
    /// The key a locally generated poster for one title is stored under.
    ///
    /// **`local` in the provider slot, and the title's id in the path slot**, which is what keeps the
    /// key's shape one shape: `sha256(provider|path|variant)` is what `image_cache`'s three columns
    /// hold, and a second shape would be a second set of columns to keep in step. `local` is not a
    /// [`Provider`] — that enum is closed over the metadata services — but the column has no
    /// constraint on it, and inventing a provider variant for something no provider serves would be
    /// the worse lie.
    ///
    /// **Keyed by the title rather than by the file**, and that is the load-bearing choice: the reads
    /// that draw a poster answer for a *title*, and a key derived from a media file would make every
    /// one of them read the title's items and files first — a second query per row on a screen that
    /// draws a hundred rows. A title can hold several files; it has one poster.
    #[must_use]
    pub fn for_local_title(title_id: i64, variant: ImageVariant) -> Self {
        Self::digest("local", &title_id.to_string(), variant)
    }

    /// The key for one image of one provider at one variant.
    #[must_use]
    pub fn for_image(provider: Provider, path: &str, variant: ImageVariant) -> Self {
        Self::digest(provider.as_str(), path, variant)
    }

    /// `sha256(owner|path|variant)`, lowercase hex — the one place the key is spelled.
    ///
    /// A shared private function rather than a body in `for_image`, because the two constructors must
    /// agree about the separators and the encoding: a second copy is a second thing to change, and the
    /// failure it would produce is a blob written under one name and looked for under another, which
    /// renders as a poster that never arrives.
    fn digest(owner: &str, path: &str, variant: ImageVariant) -> Self {
        let mut hasher = Sha256::new();
        // The separators are what make the three fields unambiguous: without them, owner `ab` with
        // path `c` and owner `a` with path `bc` would hash the same bytes.
        hasher.update(owner.as_bytes());
        hasher.update(b"|");
        hasher.update(path.as_bytes());
        hasher.update(b"|");
        hasher.update(variant.as_str().as_bytes());

        let digest: [u8; 32] = hasher.finalize().into();
        let mut hex = String::with_capacity(64);
        for byte in digest {
            write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
        }
        Self(hex)
    }

    /// The key as the database and the filesystem spell it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ImageKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One cached blob, as the store reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedImage {
    key: ImageKey,
    disk_path: PathBuf,
    bytes: u64,
}

impl CachedImage {
    /// The key it is stored under.
    #[must_use]
    pub const fn key(&self) -> &ImageKey {
        &self.key
    }

    /// Where the bytes are, which is what the `image_cache.disk_path` column records.
    #[must_use]
    pub fn disk_path(&self) -> &Path {
        &self.disk_path
    }

    /// How many bytes the stored blob holds.
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }
}

/// Why the store could not do what it was asked.
#[derive(Debug, Error)]
pub enum ImageCacheError {
    /// The cache directory could not be created.
    #[error("the image cache directory {path} could not be created")]
    CreateDirectory {
        /// The directory.
        path: PathBuf,
        /// What the filesystem said.
        #[source]
        source: std::io::Error,
    },

    /// The blob could not be written.
    #[error("the image cache could not write {path}")]
    Write {
        /// The file being written.
        path: PathBuf,
        /// What the filesystem said.
        #[source]
        source: std::io::Error,
    },

    /// The blob could not be published under its final name.
    #[error("the image cache could not publish {path}")]
    Publish {
        /// The name it was being published as.
        path: PathBuf,
        /// What the filesystem said.
        #[source]
        source: std::io::Error,
    },
}

/// The content-addressed blob store.
#[derive(Debug, Clone)]
pub struct ImageCache {
    root: PathBuf,
    /// Distinguishes two writers in one process without a random source.
    writers: std::sync::Arc<AtomicU64>,
}

impl ImageCache {
    /// A store rooted at `root`, which is the directory the asset protocol is scoped to.
    ///
    /// The directory is not created here: a cache that creates directories when it is
    /// constructed is a cache that does filesystem work on a path that may never be used.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            writers: std::sync::Arc::new(AtomicU64::new(0)),
        }
    }

    /// The directory the blobs live in.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where the blob for `key` lives, whether or not it is there yet.
    ///
    /// One name, flat, and the same name every time — a key is a hash, so it needs no escaping and
    /// no sharding to be a valid file name. The layout is not load-bearing: the row records
    /// `disk_path`, so a future layout finds what the old one wrote.
    #[must_use]
    pub fn disk_path(&self, key: &ImageKey) -> PathBuf {
        self.root.join(key.as_str())
    }

    /// Whether a whole blob for `key` is already stored.
    ///
    /// A partly written blob has a different name and is therefore never a hit, which is the
    /// property the whole write dance exists for: this is what stops a crash mid-download from
    /// becoming a broken poster on the next launch.
    #[must_use]
    pub fn contains(&self, key: &ImageKey) -> bool {
        self.disk_path(key).is_file()
    }

    /// Writes `bytes` under `key`, atomically, and reports where they landed.
    ///
    /// Storing the same key twice is not an error and does not rewrite: the bytes are immutable
    /// per their provider, so an existing blob is the answer, and a second writer that raced this
    /// one finds it rather than replacing it.
    ///
    /// The bytes are flushed before the rename rather than after, so the name never refers to
    /// data that is still in a buffer.
    ///
    /// # Errors
    ///
    /// [`ImageCacheError`] when the directory cannot be created, the blob cannot be written, or
    /// the rename fails. A failed write removes its own temporary file, so a full disk leaves no
    /// debris behind.
    pub fn store(&self, key: &ImageKey, bytes: &[u8]) -> Result<CachedImage, ImageCacheError> {
        let disk_path = self.disk_path(key);
        if let Ok(metadata) = fs::metadata(&disk_path)
            && metadata.is_file()
        {
            return Ok(CachedImage {
                key: key.clone(),
                disk_path,
                bytes: metadata.len(),
            });
        }

        fs::create_dir_all(&self.root).map_err(|source| ImageCacheError::CreateDirectory {
            path: self.root.clone(),
            source,
        })?;

        let temporary = self.temporary_path(key);
        if let Err(source) = write_and_flush(&temporary, bytes) {
            // Best effort: the error that matters is the one being returned, and a cache that
            // fails to clean up is a full disk, which the next write will report anyway.
            let _ = fs::remove_file(&temporary);
            return Err(ImageCacheError::Write {
                path: temporary,
                source,
            });
        }

        if let Err(source) = fs::rename(&temporary, &disk_path) {
            let _ = fs::remove_file(&temporary);
            return Err(ImageCacheError::Publish {
                path: disk_path,
                source,
            });
        }

        Ok(CachedImage {
            key: key.clone(),
            disk_path,
            bytes: bytes.len() as u64,
        })
    }

    /// The name a partly written blob has while it is being written.
    fn temporary_path(&self, key: &ImageKey) -> PathBuf {
        let writer = self.writers.fetch_add(1, Ordering::Relaxed);
        self.root.join(format!(
            "{}.{}.{writer}.{PARTIAL_SUFFIX}",
            key.as_str(),
            std::process::id()
        ))
    }
}

/// Writes `bytes` to `path` and flushes them to the device.
fn write_and_flush(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;

    use super::*;

    /// A directory of its own per test, removed when the guard drops.
    struct Scratch {
        root: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("peregrine-images-{name}"));
            let _ = fs::remove_dir_all(&root);
            Self { root }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn key() -> ImageKey {
        ImageKey::for_image(Provider::Tmdb, "/poster.jpg", ImageVariant::PosterW342)
    }

    #[test]
    fn a_key_is_the_sha256_of_the_three_fields_joined() {
        // Verified against `printf 'tmdb|abc|w342' | shasum -a 256` rather than remembered.
        let computed = ImageKey::for_image(Provider::Tmdb, "abc", ImageVariant::PosterW342);

        assert_eq!(
            computed.as_str(),
            "15416215ddfd1b675bc207ec45af7bbfee9fa205b4698c113e4d5ee7b332a289"
        );
    }

    #[test]
    fn the_digest_is_sha256_and_not_something_digest_shaped() {
        // The empty-input vector, which is the one case where a length-prefixed or differently
        // initialised hash would agree with SHA-256 on structure and disagree on value.
        let mut hasher = Sha256::new();
        hasher.update(b"");
        let digest: [u8; 32] = hasher.finalize().into();
        let mut hex = String::new();
        for byte in digest {
            write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
        }

        assert_eq!(
            hex,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn a_key_is_sixty_four_lowercase_hex_characters() {
        // It is a file name and a primary key, so anything else would be a bug in the rendering
        // rather than in the hash.
        for variant in ImageVariant::ALL {
            let key = ImageKey::for_image(Provider::Tmdb, "/a/b/c.jpg", variant);
            assert_eq!(key.as_str().len(), 64, "{key}");
            assert!(
                key.as_str()
                    .chars()
                    .all(|character| character.is_ascii_digit() || ('a'..='f').contains(&character)),
                "{key} is not lowercase hex"
            );
        }
    }

    #[test]
    fn every_field_changes_the_key() {
        // Any two images that differ in one field are different blobs, and the separators are
        // what stop a provider and a path from being re-split into a colliding pair.
        let base = ImageKey::for_image(Provider::Tmdb, "path", ImageVariant::PosterW342);

        assert_ne!(
            base,
            ImageKey::for_image(Provider::Tvdb, "path", ImageVariant::PosterW342)
        );
        assert_ne!(
            base,
            ImageKey::for_image(Provider::Tmdb, "other", ImageVariant::PosterW342)
        );
        assert_ne!(
            base,
            ImageKey::for_image(Provider::Tmdb, "path", ImageVariant::BackdropW780)
        );
        // The separator itself: `ab|c` and `a|bc` must not hash alike.
        assert_ne!(
            ImageKey::for_image(Provider::Tmdb, "c", ImageVariant::PosterW342),
            ImageKey::for_image(Provider::Tmdb, "bc", ImageVariant::PosterW342)
        );
    }

    #[test]
    fn the_same_image_always_has_the_same_key() {
        // Two processes fetching one image have to agree, or the second writes a competitor.
        let first = ImageKey::for_image(Provider::Tmdb, "/poster.jpg", ImageVariant::PosterW342);
        let second = ImageKey::for_image(Provider::Tmdb, "/poster.jpg", ImageVariant::PosterW342);

        assert_eq!(first, second);
    }

    #[test]
    fn the_kept_variants_are_the_five_the_screens_ask_for() {
        let named: Vec<&str> = ImageVariant::ALL.iter().map(|one| one.as_str()).collect();

        assert_eq!(named, ["w342", "w780", "w300", "w154", "local"]);
    }

    #[test]
    fn a_local_poster_is_keyed_by_its_title_and_can_never_collide_with_a_provider_s() {
        // Three properties, and each of them is a defect if it fails. Two titles must not share one
        // file. One title's local poster must not be the same blob as another title's. And a local key
        // must never equal a provider key, or a generated frame could shadow the artwork a provider was
        // about to deliver — which would leave a screen showing a film still where a poster belongs and
        // no request ever made to replace it.
        let seven = ImageKey::for_local_title(7, ImageVariant::LocalPoster);
        let eight = ImageKey::for_local_title(8, ImageVariant::LocalPoster);

        assert_ne!(seven, eight);
        assert_eq!(
            seven,
            ImageKey::for_local_title(7, ImageVariant::LocalPoster),
            "the same title always has the same key, or the second capture writes a competitor"
        );
        assert_ne!(
            seven,
            ImageKey::for_image(Provider::Tmdb, "7", ImageVariant::PosterW342),
            "the provider slot is what keeps the two apart"
        );
        assert_ne!(
            seven,
            ImageKey::for_local_title(7, ImageVariant::PosterW342),
            "and the variant is what keeps a local poster from answering a backdrop lookup"
        );
        assert_eq!(seven.as_str().len(), 64, "still a hex digest: {seven}");
    }

    #[test]
    fn each_kind_of_picture_has_exactly_one_variant() {
        // The mapping both paths share: the fetch writes the blob this names, and the read that
        // answers a screen with a path looks for the same one. Every kind is asserted, so a kind
        // added to the domain without a variant here fails rather than silently fetching a poster
        // size for a backdrop.
        assert_eq!(
            ImageVariant::for_kind(ImageKind::Poster),
            ImageVariant::PosterW342
        );
        assert_eq!(
            ImageVariant::for_kind(ImageKind::Backdrop),
            ImageVariant::BackdropW780
        );
        assert_eq!(
            ImageVariant::for_kind(ImageKind::Still),
            ImageVariant::StillW300
        );
        assert_eq!(
            ImageVariant::for_kind(ImageKind::Logo),
            ImageVariant::LogoW154
        );
        // **Every *provider* variant is some kind's**, because a variant nothing maps to is a size fetched
        // by nothing. The one exception is asserted rather than skipped: `LocalPoster` is deliberately no
        // kind's variant — it holds a frame this machine generated, not a size a provider serves, and a
        // `for_kind` that answered it would make a poster lookup read a capture.
        for variant in ImageVariant::ALL {
            let named = [
                ImageKind::Poster,
                ImageKind::Backdrop,
                ImageKind::Still,
                ImageKind::Logo,
            ]
            .into_iter()
            .any(|kind| ImageVariant::for_kind(kind) == variant);
            if variant == ImageVariant::LocalPoster {
                assert!(
                    !named,
                    "a local poster is not a kind's variant, and must not be"
                );
            } else {
                assert!(named, "{variant:?} is no kind's variant");
            }
        }
    }

    #[test]
    fn storing_bytes_makes_them_readable_at_the_path_the_key_names() {
        let scratch = Scratch::new("store");
        let cache = ImageCache::new(&scratch.root);
        let key = key();
        let bytes = b"pretend this is a jpeg";

        let stored = cache
            .store(&key, bytes)
            .expect("the scratch directory is writable");

        assert_eq!(stored.key(), &key);
        assert_eq!(stored.disk_path(), cache.disk_path(&key));
        assert_eq!(stored.bytes(), bytes.len() as u64);
        assert!(cache.contains(&key));
        assert_eq!(
            fs::read(cache.disk_path(&key)).expect("it was just written"),
            bytes
        );
    }

    #[test]
    fn a_key_that_was_never_stored_is_absent() {
        let scratch = Scratch::new("absent");
        let cache = ImageCache::new(&scratch.root);

        assert!(!cache.contains(&key()));
        assert!(!cache.disk_path(&key()).exists());
    }

    #[test]
    fn storing_the_same_key_twice_keeps_one_blob_and_the_first_bytes() {
        // The bytes are immutable per their provider, so a second write is a wasted one — and a
        // competitor's half-written file must not replace a whole one.
        let scratch = Scratch::new("twice");
        let cache = ImageCache::new(&scratch.root);
        let key = key();

        cache.store(&key, b"first").expect("writable");
        let second = cache.store(&key, b"second").expect("writable");

        assert_eq!(
            second.bytes(),
            5,
            "the stored blob is the one that was there"
        );
        assert_eq!(fs::read(cache.disk_path(&key)).expect("readable"), b"first");
        assert_eq!(cache.partials(), Vec::<String>::new());
    }

    #[test]
    fn a_partly_written_blob_is_never_a_hit() {
        // The crash case, as far as a test can stage it: the file a killed download leaves behind
        // has the temporary name, so the cache reports the image as absent and the next run
        // fetches it again rather than serving a truncated poster.
        let scratch = Scratch::new("partial");
        let cache = ImageCache::new(&scratch.root);
        fs::create_dir_all(&scratch.root).expect("writable");
        let key = key();
        let partial = scratch
            .root
            .join(format!("{}.999.0.{PARTIAL_SUFFIX}", key.as_str()));
        fs::write(&partial, b"half a jpeg").expect("writable");

        assert!(!cache.contains(&key), "a partial blob is not an image");
        assert_eq!(fs::read(&partial).expect("still there"), b"half a jpeg");
    }

    #[test]
    fn a_completed_store_leaves_no_temporary_file_behind() {
        let scratch = Scratch::new("clean");
        let cache = ImageCache::new(&scratch.root);

        cache.store(&key(), b"whole").expect("writable");

        assert_eq!(cache.partials(), Vec::<String>::new());
    }

    #[test]
    fn a_hostile_path_cannot_escape_the_cache_directory() {
        // A provider path is untrusted input (ARCHITECTURE §9). It cannot escape here, and the
        // reason is structural rather than a check that could be forgotten: the file name is the
        // hash of the path, so no part of the path reaches the filesystem.
        let scratch = Scratch::new("hostile");
        let cache = ImageCache::new(&scratch.root);
        let key = ImageKey::for_image(
            Provider::Tmdb,
            "../../../../etc/passwd",
            ImageVariant::PosterW342,
        );

        let stored = cache.store(&key, b"not a password file").expect("writable");

        assert!(stored.disk_path().starts_with(&scratch.root));
        assert_eq!(
            stored
                .disk_path()
                .file_name()
                .and_then(|name| name.to_str()),
            Some(key.as_str())
        );
    }

    #[test]
    fn the_cache_directory_is_created_on_the_first_store_and_not_before() {
        let scratch = Scratch::new("lazy");
        let cache = ImageCache::new(scratch.root.join("images"));

        assert!(!cache.root().exists(), "constructing a cache does no IO");
        cache.store(&key(), b"bytes").expect("writable");
        assert!(cache.root().is_dir());
    }

    #[test]
    fn an_empty_blob_is_stored_and_reported_as_empty() {
        // Whether zero bytes are a valid image is the caller's question, not the store's: this
        // asserts only that the store does not confuse "empty" with "absent".
        let scratch = Scratch::new("empty");
        let cache = ImageCache::new(&scratch.root);

        let stored = cache.store(&key(), b"").expect("writable");

        assert_eq!(stored.bytes(), 0);
        assert!(cache.contains(&key()), "an empty blob is a blob");
    }

    #[test]
    fn distinct_images_land_in_distinct_files() {
        let scratch = Scratch::new("distinct");
        let cache = ImageCache::new(&scratch.root);
        let keys: BTreeSet<ImageKey> = [
            (Provider::Tmdb, "/a.jpg", ImageVariant::PosterW342),
            (Provider::Tmdb, "/a.jpg", ImageVariant::BackdropW780),
            (Provider::Tmdb, "/b.jpg", ImageVariant::PosterW342),
            (Provider::Tvdb, "/a.jpg", ImageVariant::PosterW342),
        ]
        .into_iter()
        .map(|(provider, path, variant)| ImageKey::for_image(provider, path, variant))
        .collect();

        assert_eq!(keys.len(), 4, "four images, four keys");
        for key in &keys {
            cache.store(key, key.as_str().as_bytes()).expect("writable");
        }
        assert_eq!(cache.blobs().len(), 4, "one file per key, none overwritten");
    }

    #[test]
    fn two_caches_over_one_directory_agree() {
        // The asset protocol reads what the fetch wrote, and they are different objects.
        let scratch = Scratch::new("agree");
        let writer = ImageCache::new(&scratch.root);
        let reader = ImageCache::new(&scratch.root);
        let key = key();

        writer.store(&key, b"bytes").expect("writable");

        assert!(reader.contains(&key));
        assert_eq!(reader.disk_path(&key), writer.disk_path(&key));
    }

    /// Test-only: the blobs in the cache directory, sorted.
    impl ImageCache {
        fn entries(&self) -> Vec<String> {
            let Ok(directory) = fs::read_dir(&self.root) else {
                return Vec::new();
            };
            let mut names: Vec<String> = directory
                .filter_map(Result::ok)
                .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
                .collect();
            names.sort();
            names
        }

        fn partials(&self) -> Vec<String> {
            self.entries()
                .into_iter()
                .filter(|name| name.ends_with(PARTIAL_SUFFIX))
                .collect()
        }

        fn blobs(&self) -> Vec<String> {
            self.entries()
                .into_iter()
                .filter(|name| !name.ends_with(PARTIAL_SUFFIX))
                .collect()
        }
    }
}
