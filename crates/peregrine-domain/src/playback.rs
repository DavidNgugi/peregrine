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

use std::fmt;
use std::sync::Arc;

use crate::path::SafePath;
use thiserror::Error;

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

/// Why an external player could not be started.
///
/// Two variants rather than one because the user acts differently: an uninstalled player is
/// something to install, while a spawn failure is this machine refusing a binary that is there.
/// Neither sentence names a path (AGENTS R21).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PlayerError {
    /// No mpv binary was found in the places this build looks.
    #[error("mpv is not installed, so this build has no player for that file yet")]
    NotInstalled,
    /// The binary exists but the operating system refused to start it.
    #[error("mpv is installed but the operating system refused to start it")]
    Spawn,
}

/// A URL this process serves on its own loopback listener.
///
/// **The only kind of stream a player may be pointed at.** A player is handed a URL instead of a path
/// when the bytes come from the torrent engine (`PG-E13-09`), and the safety of that is the address:
/// the listener is bound to `127.0.0.1`, the port is ephemeral, and the path segment is a per-session
/// credential. Parsing it here stops a future caller handing mpv a provider URL, a `file:` path, or a
/// host on the network; the constructor refuses all three.
///
/// The `Debug` implementation prints none of the value, because the path segment is the session token
/// and a log line carrying it would be a bearer credential in a log (`SECURITY.md` T13).
#[derive(Clone, PartialEq, Eq)]
pub struct LoopbackUrl(String);

impl LoopbackUrl {
    /// Parses a loopback stream URL, or answers `None` when it is anything else.
    ///
    /// Only `http`, only `127.0.0.1`, only a non-zero port, and only a non-empty path. A userinfo
    /// section, a query, a fragment, whitespace, a control character, or a backslash is refused rather
    /// than normalised: this is the address of a socket this process bound, not a value to repair.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let rest = value.strip_prefix("http://")?;
        if value.chars().any(char::is_control)
            || rest
                .chars()
                .any(|character| matches!(character, '@' | '#' | '?' | ' ' | '\\'))
        {
            return None;
        }
        let (authority, path) = rest.split_once('/')?;
        if path.is_empty() {
            return None;
        }
        let port = authority.strip_prefix("127.0.0.1:")?;
        if !port.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let port: u16 = port.parse().ok()?;
        if port == 0 {
            return None;
        }
        Some(Self(value.to_owned()))
    }

    /// The URL, as the player receives it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for LoopbackUrl {
    /// Says what the value is and nothing it contains: the token is in the path.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LoopbackUrl(<redacted>)")
    }
}

/// What a player is asked to open: a file this process may read, or a stream it serves itself.
#[derive(Clone, Copy, Debug)]
pub enum PlayerInput<'a> {
    /// A guarded file path.
    File(&'a SafePath),
    /// A URL this process serves on loopback, which only [`LoopbackUrl`] can construct.
    Stream(&'a LoopbackUrl),
}

/// Where a player reports how far it has got.
///
/// The same pair the in-page player sends the progress write: a position in milliseconds and the
/// media duration when the player knows one. A backend that cannot report simply never calls it.
pub trait PositionSink: Send + Sync {
    /// The current position and the media duration, in milliseconds.
    fn position(&self, position_ms: i64, duration_ms: Option<i64>);
}

/// The player that handles containers the webview cannot demux.
///
/// A port rather than a direct spawn because launching an external process is an adapter concern
/// (AGENTS R8), and because a test can then assert the action without a player on the machine.
pub trait PlayerBackend: Send + Sync {
    /// Opens a file or a stream in the shared external player, replacing whatever it was showing.
    ///
    /// `start_ms` is where the app's own watch progress says to resume, or `None` for the
    /// beginning. `subtitle` is the sidecar the session selected, or `None` for no subtitle; it is a
    /// guarded path for the same reason the media is, because it is handed to another process, and a
    /// stream has no sidecar row to offer. `audio_language` is the account's preferred language, or
    /// `None` when it has expressed none; a backend that can choose an initial track must honour it,
    /// while a later manual choice belongs to that player session. `positions` is where the player
    /// reports its position.
    ///
    /// # Errors
    ///
    /// [`PlayerError::NotInstalled`] when there is no binary, and [`PlayerError::Spawn`] when the
    /// operating system refused to start one.
    fn play(
        &self,
        input: PlayerInput<'_>,
        start_ms: Option<i64>,
        subtitle: Option<&SafePath>,
        audio_language: Option<&str>,
        positions: Arc<dyn PositionSink>,
    ) -> Result<(), PlayerError>;

    /// Stops the player this backend started, if any. Idempotent.
    fn stop(&self);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loopback_url_is_the_only_stream_a_player_may_open() {
        assert!(LoopbackUrl::parse("http://127.0.0.1:43123/media/abc").is_some());
        let refused = [
            "https://127.0.0.1:43123/media/abc",
            "http://127.0.0.1:0/media/abc",
            "http://127.0.0.1/media/abc",
            "http://127.0.0.1:99999/media/abc",
            "http://127.0.0.1:8O/media/abc",
            "http://example.com:80/media/abc",
            "http://127.0.0.1.evil.com:80/media/abc",
            "http://user@127.0.0.1:80/media/abc",
            "http://127.0.0.1:80/",
            "http://127.0.0.1:80/media/abc?x=1",
            "http://127.0.0.1:80/media/abc#frag",
            "file:///etc/passwd",
            " http://127.0.0.1:80/media/abc",
            "http://127.0.0.1:80/media/abc\\def",
        ];
        for value in refused {
            assert!(LoopbackUrl::parse(value).is_none(), "{value}");
        }
    }

    #[test]
    fn a_loopback_url_s_debug_prints_no_part_of_itself() {
        let url = LoopbackUrl::parse("http://127.0.0.1:43123/media/secret-token")
            .expect("a loopback URL");
        let line = format!("{url:?}");
        assert!(!line.contains("secret-token"), "{line}");
        assert!(!line.contains("43123"), "{line}");
    }

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
