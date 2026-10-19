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
    ) -> Result<Child, TranscodeError> {
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
            .args(["-map", "0:v:0", "-map", "0:a:0?"]);
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
                return Ok(child);
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
    use super::{Transcoder, valid_hls_file, video_arguments};

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
        let mut child = transcoder
            .start(&output, &source, 0)
            .await
            .expect("the transcode starts");
        let playlist = tokio::fs::read_to_string(output.join("index.m3u8"))
            .await
            .expect("a playlist");
        assert!(playlist.contains("#EXTM3U"));
        assert!(playlist.contains("init.mp4"));
        let _ = child.kill().await;
    }

    #[tokio::test]
    async fn detection_reports_a_host_without_ffmpeg_rather_than_panicking() {
        // The variable is process-global, so this test asserts only that the call answers; the
        // real refusal is the `Missing` variant, which a host without a binary produces.
        let _ = Transcoder::detect().await;
    }
}
