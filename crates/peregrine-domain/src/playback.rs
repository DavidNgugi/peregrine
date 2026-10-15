//! What this build can play from a file it holds, and the one fact that decides it.
//!
//! **A rule, and therefore here rather than in a screen** (AGENTS R7). The question a Play control
//! needs answered is "will pressing this play the file", and the answer is a property of the
//! *container*: whether the thing that will read the bytes can demux it. This build has two readers —
//! the webview's own media stack, and mpv once a component pack ships it (ADR 0023 §4) — and which of
//! them a file needs is not something a renderer can work out. A `<video>` element reports an error
//! *code* after failing, never a reason, and "it did not work" is not something a person can act on.
//!
//! **The container is read off the file's name, and that is a deliberate limit rather than an
//! oversight.** `media_file.container` is a column, and it is empty in every row this build has ever
//! written, because filling it is `ffprobe`'s job and no sidecar exists yet (`PG-E10-02`, `PG-E26-01`).
//! A name's extension is not the container — a `.mkv` cannot hold MP4's boxes — but for the one
//! decision made here it is the *right* kind of evidence: demuxing is chosen by container, and the
//! extension is the container's own conventional name. When probing lands, this reads the column and
//! the fallback below disappears; nothing else changes.
//!
//! **What is *not* decided here is the codec.** A file inside a container the webview demuxes can
//! still carry a video stream the platform's decoder refuses, and no name distinguishes those. That
//! is the `Webview` answer's honest caveat: it means "this build will try", not "this will play".

/// What this build will do with a file, from the container its name states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocalPlayback {
    /// A container the webview's own media stack demuxes, so playback needs no sidecar.
    ///
    /// A *promise to try* rather than a promise to succeed: the container is the half a name can
    /// answer, and the codec inside it is not. Linux is the platform this reads most loosely, because
    /// `WebKitGTK` demuxes whatever `GStreamer` plugins the distribution happened to install.
    Webview,
    /// A container the webview does not demux, so this build cannot play it until mpv ships.
    ///
    /// The honest sentence beside a disabled Play control, derived from a fact rather than from a
    /// failed attempt.
    ExternalPlayer,
    /// A name this build does not recognise as a media container at all.
    ///
    /// Distinct from [`Self::ExternalPlayer`] on purpose: "this build cannot play that yet" and "this
    /// build does not know what that is" are different things to tell a person, and a single answer
    /// for both would be a screen guessing which one it meant.
    Unknown,
}

/// The containers the webview's own media stack is trusted with.
///
/// `mp4`, `m4v`, and `mov` are ISO base media file format — one demuxer, three names — and `webm` is
/// the other container `Chromium` and `WebKit` both read natively. Nothing else is here, because a
/// container that is *sometimes* demuxed is a container whose failure this build would report as a
/// broken file rather than as a missing sidecar.
const WEBVIEW_EXTENSIONS: &[&str] = &["mp4", "m4v", "mov", "webm"];

/// The containers this build will hand to mpv when mpv exists.
///
/// Matroska is the one that matters today: it is the most common container for a high-definition
/// release and it is what the tenth file in this project's own test library is. The rest are here
/// because they are the same answer for the same reason — a demuxer the webview does not have — and
/// naming them costs a table entry rather than a branch.
const EXTERNAL_EXTENSIONS: &[&str] = &[
    "mkv", "mk3d", "avi", "divx", "wmv", "flv", "ts", "m2ts", "mts", "mpg", "mpeg", "vob", "ogv",
    "ogm", "rmvb", "asf", "3gp",
];

impl LocalPlayback {
    /// What this build will do with a file whose name is `name`.
    ///
    /// Case-insensitive, because a filesystem may be either and a scan records the name the disk
    /// spelled. A name with no dot, a trailing dot, or a dot in a directory rather than a file name
    /// is [`Self::Unknown`] rather than a guess.
    #[must_use]
    pub fn of_name(name: &str) -> Self {
        // `rsplit_once` rather than `Path::extension`: this is a pure function that must not depend on
        // the platform's separator rules, and the caller may hand it a bare file name or a whole path
        // — both of which split the same way on the last dot *after* the last separator.
        let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
        let Some((stem, extension)) = file.rsplit_once('.') else {
            return Self::Unknown;
        };
        // A leading dot is a hidden file's marker rather than an extension, and an empty stem means
        // the "extension" is the whole name.
        if stem.is_empty() || extension.is_empty() {
            return Self::Unknown;
        }
        let extension = extension.to_ascii_lowercase();
        if WEBVIEW_EXTENSIONS.contains(&extension.as_str()) {
            return Self::Webview;
        }
        if EXTERNAL_EXTENSIONS.contains(&extension.as_str()) {
            return Self::ExternalPlayer;
        }
        Self::Unknown
    }

    /// The stable word this answer is written with, for the log and for a test.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Webview => "webview",
            Self::ExternalPlayer => "external-player",
            Self::Unknown => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_containers_the_webview_reads_are_answered_as_the_webview_s() {
        // The nine files this project's own library is mostly made of. An MP4 is the case the whole
        // decision exists for: it plays with no sidecar, no download, and no packaging change.
        for name in ["a.mp4", "a.m4v", "a.MOV", "A.Mp4", "a.webm"] {
            assert_eq!(
                LocalPlayback::of_name(name),
                LocalPlayback::Webview,
                "{name}"
            );
        }
    }

    #[test]
    fn matroska_is_handed_to_the_external_player_rather_than_attempted() {
        // The tenth file in this library: HEVC inside an MKV. A WebKit `<video>` cannot demux it, so
        // the honest answer is the one that produces a sentence instead of a black rectangle.
        assert_eq!(
            LocalPlayback::of_name("Nuremberg 2025 1080p HDRip HC HEVC x265 5.1 BONE.mkv"),
            LocalPlayback::ExternalPlayer
        );
        assert_eq!(
            LocalPlayback::of_name("x.MKV"),
            LocalPlayback::ExternalPlayer
        );
    }

    #[test]
    fn a_whole_path_is_read_the_way_a_bare_name_is() {
        // The scan records an absolute path, and the caller may hand over either: a rule that split on
        // the first dot would read a directory called `Movies.2024` as the file's container.
        assert_eq!(
            LocalPlayback::of_name("/Users/someone/Downloads/Movies.2024/film.mp4"),
            LocalPlayback::Webview
        );
        assert_eq!(
            LocalPlayback::of_name(r"C:\Users\someone\film.mkv"),
            LocalPlayback::ExternalPlayer
        );
    }

    #[test]
    fn a_name_that_states_no_container_is_unknown_rather_than_a_guess() {
        // Every one of these is a name this build cannot read a container out of, and guessing would
        // make the screen promise a playback it cannot keep.
        for name in [
            "film",
            "film.",
            ".mkv",
            "",
            ".",
            "..",
            "/a/b/",
            "film.mp4.bak",
            "film.zip",
            "film.txt",
        ] {
            assert_eq!(
                LocalPlayback::of_name(name),
                LocalPlayback::Unknown,
                "{name}"
            );
        }
    }

    #[test]
    fn unknown_and_external_are_different_answers() {
        // The distinction is the point: "this build cannot play that until mpv ships" tells a person
        // to wait, and "this build does not know what that is" tells them to check the file. One
        // answer for both would be a screen choosing which of those to say.
        assert_ne!(LocalPlayback::Unknown, LocalPlayback::ExternalPlayer);
        assert_ne!(
            LocalPlayback::of_name("a.zip"),
            LocalPlayback::of_name("a.mkv")
        );
    }

    #[test]
    fn every_answer_has_its_own_word() {
        // The words cross to the webview, so a collision here would collapse two answers into one
        // there without a compile error anywhere.
        let words: Vec<&str> = [
            LocalPlayback::Webview,
            LocalPlayback::ExternalPlayer,
            LocalPlayback::Unknown,
        ]
        .into_iter()
        .map(LocalPlayback::as_str)
        .collect();
        let mut unique = words.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), words.len(), "{words:?}");
    }
}
