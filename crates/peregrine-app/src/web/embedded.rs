//! Text subtitles carried inside the media file, offered to the browser beside the sidecars.
//!
//! A Matroska release usually carries its subtitles as streams rather than as `.srt` files next to
//! it, and the browser cannot read a stream out of a container it is not demuxing — nor out of a
//! transcode that maps only video and audio. The mint lists the file's text streams; the subtitle
//! route asks ffmpeg for one of them as `WebVTT` and retimes it like any sidecar. Bitmap streams
//! (PGS, `VobSub`, DVB) are not listed: they would need burning in (PG-E17-08).

use std::path::Path;
use std::process::Stdio;

use tokio::process::Command;
use tokio::time::{Duration, timeout};

use super::transcode;

/// How long one extraction may take. ffmpeg has to demux the whole file to collect a stream's
/// packets, which is I/O-bound and seconds for a local film, but a stalled disk must not hold a
/// request open forever.
const EXTRACT_TIMEOUT: Duration = Duration::from_secs(120);

/// The text codecs ffmpeg can turn into `WebVTT`.
const TEXT_CODECS: &[&str] = &["subrip", "srt", "ass", "ssa", "webvtt", "mov_text", "text"];

/// One text subtitle stream inside the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedSubtitle {
    /// The stream's position among the file's subtitle streams, which is what `0:s:n` selects.
    pub ordinal: usize,
    pub language: String,
    pub title: Option<String>,
    pub codec: String,
    pub forced: bool,
    pub sdh: bool,
    pub is_default: bool,
}

/// The track id an embedded stream is offered under.
///
/// Sidecar ids are positive storage row ids, so a stream takes the negative space: `-(n + 1)` for
/// ordinal `n`. The id never reaches storage; it is a name the subtitle route reverses.
#[must_use]
pub fn track_id(ordinal: usize) -> i64 {
    -(i64::try_from(ordinal).unwrap_or(i64::MAX - 1) + 1)
}

/// The ordinal an embedded track id names, or `None` for a sidecar's id.
#[must_use]
pub fn ordinal_of(track_id: i64) -> Option<usize> {
    if track_id >= 0 {
        return None;
    }
    usize::try_from(-(track_id + 1)).ok()
}

/// The text streams in ffprobe's JSON for `-select_streams s`.
///
/// The ordinal counts every subtitle stream, bitmap ones included, because `0:s:n` does: skipping
/// a PGS stream must not shift the text stream after it onto the wrong index. A stream without a
/// language tag is `und`, which is what the preference matcher already treats as unknown.
#[must_use]
pub fn text_streams(probe_json: &str) -> Vec<EmbeddedSubtitle> {
    let Ok(probe) = serde_json::from_str::<serde_json::Value>(probe_json) else {
        return Vec::new();
    };
    let Some(streams) = probe.get("streams").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    let flag = |stream: &serde_json::Value, name: &str| {
        stream
            .get("disposition")
            .and_then(|disposition| disposition.get(name))
            .and_then(serde_json::Value::as_i64)
            .is_some_and(|value| value != 0)
    };
    let tag = |stream: &serde_json::Value, name: &str| {
        stream
            .get("tags")
            .and_then(|tags| tags.get(name))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    streams
        .iter()
        .enumerate()
        .filter_map(|(ordinal, stream)| {
            let codec = stream.get("codec_name")?.as_str()?.to_ascii_lowercase();
            if !TEXT_CODECS.contains(&codec.as_str()) {
                return None;
            }
            let title = tag(stream, "title");
            // Many releases state forced and SDH in the stream title rather than the disposition.
            let titled = |word: &str| {
                title
                    .as_deref()
                    .is_some_and(|title| title.to_ascii_lowercase().contains(word))
            };
            let sdh = flag(stream, "hearing_impaired") || titled("sdh");
            let forced = flag(stream, "forced") || titled("forced");
            Some(EmbeddedSubtitle {
                ordinal,
                language: tag(stream, "language").unwrap_or_else(|| "und".to_owned()),
                title,
                codec,
                forced,
                sdh,
                is_default: flag(stream, "default"),
            })
        })
        .collect()
}

/// The file's text subtitle streams, or none when ffprobe is missing or refuses the file.
pub async fn probe(path: &Path) -> Vec<EmbeddedSubtitle> {
    let output = Command::new(transcode::locate_probe())
        .args([
            "-v",
            "error",
            "-select_streams",
            "s",
            "-show_entries",
            "stream=index,codec_name:stream_tags=language,title:stream_disposition=default,forced,hearing_impaired",
            "-of",
            "json",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await;
    match output {
        Ok(output) if output.status.success() => {
            text_streams(&String::from_utf8_lossy(&output.stdout))
        }
        _ => Vec::new(),
    }
}

/// One stream as a `WebVTT` document, or `None` when ffmpeg could not produce it in time.
pub async fn extract(path: &Path, ordinal: usize) -> Option<String> {
    let map = format!("0:s:{ordinal}");
    let run = Command::new(transcode::locate())
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args(["-map", map.as_str(), "-f", "webvtt", "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = timeout(EXTRACT_TIMEOUT, run).await.ok()?.ok()?;
    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::{EmbeddedSubtitle, ordinal_of, text_streams, track_id};

    #[test]
    fn a_track_id_round_trips_and_never_collides_with_a_row_id() {
        for ordinal in [0, 1, 7, 99] {
            let id = track_id(ordinal);
            assert!(id < 0, "{id}");
            assert_eq!(ordinal_of(id), Some(ordinal));
        }
        assert_eq!(track_id(0), -1);
        assert_eq!(ordinal_of(0), None);
        assert_eq!(ordinal_of(42), None);
    }

    #[test]
    fn text_streams_keep_the_absolute_subtitle_ordinal_past_a_bitmap_stream() {
        let probe = r#"{"streams":[
            {"index":2,"codec_name":"hdmv_pgs_subtitle","tags":{"language":"eng"}},
            {"index":3,"codec_name":"subrip","tags":{"language":"eng","title":"English SDH"},
             "disposition":{"default":1,"forced":0,"hearing_impaired":0}},
            {"index":4,"codec_name":"ass","tags":{"language":"jpn"},
             "disposition":{"default":0,"forced":1,"hearing_impaired":0}},
            {"index":5,"codec_name":"dvd_subtitle"}
        ]}"#;
        assert_eq!(
            text_streams(probe),
            vec![
                EmbeddedSubtitle {
                    ordinal: 1,
                    language: "eng".to_owned(),
                    title: Some("English SDH".to_owned()),
                    codec: "subrip".to_owned(),
                    forced: false,
                    sdh: true,
                    is_default: true,
                },
                EmbeddedSubtitle {
                    ordinal: 2,
                    language: "jpn".to_owned(),
                    title: None,
                    codec: "ass".to_owned(),
                    forced: true,
                    sdh: false,
                    is_default: false,
                },
            ]
        );
    }

    #[test]
    fn an_untagged_stream_is_und_and_the_hearing_impaired_flag_marks_sdh() {
        let probe = r#"{"streams":[{"codec_name":"mov_text","tags":{"title":"  "},
            "disposition":{"hearing_impaired":1}}]}"#;
        let streams = text_streams(probe);
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].language, "und");
        assert_eq!(streams[0].title, None);
        assert!(streams[0].sdh);
        assert!(!streams[0].forced);
    }

    #[test]
    fn a_stream_titled_forced_is_forced_without_the_disposition() {
        let probe = r#"{"streams":[{"codec_name":"subrip","tags":{"language":"ita","title":"Forced"},
            "disposition":{"default":0,"forced":0,"hearing_impaired":0}}]}"#;
        let streams = text_streams(probe);
        assert!(streams[0].forced);
        assert!(!streams[0].sdh);
    }

    #[test]
    fn junk_and_an_empty_file_offer_nothing() {
        for junk in [
            "",
            "not json",
            "{}",
            r#"{"streams":"x"}"#,
            r#"{"streams":[{"index":1}]}"#,
        ] {
            assert_eq!(text_streams(junk), Vec::new(), "{junk:?}");
        }
        assert_eq!(text_streams(r#"{"streams":[]}"#), Vec::new());
    }
}
