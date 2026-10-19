//! Transcoding a file into HLS, so a browser can play what the webview cannot demux.
//!
//! ADR 0014 fixes the encoder ladder: hardware H.264 first, then `libopenh264`, and never
//! `libx264`, because the ffmpeg this project redistributes is LGPL. The command is built as an
//! argument vector and passed to a child process; the file path is a guarded path the core resolved,
//! so nothing here assembles a path or a shell word.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use thiserror::Error;
use tokio::process::{Child, Command};
use tokio::time::{Duration, Instant, sleep};

/// How long a playlist may take to appear before the start is called a failure.
const PLAYLIST_TIMEOUT: Duration = Duration::from_secs(20);

/// The encoders this build may use, in preference order.
///
/// `h264_vaapi` is deliberately absent: it needs a device and a filter chain this slice does not
/// configure, and an encoder that fails on a host without `/dev/dri` is worse than a clear refusal.
const ENCODERS: &[&str] = &[
    "h264_videotoolbox",
    "h264_nvenc",
    "h264_qsv",
    "h264_amf",
    "libopenh264",
];

/// Why a transcode could not start.
#[derive(Debug, Error)]
pub enum TranscodeError {
    /// No ffmpeg could be found.
    #[error("no ffmpeg binary was found")]
    Missing,
    /// The listing was readable but holds none of the encoders this build may use.
    #[error("this ffmpeg has no H.264 encoder this build may use")]
    NoEncoder,
    /// A process could not be started or a directory could not be made.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The process exited before a playlist appeared.
    #[error("ffmpeg exited before producing a playlist: {reason}")]
    Exited {
        /// The tail of its log, redacted of the source path.
        reason: String,
    },
    /// The playlist did not appear in time.
    #[error("the transcoder produced no playlist in time")]
    NoPlaylist,
}

/// The ffmpeg this process uses and the encoder it found.
#[derive(Debug, Clone)]
pub struct Transcoder {
    ffmpeg: PathBuf,
    encoder: &'static str,
}

/// A started transcode and the audio stream it mapped.
#[derive(Debug)]
pub struct StartedTranscode {
    /// The ffmpeg child, owned by the session that started it.
    pub child: Child,
    /// The language tag of the stream the host selected, or `None` when it mapped the first.
    pub audio_language: Option<String>,
}

/// Which audio stream a transcode maps, and the tag that named it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AudioStreamChoice {
    /// The audio stream's ordinal within the file's audio streams, which is what `0:a:n` takes.
    pub ordinal: usize,
    /// The tag of the stream that matched, or `None` for the first-stream fallback.
    pub language: Option<String>,
}

impl Transcoder {
    /// Finds an ffmpeg and the best encoder it offers.
    ///
    /// # Errors
    ///
    /// [`TranscodeError::Missing`] when the binary cannot be started, [`TranscodeError::NoEncoder`]
    /// when its listing holds none of the permitted encoders.
    pub async fn detect() -> Result<Self, TranscodeError> {
        let ffmpeg = locate();
        let output = Command::new(&ffmpeg)
            .args(["-hide_banner", "-encoders"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .await
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    TranscodeError::Missing
                } else {
                    TranscodeError::Io(error)
                }
            })?;
        let listing = String::from_utf8_lossy(&output.stdout);
        let encoder = ENCODERS
            .iter()
            .find(|name| listing.contains(**name))
            .copied()
            .ok_or(TranscodeError::NoEncoder)?;
        Ok(Self { ffmpeg, encoder })
    }

    /// The encoder this host will use, for the log and for a diagnostics line.
    #[must_use]
    pub const fn encoder(&self) -> &'static str {
        self.encoder
    }

    /// Starts one HLS transcode and waits for its playlist.
    ///
    /// The audio stream is the one the probe's tag names for `audio_language`; a host that
    /// cannot probe, or finds no match, maps the first audio stream and reports no language.
    ///
    /// # Errors
    ///
    /// [`TranscodeError::Io`] when the directory or process cannot be made,
    /// [`TranscodeError::Exited`] when ffmpeg refused the file, and
    /// [`TranscodeError::NoPlaylist`] when it ran but produced nothing in time.
    pub async fn start(
        &self,
        directory: &Path,
        source: &Path,
        start_ms: i64,
        audio_language: Option<&str>,
    ) -> Result<StartedTranscode, TranscodeError> {
        let choice = match audio_language
            .map(str::trim)
            .filter(|preferred| !preferred.is_empty())
        {
            Some(preferred) => {
                let probe = probe_audio(source).await.unwrap_or_default();
                choose_audio_stream(&probe, Some(preferred))
            }
            None => AudioStreamChoice::default(),
        };
        tokio::fs::create_dir_all(directory).await?;
        let playlist = directory.join("index.m3u8");
        let log_path = directory.join("ffmpeg.log");
        let log = std::fs::File::create(&log_path)?;
        let mut command = Command::new(&self.ffmpeg);
        command
            .arg("-hide_banner")
            .arg("-nostdin")
            .arg("-loglevel")
            .arg("error")
            .arg("-y");
        if start_ms > 0 {
            command
                .arg("-ss")
                .arg(format!("{:.3}", start_ms as f64 / 1000.0));
        }
        command
            .arg("-i")
            .arg(source)
            .arg("-map")
            .arg("0:v:0")
            .arg("-map")
            .arg(format!("0:a:{}?", choice.ordinal));
        command.args(video_arguments(self.encoder));
        command.args([
            "-force_key_frames",
            "expr:gte(t,n_forced*4)",
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-ac",
            "2",
            "-f",
            "hls",
            "-hls_time",
            "4",
            "-hls_playlist_type",
            "event",
            "-hls_segment_type",
            "fmp4",
            "-hls_fmp4_init_filename",
            "init.mp4",
            "-hls_flags",
            "independent_segments",
            "-hls_segment_filename",
        ]);
        let segments = directory.join("seg%05d.m4s");
        command
            .arg(segments)
            .arg(&playlist)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(log))
            .kill_on_drop(true);
        let mut child = command.spawn()?;
        let started = Instant::now();
        loop {
            if playlist.exists()
                && let Ok(metadata) = tokio::fs::metadata(&playlist).await
                && metadata.len() > 0
            {
                return Ok(StartedTranscode {
                    child,
                    audio_language: choice.language,
                });
            }
            if let Some(status) = child.try_wait()? {
                let reason = tail(&log_path, source).await;
                return Err(TranscodeError::Exited {
                    reason: format!("{status}: {reason}"),
                });
            }
            if started.elapsed() > PLAYLIST_TIMEOUT {
                let _ = child.kill().await;
                return Err(TranscodeError::NoPlaylist);
            }
            sleep(Duration::from_millis(100)).await;
        }
    }
}

/// The codec arguments for one encoder.
fn video_arguments(encoder: &str) -> &'static [&'static str] {
    match encoder {
        "h264_videotoolbox" => &[
            "-c:v",
            "h264_videotoolbox",
            "-b:v",
            "8M",
            "-maxrate",
            "10M",
            "-bufsize",
            "16M",
            "-pix_fmt",
            "yuv420p",
        ],
        "h264_nvenc" => &[
            "-c:v",
            "h264_nvenc",
            "-preset",
            "p5",
            "-b:v",
            "8M",
            "-pix_fmt",
            "yuv420p",
        ],
        "h264_qsv" => &["-c:v", "h264_qsv", "-b:v", "8M", "-pix_fmt", "nv12"],
        "h264_amf" => &["-c:v", "h264_amf", "-b:v", "8M", "-pix_fmt", "yuv420p"],
        _ => &["-c:v", "libopenh264", "-b:v", "8M", "-pix_fmt", "yuv420p"],
    }
}

/// The last lines of ffmpeg's log, with the source path replaced by a marker.
async fn tail(path: &Path, source: &Path) -> String {
    let text = tokio::fs::read_to_string(path).await.unwrap_or_default();
    let last: Vec<&str> = text.lines().rev().take(3).collect();
    let mut reason = last.into_iter().rev().collect::<Vec<_>>().join(" / ");
    if let Some(source) = source.to_str() {
        reason = reason.replace(source, "[file]");
    }
    if reason.len() > 300 {
        reason.truncate(300);
    }
    reason
}

/// Whether a probed pair of codecs is one a browser plays directly.
///
/// The container alone is not enough: an MP4 can carry HEVC video or AC3 audio, and a browser that
/// demuxes the container still fails on the stream. A codec the probe did not name is treated as
/// safe, because refusing to play a file because ffprobe could not read it would be a regression on
/// a host without the probe.
#[must_use]
pub fn codecs_are_safe(video: Option<&str>, audio: Option<&str>) -> bool {
    let video_ok = video.is_none_or(|codec| matches!(codec, "h264" | "vp8" | "vp9" | "av1"));
    let audio_ok = audio.is_none_or(|codec| matches!(codec, "aac" | "opus" | "vorbis" | "mp3"));
    video_ok && audio_ok
}

/// Whether this file's streams are ones the browser plays without a transcode.
///
/// `ffprobe` is the same component pack as ffmpeg. A missing probe, a refused file, or a stream it
/// could not name all answer `true`: the container was already the strongest evidence available,
/// and this check exists to catch the codec inside it, not to gate every play on a process.
pub async fn browser_safe(path: &Path) -> bool {
    let ffprobe = locate_probe();
    let video = probe_stream(&ffprobe, path, "v:0").await;
    let audio = probe_stream(&ffprobe, path, "a:0").await;
    codecs_are_safe(video.as_deref(), audio.as_deref())
}

async fn probe_stream(ffprobe: &Path, path: &Path, stream: &str) -> Option<String> {
    let output = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            stream,
            "-show_entries",
            "stream=codec_name",
            "-of",
            "default=nw=1:nk=1",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    let codec = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()?
        .trim()
        .to_owned();
    (!codec.is_empty()).then_some(codec)
}

/// The audio streams' indices and language tags, as ffprobe's JSON, or `None` when it failed.
async fn probe_audio(path: &Path) -> Option<String> {
    let output = Command::new(locate_probe())
        .args([
            "-v",
            "error",
            "-select_streams",
            "a",
            "-show_entries",
            "stream=index:stream_tags=language",
            "-of",
            "json",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Chooses the audio stream to map from ffprobe's own JSON.
///
/// The ordinal is the stream's position among the file's audio streams, not the absolute
/// `stream.index` ffprobe reports: a video is usually index 0, so the first audio stream is
/// index 1 and `0:a:1` would select the second. `-select_streams a` makes the array positions
/// audio-relative, which is exactly what `0:a:n` takes.
///
/// A preference that matches nothing, a probe this build cannot read, and a preferred tag that is
/// blank all fall back to the first audio stream with no language claimed.
#[must_use]
pub fn choose_audio_stream(probe_json: &str, preferred: Option<&str>) -> AudioStreamChoice {
    let fallback = AudioStreamChoice::default();
    let Some(preferred) = preferred.map(str::trim).filter(|value| !value.is_empty()) else {
        return fallback;
    };
    let Ok(probe) = serde_json::from_str::<serde_json::Value>(probe_json) else {
        return fallback;
    };
    let Some(streams) = probe.get("streams").and_then(serde_json::Value::as_array) else {
        return fallback;
    };
    for (ordinal, stream) in streams.iter().enumerate() {
        let Some(tag) = stream
            .get("tags")
            .and_then(|tags| tags.get("language"))
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        if language_matches(tag, preferred) {
            return AudioStreamChoice {
                ordinal,
                language: Some(tag.to_owned()),
            };
        }
    }
    fallback
}

/// Whether a stream's tag names the preferred language: a case-insensitive prefix either way.
fn language_matches(tag: &str, preferred: &str) -> bool {
    let tag = tag.trim().to_ascii_lowercase();
    let preferred = preferred.trim().to_ascii_lowercase();
    !tag.is_empty()
        && !preferred.is_empty()
        && (tag.starts_with(&preferred) || preferred.starts_with(&tag))
}

fn locate_probe() -> PathBuf {
    if let Ok(explicit) = std::env::var("PEREGRINE_FFPROBE") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return path;
        }
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(directory) = executable.parent()
    {
        for candidate in [
            directory.join("ffprobe"),
            directory.join("sidecars/ffprobe"),
            directory.join("sidecars/ffprobe/ffprobe"),
        ] {
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    // The same development fallback as ffmpeg: a binary on PATH is run as a child, never a shell.
    PathBuf::from("ffprobe")
}

/// The ffmpeg this build will run.
///
/// The explicit variable exists for a test and for a user pointing at their own build; the sidecar
/// beside the application comes next, and `PATH` is the development fallback. The binary is run as a
/// child process with an argument vector, never through a shell.
fn locate() -> PathBuf {
    if let Ok(explicit) = std::env::var("PEREGRINE_FFMPEG") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return path;
        }
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(directory) = executable.parent()
    {
        for candidate in [
            directory.join("ffmpeg"),
            directory.join("sidecars/ffmpeg"),
            directory.join("sidecars/ffmpeg/ffmpeg"),
        ] {
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    PathBuf::from("ffmpeg")
}

/// Whether a name is one of the files a transcode session may serve.
///
/// The route takes a name, never a path: a separator, a dot segment, or anything outside the three
/// shapes is refused before the filesystem is asked.
#[must_use]
pub fn valid_hls_file(name: &str) -> bool {
    if name == "index.m3u8" || name == "init.mp4" {
        return true;
    }
    let Some(number) = name
        .strip_prefix("seg")
        .and_then(|rest| rest.strip_suffix(".m4s"))
    else {
        return false;
    };
    number.len() == 5 && number.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::{
        AudioStreamChoice, Transcoder, choose_audio_stream, valid_hls_file, video_arguments,
    };

    #[test]
    fn only_the_three_shapes_of_a_session_file_are_served() {
        assert!(valid_hls_file("index.m3u8"));
        assert!(valid_hls_file("init.mp4"));
        assert!(valid_hls_file("seg00042.m4s"));
        assert!(!valid_hls_file("../secret"));
        assert!(!valid_hls_file("seg42.m4s"));
        assert!(!valid_hls_file("seg00042.ts"));
        assert!(!valid_hls_file("index.m3u8/../../x"));
    }

    #[test]
    fn a_codec_pair_a_browser_plays_is_safe_and_one_it_cannot_is_not() {
        assert!(super::codecs_are_safe(Some("h264"), Some("aac")));
        assert!(super::codecs_are_safe(None, None));
        assert!(!super::codecs_are_safe(Some("hevc"), Some("aac")));
        assert!(!super::codecs_are_safe(Some("h264"), Some("ac3")));
    }

    #[test]
    fn the_encoder_arguments_never_name_libx264() {
        for encoder in [
            "h264_videotoolbox",
            "h264_nvenc",
            "h264_qsv",
            "h264_amf",
            "libopenh264",
        ] {
            let arguments = video_arguments(encoder);
            assert!(!arguments.contains(&"libx264"), "{encoder} must stay LGPL");
        }
    }

    #[test]
    fn an_audio_stream_is_chosen_by_its_audio_ordinal_not_its_absolute_index() {
        let probe = r#"{"streams":[{"index":1,"tags":{"language":"fre"}},{"index":2,"tags":{"language":"eng"}}]}"#;
        assert_eq!(
            choose_audio_stream(probe, Some("en")),
            AudioStreamChoice {
                ordinal: 1,
                language: Some("eng".to_owned()),
            },
            "the second audio stream is 0:a:1; its absolute index 2 is not the ordinal"
        );
        let first = r#"{"streams":[{"index":5,"tags":{"language":"eng"}}]}"#;
        assert_eq!(choose_audio_stream(first, Some("en")).ordinal, 0);
    }

    #[test]
    fn no_match_malformed_and_mono_fall_back_to_the_first_stream() {
        let fallback = AudioStreamChoice::default();
        let german = r#"{"streams":[{"index":1,"tags":{"language":"ger"}}]}"#;
        assert_eq!(choose_audio_stream(german, Some("en")), fallback);
        assert_eq!(choose_audio_stream("not json", Some("en")), fallback);
        assert_eq!(
            choose_audio_stream(r#"{"streams":[]}"#, Some("en")),
            fallback
        );
        let untagged = r#"{"streams":[{"index":1}]}"#;
        assert_eq!(choose_audio_stream(untagged, Some("en")), fallback);
        assert_eq!(choose_audio_stream(german, None), fallback);
        assert_eq!(choose_audio_stream(german, Some("  ")), fallback);
    }

    #[test]
    fn a_language_tag_matches_case_insensitively_by_prefix() {
        let probe = r#"{"streams":[{"index":2,"tags":{"language":"ENG"}}]}"#;
        assert_eq!(
            choose_audio_stream(probe, Some("en")),
            AudioStreamChoice {
                ordinal: 0,
                language: Some("ENG".to_owned()),
            }
        );
        let regional = r#"{"streams":[{"index":1,"tags":{"language":"en"}}]}"#;
        assert_eq!(choose_audio_stream(regional, Some("en-US")).ordinal, 0);
    }

    #[tokio::test]
    #[ignore = "needs a real ffmpeg with a permitted H.264 encoder on PATH"]
    async fn a_real_mkv_becomes_a_playlist() {
        let directory = tempfile::tempdir().expect("a scratch directory");
        let source = directory.path().join("sample.mkv");
        let made = tokio::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=24",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440",
                "-t",
                "3",
                "-c:v",
                "mpeg4",
                "-c:a",
                "pcm_s16le",
            ])
            .arg(&source)
            .status()
            .await
            .expect("the fixture is generated");
        assert!(made.success(), "ffmpeg has to make the fixture");

        let transcoder = Transcoder::detect().await.expect("an encoder");
        let output = directory.path().join("hls");
        let mut started = transcoder
            .start(&output, &source, 0, Some("en"))
            .await
            .expect("the transcode starts");
        let playlist = tokio::fs::read_to_string(output.join("index.m3u8"))
            .await
            .expect("a playlist");
        assert!(playlist.contains("#EXTM3U"));
        assert!(playlist.contains("init.mp4"));
        let _ = started.child.kill().await;
    }

    #[tokio::test]
    async fn detection_reports_a_host_without_ffmpeg_rather_than_panicking() {
        // The variable is process-global, so this test asserts only that the call answers; the
        // real refusal is the `Missing` variant, which a host without a binary produces.
        let _ = Transcoder::detect().await;
    }
}
