//! What a screen needs to know about a title's artwork (PG-E2-08, PG-E6-09, PG-E6-15).
//!
//! Two answers, and they are the same two questions every screen with a picture on it has to ask:
//! *where is the blob*, and *is asking a provider for it worth a request*. Both belong here rather
//! than in a component, because both are computed from things only the core holds — the cache key is
//! a hash the webview cannot compute, and whether a fill has already happened is a column.
//!
//! # The path is the cache's, not the row's
//!
//! [`cached_image`] decides by asking the filesystem, not by reading the `image_cache` row. A row
//! without a file is a row an eviction tolerates (`PG-E6-09`), and a path whose blob is gone is a
//! path the webview would hand to the asset protocol and be refused — an error in a devtools console
//! that looks exactly like a rendering bug. A row is a record of what was written; the file is what
//! can be served.

use std::path::{Path, PathBuf};

use peregrine_domain::catalogue::ImageKind;
use peregrine_domain::id::Provider;
use peregrine_sync::{ImageCache, ImageKey, ImageVariant};

/// Where the cache holds one provider's image, or `None` when it does not.
///
/// The key is the one the fetch wrote — both paths go through [`ImageVariant::for_kind`] — so a
/// single mapping decides which size is fetched and which size is looked for.
#[must_use]
pub fn cached_image(
    cache: &ImageCache,
    provider: Provider,
    path: &str,
    kind: ImageKind,
) -> Option<PathBuf> {
    let key = ImageKey::for_image(provider, path, ImageVariant::for_kind(kind));
    cache.contains(&key).then(|| cache.disk_path(&key))
}

/// What the catalogue and the cache hold for one title's primary poster.
///
/// Three states rather than four flags because the fourth combination does not exist: the cache
/// cannot hold a blob whose path the catalogue does not record. Saying so in the type is what stops
/// a reader wondering which of the two a poster-less title is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Artwork {
    /// The catalogue records no poster path: no provider has ever given this title one.
    Unknown,
    /// The catalogue records a path, and the cache does not hold the blob it names.
    ///
    /// Either the bytes were never fetched or an eviction took them; both are one request away from
    /// being a picture.
    Missing,
    /// The catalogue records a path and the cache holds the blob.
    Cached,
}

/// What the catalogue, the cache, and this installation's providers know between them about one
/// title's artwork.
///
/// A value rather than three arguments, because the rule reads them together: booleans in a row at a
/// call site say nothing about which is which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtworkState {
    /// Whether a configured provider owns an identifier this title carries.
    ///
    /// **The input a screen could not possibly compute**, and the one that keeps the rest of the rule
    /// honest: it needs the credential store's answer and the title's external ids together. A title
    /// no configured provider has an id for cannot be filled in by any of them, so asking would spend
    /// a job and a journal row per visit and fetch nothing.
    pub answerable: bool,
    /// Whether any provider has ever filled this title in.
    pub synced: bool,
    /// What the catalogue and the cache hold.
    pub artwork: Artwork,
}

impl ArtworkState {
    /// The state a title read hands the rule: the row the catalogue returned, the blob the cache
    /// answered with, and whether a provider can be asked at all.
    ///
    /// One place decides which column feeds which input, and that is the part a test of the rule
    /// itself cannot see: a poster taken from the wrong source is a screen that asks at the wrong
    /// moments for the life of the feature, and no gate reports it.
    #[must_use]
    pub fn of(
        row: &peregrine_storage::TitleDetailRow,
        poster: Option<&Path>,
        answerable: bool,
    ) -> Self {
        let artwork = match (row.poster.is_some(), poster.is_some()) {
            (false, _) => Artwork::Unknown,
            (true, false) => Artwork::Missing,
            (true, true) => Artwork::Cached,
        };
        Self {
            answerable,
            synced: row.synced,
            artwork,
        }
    }
}

/// Whether any of `configured` owns one of the identifiers `held`.
///
/// The first fact [`refresh_due`] needs, on its own so it can be tested without a runtime, a
/// credential store, or a database: a title nobody can be asked about is a title a screen must not
/// start a job for.
#[must_use]
pub fn can_answer(configured: &[Provider], held: &[peregrine_domain::ExternalId]) -> bool {
    held.iter().any(|id| configured.contains(&id.provider()))
}

/// Whether opening a title's screen should ask its provider for it.
///
/// **A rule, and the reason it is here rather than in a screen** (AGENTS R7). Three facts decide it:
///
/// * **There is something to ask with.** No configured provider owns an id for this title, so none
///   of them can answer about it.
/// * **The title has never been filled in.** Then a provider may hold everything the catalogue is
///   missing, so the screen asks once.
/// * **A provider is known to have artwork the cache does not hold.** Then the blob was evicted, or
///   the fill recorded the path and the bytes never arrived, and one request puts it back.
///
/// What the three together *exclude* is the case that would otherwise ask forever: a title that has
/// been filled in, whose provider had no poster. Asking again would spend a request to be told the
/// same thing on every visit, which is the shape `AGENTS R24` exists to prevent.
///
/// **This is not the staleness policy.** When a *filled* title is due again — the six-month window,
/// the state machine for `MetadataState::Stale` — is `PG-E6-15`'s, and nothing here has an opinion
/// about it: a title whose metadata is old is a title this function says not to ask about.
///
/// **This decides one ask, not how often one may happen.** The caller owns the visit: it asks once
/// and remembers that it did, so a refresh that produced nothing cannot become a request loop.
#[must_use]
pub const fn refresh_due(state: &ArtworkState) -> bool {
    state.answerable && (!state.synced || matches!(state.artwork, Artwork::Missing))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// A cache of its own per test, removed when the guard drops.
    struct Scratch {
        root: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("peregrine-artwork-{name}"));
            let _ = fs::remove_dir_all(&root);
            Self { root }
        }

        fn cache(&self) -> ImageCache {
            ImageCache::new(&self.root)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn a_blob_the_cache_holds_is_answered_with_its_path() {
        let scratch = Scratch::new("holds");
        let cache = scratch.cache();
        let key = ImageKey::for_image(Provider::Tmdb, "/poster.jpg", ImageVariant::PosterW342);
        cache
            .store(&key, b"bytes")
            .expect("the scratch is writable");

        assert_eq!(
            cached_image(&cache, Provider::Tmdb, "/poster.jpg", ImageKind::Poster),
            Some(cache.disk_path(&key))
        );
    }

    #[test]
    fn a_kind_the_cache_never_stored_is_absent() {
        // The poster of a title whose backdrop was fetched is still absent, and vice versa: the
        // variant is part of the key, so a lookup cannot answer with the other picture's bytes.
        let scratch = Scratch::new("other-kind");
        let cache = scratch.cache();
        cache
            .store(
                &ImageKey::for_image(Provider::Tmdb, "/poster.jpg", ImageVariant::BackdropW780),
                b"bytes",
            )
            .expect("the scratch is writable");

        assert_eq!(
            cached_image(&cache, Provider::Tmdb, "/poster.jpg", ImageKind::Poster),
            None
        );
    }

    #[test]
    fn another_providers_path_is_absent() {
        // Two providers can spell the same path; the key carries the provider, so one's blob is not
        // served for the other's title.
        let scratch = Scratch::new("other-provider");
        let cache = scratch.cache();
        cache
            .store(
                &ImageKey::for_image(Provider::Tmdb, "/poster.jpg", ImageVariant::PosterW342),
                b"bytes",
            )
            .expect("the scratch is writable");

        assert_eq!(
            cached_image(&cache, Provider::Tvdb, "/poster.jpg", ImageKind::Poster),
            None
        );
    }

    #[test]
    fn a_row_without_a_file_is_not_a_hit_this_lookup_can_see() {
        // The store's own rule, asserted from this side because it is what makes the answer safe:
        // a partly written blob has a different name, so it is never answered as the cache's.
        let scratch = Scratch::new("partial");
        let cache = scratch.cache();
        fs::create_dir_all(&scratch.root).expect("the scratch is creatable");
        let key = ImageKey::for_image(Provider::Tmdb, "/poster.jpg", ImageVariant::PosterW342);
        fs::write(
            scratch.root.join(format!("{}.0000.partial", key.as_str())),
            b"half",
        )
        .expect("the leftover writes");

        assert_eq!(
            cached_image(&cache, Provider::Tmdb, "/poster.jpg", ImageKind::Poster),
            None,
            "a killed download must not be served as a poster"
        );
    }

    /// The three facts, in the order the rule reads them.
    fn state(answerable: bool, synced: bool, artwork: Artwork) -> ArtworkState {
        ArtworkState {
            answerable,
            synced,
            artwork,
        }
    }

    #[test]
    fn a_title_nobody_has_filled_in_is_asked_about() {
        // The first visit to a title the library just scanned: no fill has happened, so a provider
        // may hold everything the catalogue is missing.
        assert!(refresh_due(&state(true, false, Artwork::Unknown)));
        // Even when the catalogue somehow records a poster: a fill that never completed still left
        // the row saying a provider has never answered.
        assert!(refresh_due(&state(true, false, Artwork::Cached)));
    }

    #[test]
    fn a_filled_title_whose_provider_had_no_artwork_is_never_asked_again() {
        // The case that would otherwise spend a request on every visit, forever: the provider was
        // asked, it had no poster, and asking again cannot change that (AGENTS R24).
        assert!(!refresh_due(&state(true, true, Artwork::Unknown)));
    }

    #[test]
    fn an_evicted_poster_is_fetched_again_and_a_cached_one_is_not() {
        assert!(
            refresh_due(&state(true, true, Artwork::Missing)),
            "the catalogue knows of artwork the cache does not hold"
        );
        assert!(
            !refresh_due(&state(true, true, Artwork::Cached)),
            "a poster on disk is nothing to ask for"
        );
    }

    /// One title row, with only the two fields the state is built from varying.
    fn row(synced: bool, poster: Option<&str>) -> peregrine_storage::TitleDetailRow {
        peregrine_storage::TitleDetailRow {
            library: peregrine_storage::LibraryTitleRow {
                title: peregrine_storage::TitleIdentity {
                    id: 7,
                    kind: peregrine_storage::TitleKind::Movie,
                    subtype: peregrine_storage::TitleSubtype::Standard,
                    name: "Alien".to_owned(),
                    sort_name: "alien".to_owned(),
                    metadata_state: peregrine_storage::MetadataState::Stub,
                    adult: false,
                },
                items: 0,
                files: 0,
                bytes: 0,
                missing_files: 0,
            },
            poster: poster.map(|path| peregrine_storage::ArtworkPath {
                provider: Provider::Tmdb,
                path: path.to_owned(),
            }),
            synced,
        }
    }

    #[test]
    fn the_state_a_title_read_hands_the_rule_comes_from_the_row_and_the_cache() {
        // The wiring, not the rule: these are the two inputs a swap would hide, and the failure it
        // would produce is a screen that asks at the wrong moments rather than an error.
        assert_eq!(
            ArtworkState::of(&row(false, Some("/poster.jpg")), None, true),
            state(true, false, Artwork::Missing)
        );
        assert_eq!(
            ArtworkState::of(&row(false, None), None, false),
            state(false, false, Artwork::Unknown)
        );
        assert_eq!(
            ArtworkState::of(
                &row(true, Some("/poster.jpg")),
                Some(Path::new("/cache/images/abc.webp")),
                true
            ),
            state(true, true, Artwork::Cached)
        );
        assert_eq!(
            ArtworkState::of(&row(true, None), None, true),
            state(true, true, Artwork::Unknown),
            "a filled title whose provider recorded no poster"
        );
    }

    #[test]
    fn a_provider_can_only_answer_about_a_title_it_holds_an_id_for() {
        // The comparison the whole gate rests on. A configured provider with no id for the title is
        // a provider that cannot be asked about it — `refresh_title` would answer `None` without a
        // request, so the screen would start a job that does nothing.
        assert!(!can_answer(&[], &[]));
        assert!(!can_answer(&[], &[id(Provider::Tmdb, "1")]));
        assert!(!can_answer(&[Provider::Tmdb], &[]));
        assert!(can_answer(&[Provider::Tmdb], &[id(Provider::Tmdb, "1")]));
        // Another provider's id is not this provider's to serve: the id carries the provider.
        assert!(!can_answer(
            &[Provider::Tmdb],
            &[id(Provider::Imdb, "tt0078748")]
        ));
        // And one match among several ids is enough, whichever order they arrive in.
        assert!(can_answer(
            &[Provider::Tmdb],
            &[id(Provider::Imdb, "tt0078748"), id(Provider::Tmdb, "1")]
        ));
    }

    /// One external id, as the catalogue holds it.
    fn id(provider: Provider, value: &str) -> peregrine_domain::ExternalId {
        peregrine_domain::ExternalId::new(provider, value).expect("the fixture passes a real id")
    }

    #[test]
    fn a_title_no_configured_provider_knows_is_never_asked_about() {
        // Every other fact says yes — nobody has filled it in, the poster is missing — and the answer
        // is still no, because there is nothing to ask *with*. Without this the screen would start a
        // job and write a journal row on every visit to a title no provider can be asked about, and
        // the job would answer immediately having done nothing.
        for artwork in [Artwork::Unknown, Artwork::Missing, Artwork::Cached] {
            for synced in [false, true] {
                assert!(
                    !refresh_due(&state(false, synced, artwork)),
                    "answerable=false must decide the rule on its own: {synced} {artwork:?}"
                );
            }
        }
    }
}
