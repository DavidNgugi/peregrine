//! What a library scan treats as media, and what it skips — and why (PG-E14-02).
//!
//! A scanner that walks a media folder meets far more than films: release samples, an `Extras`
//! folder of interviews, half-downloaded `.part` files, macOS's `._` twins, editor backups, and
//! every sidecar the naming convention puts beside a video. Indexing those produces a library
//! full of entries nobody wants, so the decision about what a *candidate* is belongs here, where
//! it is a rule with a name rather than a condition buried in a directory walk.
//!
//! Nothing here touches the filesystem. A path is text to this module, which is what lets the
//! whole policy be tested as a table of names and expected reasons rather than by building a
//! corpus for every case — the walk that has to exist anyway (PG-E14-02's scanner) covers the
//! other half, against real files in a real directory.
//!
//! # The lists are this build's policy, and the extension list is deliberately small
//!
//! `docs/ARCHITECTURE.md` does not enumerate media containers — `media_file.container` is filled
//! by `ffprobe` (PG-E14-10) — so the extension list below is this build's decision about what to
//! hand the prober. It is small on purpose: an extension this list omits costs a file that can be
//! added by a future entry, while one it wrongly includes costs a `.srt` or an `.nfo` indexed as a
//! film. `.vob` is the interesting omission: it is a piece of a disc structure rather than a
//! title, and indexing half a DVD as a media file is worse than ignoring it until disc support
//! exists.

use std::path::Path;

/// The containers this build hands to the prober, lowercased and without the dot.
///
/// Grouped by family so the omissions are visible as omissions: a Matroska, MP4, or MPEG
/// transport stream file is media; a DVD's `.vob` is a piece of a structure, and `RealMedia` and
/// disc-image formats are not in scope.
const MEDIA_EXTENSIONS: &[&str] = &[
    // Matroska.
    "mkv", "mk3d", "webm",
    // MPEG-4 family: `.m4v` is iTunes' video container, `.mov` is QuickTime.
    "mp4", "m4v", "mov", "3gp", "3g2",
    // MPEG transport and program streams, including the camcorder variants.
    "ts", "m2ts", "mts", "mpg", "mpeg", // Everything else people actually have.
    "avi", "wmv", "flv", "ogv", "asf",
];

/// Suffixes that mean "this file is not finished being written", lowercased.
///
/// A partial download indexed as a film is a library entry whose file cannot be played, and it is
/// exactly the file a scan is most likely to meet, because a download in progress sits in the
/// library folder while it happens.
const TEMPORARY_SUFFIXES: &[&str] = &[
    "part",
    "partial",
    "crdownload",
    "download",
    "tmp",
    "temp",
    "filepart",
    "!ut",
    "bc!",
];

/// Directory names whose contents are extras rather than titles, lowercased.
///
/// Plex's and Jellyfin's convention, minus the two entries that would be dangerous here:
/// `specials` is a television season (`season 0`) and not an extras folder, and `scenes` and
/// `shorts` are ordinary words for a film to be called. A library that eats a show's specials or
/// a film called `Shorts` has lost content, which is a worse failure than indexing one interview.
const EXTRAS_DIRECTORIES: &[&str] = &[
    "extras",
    "featurettes",
    "behind the scenes",
    "deleted scenes",
    "interviews",
];

/// What a scan recorded about one file, which is what the next scan compares.
///
/// Two numbers, because each catches what the other misses. A **size** change is what a rewrite
/// inside a filesystem's mtime granularity looks like — FAT and NTFS round to two seconds, some
/// network shares to one, and a download that finished and was re-fetched within that window keeps
/// its timestamp. An **mtime** change is what an edit that happens to preserve the length looks
/// like. Both have to match for a file to be left alone, which is the rule
/// [`Freshness::of`] applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Stamp {
    /// The file's size in bytes.
    pub size_bytes: u64,
    /// Its modification time, in milliseconds since the Unix epoch.
    pub mtime_ms: i64,
}

/// What an incremental pass decided about one candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// The library has never seen this path.
    New,
    /// The library has it, and neither number changed.
    Unchanged,
    /// The library has it and something changed.
    Changed,
}

impl Freshness {
    /// Classifies a candidate against what the library already recorded for its path.
    ///
    /// # The hash fallback this does not have, and cannot yet
    ///
    /// The one case two numbers cannot see is content that changed while both stayed the same:
    /// same length, same second. Catching it means digesting the file and comparing the digest
    /// with the one from last time — and `media_file` has no digest column
    /// (`docs/DATA_MODEL.md`), so there is nothing to compare against. Digesting every unchanged
    /// file on every scan is exactly the work incremental scanning exists to avoid, so the
    /// fallback is not a shortcut away: it needs a stored digest plus a rule for *when* to spend a
    /// read, and neither exists yet. The acceptance this rule was written for — a file that
    /// changes size while its mtime stays put — is decided by the size alone, which is why it
    /// works without one.
    #[must_use]
    pub fn of(recorded: Option<Stamp>, candidate: Stamp) -> Self {
        match recorded {
            None => Self::New,
            Some(recorded) if recorded == candidate => Self::Unchanged,
            Some(_) => Self::Changed,
        }
    }

    /// Whether this candidate needs the expensive path: a parse, a probe, and possibly a match.
    #[must_use]
    pub const fn needs_work(self) -> bool {
        !matches!(self, Self::Unchanged)
    }
}

/// Why a path is not a library candidate.
///
/// The reasons are the ones the task names plus the two the walk needs: a link it will not
/// follow, and an extension this build does not treat as media. A reason is a stable label
/// rather than a sentence, so a scan can log `reason = "sample"` and a diagnostics screen can
/// count them without parsing prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SkipReason {
    /// A name that begins with a dot, which every platform treats as hidden.
    Hidden,
    /// A file that is still being written, or a scratch file an editor left behind.
    Temporary,
    /// A release sample.
    Sample,
    /// A folder of extras.
    Extras,
    /// A trailer rather than the title.
    Trailer,
    /// A symbolic link, which this build does not follow.
    Symlink,
    /// An extension that is not a media container.
    Unsupported,
}

impl SkipReason {
    /// The label a log line and a diagnostics count carry.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hidden => "hidden",
            Self::Temporary => "temporary",
            Self::Sample => "sample",
            Self::Extras => "extras",
            Self::Trailer => "trailer",
            Self::Symlink => "symlink",
            Self::Unsupported => "unsupported",
        }
    }
}

/// What the policy decided about one path.
///
/// Named `ScanVerdict` rather than `Verdict` because the torrent inspector already has one
/// (`peregrine_domain::Verdict`, PG-E4-07): two types with one name in one crate is how a merge
/// puts the wrong one in scope, which this repository has already paid for once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanVerdict {
    /// The library could hold this file.
    Candidate,
    /// It could not, and this is why.
    Skipped(SkipReason),
}

impl ScanVerdict {
    /// Whether the file is a candidate.
    #[must_use]
    pub const fn is_candidate(self) -> bool {
        matches!(self, Self::Candidate)
    }
}

/// Whether an extension, without its dot and in any case, is a container this build scans.
#[must_use]
pub fn is_media_extension(extension: &str) -> bool {
    let lowered = extension.to_ascii_lowercase();
    MEDIA_EXTENSIONS.contains(&lowered.as_str())
}

/// The tokens in a file stem, lowercased, split on the separators release names use.
///
/// `The.Movie.2024.1080p-GROUP` becomes `["the", "movie", "2024", "1080p", "group"]`, which is
/// what lets the sample and trailer rules match a *word* rather than a substring: `sampler` and
/// `trailersmith` are not `sample` and not `trailer`.
fn tokens(stem: &str) -> Vec<String> {
    stem.split(|character: char| {
        character == '.' || character == '-' || character == '_' || character.is_whitespace()
    })
    .filter(|token| !token.is_empty())
    .map(str::to_ascii_lowercase)
    .collect()
}

/// Whether a name is one of `list`, compared case-insensitively with runs of spaces collapsed.
fn is_named(name: &str, list: &[&str]) -> bool {
    let lowered = name.trim().to_ascii_lowercase();
    let collapsed = lowered.split_whitespace().collect::<Vec<_>>().join(" ");
    list.contains(&collapsed.as_str())
}

/// A library's scan policy.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScanPolicy;

impl ScanPolicy {
    /// The policy this build ships.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Whether a directory is walked, or skipped and why.
    ///
    /// Only the directory's own name is considered: a directory is skipped because of what it is
    /// called, and a hidden or extras directory anywhere in the tree takes its whole subtree with
    /// it, which is the behaviour a user expects from a folder they named `Extras`.
    #[must_use]
    pub fn directory(self, name: &str) -> ScanVerdict {
        if name.starts_with('.') {
            return ScanVerdict::Skipped(SkipReason::Hidden);
        }
        if is_named(name, EXTRAS_DIRECTORIES) {
            return ScanVerdict::Skipped(SkipReason::Extras);
        }
        if is_named(name, &["sample", "samples"]) {
            return ScanVerdict::Skipped(SkipReason::Sample);
        }
        if is_named(name, &["trailer", "trailers"]) {
            return ScanVerdict::Skipped(SkipReason::Trailer);
        }
        ScanVerdict::Candidate
    }

    /// Whether a file is a candidate, or skipped and why.
    ///
    /// `path` is the path *below its root*, which is the distinction that stops a policy about
    /// names from reading the user's own configuration: a library at `/Users/me/.media/Movies` is
    /// a path the user chose, so the dot in `.media` is not a reason to skip every file in it. The
    /// scanner gives this function a stripped path for that reason, and a caller classifying a path
    /// by hand has to do the same.
    ///
    /// `symlink` is the walk's answer rather than this module's: whether a path is a link is a
    /// filesystem fact, and this module has no filesystem. Passing it in keeps the decision in
    /// one place rather than letting the walk skip links behind the policy's back.
    ///
    /// Every component below the root is checked, not only the file's own name, because a file
    /// inside a `Sample` folder is a sample whatever it is called.
    #[must_use]
    pub fn file(self, path: &Path, symlink: bool) -> ScanVerdict {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return ScanVerdict::Skipped(SkipReason::Unsupported);
        };

        // The components above the file first: a `Sample/` folder is a decision about everything
        // under it, and reporting the reason the user would give is more useful than reporting one
        // they cannot see.
        for component in path.parent().into_iter().flat_map(Path::components) {
            let std::path::Component::Normal(part) = component else {
                continue;
            };
            let Some(part) = part.to_str() else {
                return ScanVerdict::Skipped(SkipReason::Unsupported);
            };
            match self.directory(part) {
                ScanVerdict::Candidate => {}
                ScanVerdict::Skipped(reason) => return ScanVerdict::Skipped(reason),
            }
        }

        if name.starts_with('.') {
            return ScanVerdict::Skipped(SkipReason::Hidden);
        }
        if is_temporary(name) {
            return ScanVerdict::Skipped(SkipReason::Temporary);
        }
        if symlink {
            return ScanVerdict::Skipped(SkipReason::Symlink);
        }

        let (stem, extension) = match name.rsplit_once('.') {
            Some((stem, extension)) if !stem.is_empty() => (stem, extension),
            // No dot at all: a file called `movie` has no container to hand the prober.
            _ => return ScanVerdict::Skipped(SkipReason::Unsupported),
        };

        // The sample rule looks at the *last* token only, and that asymmetry is the whole reason
        // the rule is safe: release samples are named `…-sample.mkv`, while films called
        // `Sample This` begin with the word and would be lost by a rule that matched anywhere.
        let stem_tokens = tokens(stem);
        if stem_tokens
            .last()
            .is_some_and(|token| token == "sample" || token == "samples")
        {
            return ScanVerdict::Skipped(SkipReason::Sample);
        }

        // Trailers are named at either end in practice — `Trailer - The Movie` and
        // `The.Movie-trailer` are both common — and a film called `Trailer` is rare enough that
        // the trade is worth making the other way around.
        let is_trailer = |token: &String| token == "trailer" || token == "trailers";
        if stem_tokens.first().is_some_and(is_trailer) || stem_tokens.last().is_some_and(is_trailer)
        {
            return ScanVerdict::Skipped(SkipReason::Trailer);
        }

        if !is_media_extension(extension) {
            return ScanVerdict::Skipped(SkipReason::Unsupported);
        }

        ScanVerdict::Candidate
    }
}

/// Whether a file name looks like something that is not finished, or not a library file at all.
fn is_temporary(name: &str) -> bool {
    // Office writes `~$document.xlsx`, and every editor that keeps a backup writes `file~`.
    if name.starts_with("~$") || name.ends_with('~') {
        return true;
    }

    let Some((_, extension)) = name.rsplit_once('.') else {
        return false;
    };
    TEMPORARY_SUFFIXES.contains(&extension.to_ascii_lowercase().as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::path::PathBuf;

    fn verdict(path: &str) -> ScanVerdict {
        ScanPolicy::new().file(&PathBuf::from(path), false)
    }

    fn reason(path: &str) -> SkipReason {
        match verdict(path) {
            ScanVerdict::Skipped(reason) => reason,
            ScanVerdict::Candidate => panic!("{path} was expected to be skipped"),
        }
    }

    #[test]
    fn an_ordinary_film_is_a_candidate() {
        assert_eq!(
            verdict("/media/Movies/Alien (1979).mkv"),
            ScanVerdict::Candidate
        );
        assert_eq!(
            verdict("/media/Shows/Breaking Bad/Season 1/S01E01.mkv"),
            ScanVerdict::Candidate
        );
        assert_eq!(
            verdict("/media/Movies/Up.2009.1080p.BluRay.x264-GROUP.mp4"),
            ScanVerdict::Candidate
        );
    }

    #[test]
    fn every_container_the_build_claims_is_one() {
        // The table the extension decision rests on, asserted as a table: a name added to the list
        // without a test here would be a format the scanner claims to handle and nothing checks.
        for extension in MEDIA_EXTENSIONS {
            assert_eq!(
                verdict(&format!("/media/Movie.{extension}")),
                ScanVerdict::Candidate,
                "{extension}"
            );
            assert!(
                is_media_extension(&extension.to_ascii_uppercase()),
                "{extension} in upper case"
            );
        }
    }

    #[test]
    fn sidecars_are_not_media() {
        // The files every release ships beside the video, none of which is a title.
        for name in [
            "Alien (1979).srt",
            "Alien (1979).en.srt",
            "Alien (1979).nfo",
            "poster.jpg",
            "fanart.png",
            "Alien (1979).txt",
            "playlist.m3u",
            "checksum.sfv",
        ] {
            assert_eq!(
                reason(&format!("/media/Movies/{name}")),
                SkipReason::Unsupported,
                "{name}"
            );
        }
    }

    #[test]
    fn a_file_with_no_extension_is_not_media() {
        assert_eq!(reason("/media/Movies/Alien"), SkipReason::Unsupported);
        assert_eq!(reason("/media/Movies/.hidden"), SkipReason::Hidden);
    }

    #[test]
    fn hidden_names_are_skipped_at_every_level() {
        assert_eq!(reason("/media/Movies/.Alien.mkv"), SkipReason::Hidden);
        assert_eq!(reason("/media/.Trash/Alien.mkv"), SkipReason::Hidden);
        // The macOS AppleDouble twin of a real file, which is a dot file like any other.
        assert_eq!(reason("/media/Movies/._Alien.mkv"), SkipReason::Hidden);
    }

    #[test]
    fn a_hidden_directory_takes_its_contents_with_it() {
        assert_eq!(reason("/media/.stfolder/Alien.mkv"), SkipReason::Hidden);
    }

    #[test]
    fn files_that_are_still_being_written_are_skipped() {
        // The case that matters most: a download in progress sits in the library folder, and
        // indexing it produces an entry whose file cannot be played.
        for name in [
            "Alien.mkv.part",
            "Alien.mkv.partial",
            "Alien.mkv.crdownload",
            "Alien.mkv.download",
            "Alien.mkv.!ut",
            "Alien.mkv.bc!",
            "Alien.mkv.filepart",
            "Alien.tmp",
            "Alien.temp",
            "~$Alien.mkv",
            "Alien.mkv~",
        ] {
            assert_eq!(
                reason(&format!("/media/Movies/{name}")),
                SkipReason::Temporary,
                "{name}"
            );
        }
    }

    #[test]
    fn a_release_sample_is_skipped_at_either_end_of_its_name() {
        assert_eq!(reason("/media/Movies/Alien-sample.mkv"), SkipReason::Sample);
        assert_eq!(
            reason("/media/Movies/Alien.1979.1080p.BluRay.x264-GROUP-sample.mkv"),
            SkipReason::Sample
        );
        assert_eq!(
            reason("/media/Movies/Alien/Sample/Alien.mkv"),
            SkipReason::Sample
        );
        assert_eq!(
            reason("/media/Movies/Alien/Samples/clip.mkv"),
            SkipReason::Sample
        );
    }

    #[test]
    fn a_film_whose_name_begins_with_sample_is_not_a_sample() {
        // The false positive the rule's shape exists to avoid: a rule that matched `sample`
        // anywhere in the stem would drop this film, and the release convention it is trying to
        // catch only ever puts the word at the end.
        assert_eq!(
            verdict("/media/Movies/Sample This (2012).mkv"),
            ScanVerdict::Candidate
        );
        assert_eq!(
            verdict("/media/Movies/The Sampler (2012).mkv"),
            ScanVerdict::Candidate
        );
    }

    #[test]
    fn a_word_that_merely_contains_sample_or_trailer_is_not_one() {
        assert_eq!(
            verdict("/media/Movies/Sampler.2019.mkv"),
            ScanVerdict::Candidate
        );
        assert_eq!(
            verdict("/media/Movies/Trailersmith.2019.mkv"),
            ScanVerdict::Candidate
        );
    }

    #[test]
    fn trailers_are_skipped_by_name_and_by_folder() {
        assert_eq!(
            reason("/media/Movies/Trailer - Alien.mkv"),
            SkipReason::Trailer
        );
        assert_eq!(
            reason("/media/Movies/Alien-trailer.mkv"),
            SkipReason::Trailer
        );
        assert_eq!(
            reason("/media/Movies/Alien/Trailers/Alien.mkv"),
            SkipReason::Trailer
        );
    }

    #[test]
    fn an_extras_folder_is_skipped_and_so_is_everything_under_it() {
        for folder in EXTRAS_DIRECTORIES {
            assert_eq!(
                reason(&format!("/media/Movies/Alien/{folder}/interview.mkv")),
                SkipReason::Extras,
                "{folder}"
            );
        }
        // The spellings people actually use, which are not the ones in the table.
        assert_eq!(
            reason("/media/Movies/Alien/Behind The Scenes/clip.mkv"),
            SkipReason::Extras
        );
        assert_eq!(
            reason("/media/Movies/Alien/Deleted Scenes/clip.mkv"),
            SkipReason::Extras
        );
    }

    #[test]
    fn a_directory_that_merely_resembles_extras_is_walked() {
        // `specials` is a television season, `shorts` and `scenes` are ordinary words. Eating
        // them would lose content, which is worse than indexing one interview.
        for folder in ["Specials", "Season 0", "Shorts", "Scenes", "Extra"] {
            assert_eq!(
                ScanPolicy::new().directory(folder),
                ScanVerdict::Candidate,
                "{folder}"
            );
        }
    }

    #[test]
    fn the_reason_a_directory_is_skipped_is_the_reason_for_its_contents() {
        let policy = ScanPolicy::new();

        assert_eq!(
            policy.directory("Extras"),
            ScanVerdict::Skipped(SkipReason::Extras)
        );
        assert_eq!(
            policy.directory("extras"),
            ScanVerdict::Skipped(SkipReason::Extras)
        );
        assert_eq!(
            policy.directory("Sample"),
            ScanVerdict::Skipped(SkipReason::Sample)
        );
        assert_eq!(
            policy.directory(".hidden"),
            ScanVerdict::Skipped(SkipReason::Hidden)
        );
        assert_eq!(policy.directory("Season 1"), ScanVerdict::Candidate);
        assert_eq!(policy.directory("Movies"), ScanVerdict::Candidate);
    }

    #[test]
    fn a_link_is_skipped_rather_than_followed() {
        // Whether a path is a link is the walk's answer; the policy is what gives it a reason.
        // A link inside a root that points outside it is the way an index ends up holding files
        // the user never pointed at.
        assert_eq!(
            ScanPolicy::new().file(Path::new("/media/Movies/Alien.mkv"), true),
            ScanVerdict::Skipped(SkipReason::Symlink)
        );
    }

    #[test]
    fn a_link_is_reported_as_a_link_rather_than_as_a_bad_extension() {
        // Order matters for the log line a user reads: "we do not follow links" is actionable,
        // "unsupported format" is not, and a link named `Alien.mkv.lnk` would otherwise report
        // the second.
        assert_eq!(
            ScanPolicy::new().file(Path::new("/media/Movies/Alien.mkv.part"), true),
            ScanVerdict::Skipped(SkipReason::Temporary),
            "a name that says what it is wins over the link, because the user can see the name"
        );
    }

    #[test]
    fn every_reason_has_its_own_label() {
        let labels: Vec<&str> = [
            SkipReason::Hidden,
            SkipReason::Temporary,
            SkipReason::Sample,
            SkipReason::Extras,
            SkipReason::Trailer,
            SkipReason::Symlink,
            SkipReason::Unsupported,
        ]
        .into_iter()
        .map(SkipReason::as_str)
        .collect();

        let mut unique = labels.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), labels.len(), "two reasons share a label");
        assert!(labels.iter().all(|label| !label.is_empty()));
    }

    #[test]
    fn a_candidate_is_a_candidate_and_a_skip_is_not() {
        assert!(verdict("/media/Alien.mkv").is_candidate());
        assert!(!verdict("/media/Alien.srt").is_candidate());
    }

    #[test]
    fn a_path_whose_components_are_not_text_is_not_a_candidate() {
        // A path this module cannot read as text is one the schema cannot store either, so it is
        // skipped rather than guessed at. On Unix a byte sequence that is not UTF-8 reaches here.
        #[cfg(unix)]
        {
            use std::ffi::OsStr;
            use std::os::unix::ffi::OsStrExt as _;

            let name = OsStr::from_bytes(b"Alien\xff.mkv");
            let path = PathBuf::from("/media/Movies").join(name);
            assert_eq!(
                ScanPolicy::new().file(&path, false),
                ScanVerdict::Skipped(SkipReason::Unsupported)
            );
        }
    }

    fn stamp(size_bytes: u64, mtime_ms: i64) -> Stamp {
        Stamp {
            size_bytes,
            mtime_ms,
        }
    }

    #[test]
    fn a_file_that_changed_size_without_changing_its_mtime_is_detected() {
        // PG-E14-03's acceptance. This is the case the size half of the pair exists for: a rewrite
        // inside the filesystem's mtime granularity leaves the timestamp alone, so a rule that
        // trusted mtime alone would skip the file forever.
        assert_eq!(
            Freshness::of(
                Some(stamp(1_000, 1_700_000_000_000)),
                stamp(1_001, 1_700_000_000_000)
            ),
            Freshness::Changed
        );
        // The other direction, which is the same defect: a truncation.
        assert_eq!(
            Freshness::of(
                Some(stamp(1_000, 1_700_000_000_000)),
                stamp(999, 1_700_000_000_000)
            ),
            Freshness::Changed
        );
    }

    #[test]
    fn a_file_that_changed_mtime_without_changing_its_size_is_detected() {
        // An edit that happens to preserve the length: the size half sees nothing.
        assert_eq!(
            Freshness::of(
                Some(stamp(1_000, 1_700_000_000_000)),
                stamp(1_000, 1_700_000_000_001)
            ),
            Freshness::Changed
        );
    }

    #[test]
    fn both_numbers_matching_is_the_only_way_a_file_is_left_alone() {
        let recorded = stamp(1_000, 1_700_000_000_000);

        assert_eq!(
            Freshness::of(Some(recorded), recorded),
            Freshness::Unchanged
        );
        assert!(!Freshness::of(Some(recorded), recorded).needs_work());
    }

    #[test]
    fn a_path_the_library_has_never_seen_is_new_rather_than_changed() {
        // The states are different work: a new file is parsed and matched, an existing one that
        // changed is re-probed against an item that is already there.
        assert_eq!(Freshness::of(None, stamp(1, 2)), Freshness::New);
        assert!(Freshness::of(None, stamp(1, 2)).needs_work());
    }

    #[test]
    fn a_change_in_either_number_needs_work_and_a_match_does_not() {
        assert!(Freshness::Changed.needs_work());
        assert!(Freshness::New.needs_work());
        assert!(!Freshness::Unchanged.needs_work());
    }

    proptest! {
        /// The rule as a property: `Unchanged` holds exactly when the two stamps are equal, so no
        /// pair of numbers reaches "leave it alone" without both having been compared.
        #[test]
        fn unchanged_holds_exactly_for_equal_stamps(
            size in 0_u64..u64::MAX,
            mtime in i64::MIN..i64::MAX,
            other_size in 0_u64..u64::MAX,
            other_mtime in i64::MIN..i64::MAX,
        ) {
            let recorded = stamp(size, mtime);
            let candidate = stamp(other_size, other_mtime);
            let expected = if recorded == candidate {
                Freshness::Unchanged
            } else {
                Freshness::Changed
            };
            prop_assert_eq!(Freshness::of(Some(recorded), candidate), expected);
        }
    }

    #[test]
    fn a_case_insensitive_filesystem_still_scans_candidates() {
        // `.MKV` and `.Mkv` are the same file to a case-insensitive filesystem and different
        // strings to this module, so the extension check folds case.
        for name in ["Alien.MKV", "Alien.Mkv", "Alien.mKv"] {
            assert_eq!(
                verdict(&format!("/media/{name}")),
                ScanVerdict::Candidate,
                "{name}"
            );
        }
    }
}
