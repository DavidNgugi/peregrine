//! Chapter extraction and the name mapping that turns a container's chapters into skip markers.
//!
//! A chapter is the file talking about itself, which is why it outranks every network source. The
//! mapping is deliberately conservative: only a name this module recognises becomes a marker, and
//! an unrecognised chapter is not guessed at (MKR-02).

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use peregrine_domain::{MarkerKind, PlaybackMarker};
use serde_json::{Map, Value};
use tokio::process::Command;
use tokio::time::timeout;

/// How long ffprobe may take before the answer counts as unavailable.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// The names a chapter may carry, longest phrase first so a phrase is not read as its own words.
///
/// This is a whitelist, not a keyword search: a title is split into whole words and a phrase only
/// matches a run of those words, so "Operation" is not "op" and "Editor" is not "ed".
const NAMES: &[(&[&str], MarkerKind)] = &[
    (&["closing", "credits"], MarkerKind::Credits),
    (&["next", "episode"], MarkerKind::Preview),
    (&["previously", "on"], MarkerKind::Recap),
    (&["end", "credits"], MarkerKind::Credits),
    (&["next", "time"], MarkerKind::Preview),
    (&["next", "on"], MarkerKind::Preview),
    (&["previously"], MarkerKind::Recap),
    (&["opening"], MarkerKind::Intro),
    (&["credits"], MarkerKind::Credits),
    (&["preview"], MarkerKind::Preview),
    (&["ending"], MarkerKind::Credits),
    (&["outro"], MarkerKind::Credits),
    (&["intro"], MarkerKind::Intro),
    (&["recap"], MarkerKind::Recap),
    (&["open"], MarkerKind::Intro),
    (&["op"], MarkerKind::Intro),
    (&["ed"], MarkerKind::Credits),
];

/// One chapter, as ffprobe reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chapter {
    /// The chapter's own title.
    pub title: String,
    /// Where it starts, in milliseconds.
    pub start_ms: i64,
    /// Where it ends, in milliseconds.
    pub end_ms: i64,
}

/// Why chapters could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ChapterError {
    /// The probe could not be started or exited without an answer.
    #[error("the chapter probe could not read that file")]
    Probe,
    /// The answer was not the JSON shape this build expects.
    #[error("the chapter probe answered with a shape this build does not recognise")]
    Schema,
}

/// Reads the chapters of one file with ffprobe.
///
/// # Errors
///
/// [`ChapterError`] when the probe fails or answers something unusable.
pub async fn extract(ffprobe: &Path, media: &Path) -> Result<Vec<Chapter>, ChapterError> {
    let mut command = Command::new(ffprobe);
    command
        .arg("-v")
        .arg("error")
        .arg("-show_chapters")
        .arg("-print_format")
        .arg("json")
        .arg("-i")
        .arg(media)
        .stdin(Stdio::null())
        // A probe that timed out must not keep the file open behind the future that owned it.
        .kill_on_drop(true);

    let output = timeout(PROBE_TIMEOUT, command.output())
        .await
        .map_err(|_| ChapterError::Probe)?
        .map_err(|_| ChapterError::Probe)?;
    if !output.status.success() {
        return Err(ChapterError::Probe);
    }
    let answer = String::from_utf8(output.stdout).map_err(|_| ChapterError::Probe)?;
    parse(&answer)
}

/// Parses ffprobe's `-show_chapters -print_format json` answer.
///
/// # Errors
///
/// [`ChapterError::Schema`] when the answer is not the documented shape.
pub fn parse(json: &str) -> Result<Vec<Chapter>, ChapterError> {
    let value: Value = serde_json::from_str(json).map_err(|_| ChapterError::Schema)?;
    let object = value.as_object().ok_or(ChapterError::Schema)?;
    match object.get("chapters") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(entries)) => Ok(entries.iter().filter_map(parsed_chapter).collect()),
        Some(_) => Err(ChapterError::Schema),
    }
}

/// Maps recognised chapter titles to markers, dropping everything this build cannot name.
#[must_use]
pub fn markers(chapters: &[Chapter]) -> Vec<PlaybackMarker> {
    chapters
        .iter()
        .filter_map(|chapter| {
            let kind = kind_of(&chapter.title)?;
            // The constructor is the interval guard: a chapter that ends where it starts, opens
            // before zero, or runs backwards is not a window the player may skip.
            PlaybackMarker::chapter(kind, chapter.start_ms, chapter.end_ms).ok()
        })
        .collect()
}

/// One chapter entry, or `None` when it carries no trustworthy interval.
fn parsed_chapter(value: &Value) -> Option<Chapter> {
    let object = value.as_object()?;
    let start_ms = endpoint(object, "start_time", "start")?;
    let end_ms = endpoint(object, "end_time", "end")?;
    if end_ms <= start_ms {
        return None;
    }
    let title = object
        .get("tags")
        .and_then(Value::as_object)
        .and_then(|tags| tags.get("title"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Some(Chapter {
        title,
        start_ms,
        end_ms,
    })
}

/// One endpoint: the decimal-seconds form first, the time-base ticks as its fallback.
///
/// A key that is present but unreadable is not retried through the other form: ffprobe said
/// something this build cannot trust, and the chapter is skipped rather than placed at a guess.
fn endpoint(object: &Map<String, Value>, seconds_key: &str, ticks_key: &str) -> Option<i64> {
    if let Some(value) = object.get(seconds_key) {
        return value.as_str().and_then(seconds_to_millis);
    }
    let ticks = object.get(ticks_key)?.as_i64()?;
    let (numerator, denominator) = time_base(object)?;
    scaled(ticks, numerator, denominator)
}

/// Decimal seconds to whole milliseconds, rounded to the nearest one.
fn seconds_to_millis(text: &str) -> Option<i64> {
    let seconds: f64 = text.trim().parse().ok()?;
    if !seconds.is_finite() {
        return None;
    }
    let millis = (seconds * 1000.0).round();
    // A timestamp outside i64's range would saturate to a point the file never had.
    if millis < i64::MIN as f64 || millis > i64::MAX as f64 {
        return None;
    }
    Some(millis as i64)
}

/// The `num/den` seconds a tick is worth, in the documented `"1/1000"` form.
fn time_base(object: &Map<String, Value>) -> Option<(i64, i64)> {
    let (numerator, denominator) = object.get("time_base")?.as_str()?.split_once('/')?;
    let numerator: i64 = numerator.trim().parse().ok()?;
    let denominator: i64 = denominator.trim().parse().ok()?;
    (denominator != 0).then_some((numerator, denominator))
}

/// Ticks scaled by `numerator/denominator` into whole milliseconds, rounded to the nearest one.
fn scaled(ticks: i64, numerator: i64, denominator: i64) -> Option<i64> {
    // A negative denominator is a legal rational; normalising it keeps the rounding below in one
    // direction. The denominator is already known to be non-zero, but this function does its own
    // division and must not depend on that.
    let (numerator, denominator) = if denominator < 0 {
        (numerator.checked_neg()?, denominator.checked_neg()?)
    } else {
        (numerator, denominator)
    };
    if denominator == 0 {
        return None;
    }
    let scaled = i128::from(ticks)
        .checked_mul(i128::from(numerator))?
        .checked_mul(1000)?;
    let denominator = i128::from(denominator);
    let half = denominator / 2;
    let millis = if scaled >= 0 {
        scaled.checked_add(half)?.checked_div(denominator)?
    } else {
        scaled.checked_sub(half)?.checked_div(denominator)?
    };
    i64::try_from(millis).ok()
}

/// The kind a title names, if it names one at all.
fn kind_of(title: &str) -> Option<MarkerKind> {
    let lowered = title.to_lowercase();
    let words: Vec<&str> = lowered
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    (0..words.len()).find_map(|at| {
        NAMES.iter().find_map(|(phrase, kind)| {
            (words.get(at..at + phrase.len()) == Some(*phrase)).then_some(*kind)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn title_chapter(title: &str, start_ms: i64, end_ms: i64) -> Chapter {
        Chapter {
            title: title.to_owned(),
            start_ms,
            end_ms,
        }
    }

    /// One valid chapter per title, so the assertion is about the name and nothing else.
    fn kinds(titles: &[&str]) -> Vec<MarkerKind> {
        let chapters: Vec<Chapter> = titles
            .iter()
            .enumerate()
            .map(|(index, title)| {
                let start = i64::try_from(index).expect("a test index fits in i64") * 10_000;
                title_chapter(title, start, start + 5_000)
            })
            .collect();
        markers(&chapters)
            .into_iter()
            .map(PlaybackMarker::kind)
            .collect()
    }

    #[test]
    fn a_documented_probe_answer_parses_into_milliseconds() {
        let json = r#"{"chapters":[
            {"id":0,"time_base":"1/1000","start":0,"start_time":"0.000000","end":60000,"end_time":"60.000000","tags":{"title":"Intro"}},
            {"id":1,"time_base":"1/1000","start":60000,"start_time":"60.000000","end":90000,"end_time":"90.000600","tags":{"title":"End Credits"}}
        ]}"#;

        assert_eq!(
            parse(json).expect("the documented shape parses"),
            vec![
                title_chapter("Intro", 0, 60_000),
                title_chapter("End Credits", 60_000, 90_001),
            ]
        );
    }

    #[test]
    fn the_time_base_scales_ticks_when_the_seconds_form_is_absent() {
        let json = r#"{"chapters":[{"time_base":"1/90000","start":900000,"end":1800000,"tags":{"title":"Recap"}}]}"#;

        assert_eq!(
            parse(json).expect("ticks scale through the time base"),
            vec![title_chapter("Recap", 10_000, 20_000)]
        );
    }

    #[test]
    fn an_unrecognised_name_produces_no_marker() {
        assert_eq!(
            kinds(&[
                "Chapter 1",
                "Introduction",
                "Operation",
                "Editor",
                "Extended Edition",
                "Previews",
                "",
            ]),
            Vec::<MarkerKind>::new()
        );
    }

    #[test]
    fn opening_and_credits_match_only_as_whole_names() {
        assert_eq!(kinds(&["Opening"]), vec![MarkerKind::Intro]);
        assert_eq!(kinds(&["op"]), vec![MarkerKind::Intro]);
        assert_eq!(kinds(&["Credits"]), vec![MarkerKind::Credits]);
        assert_eq!(kinds(&["End Credits"]), vec![MarkerKind::Credits]);

        // "Opening Credits" names the opening, not two segments wearing one chapter title.
        let mapped = markers(&[title_chapter("Opening Credits", 30_000, 90_000)]);
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].kind(), MarkerKind::Intro);
    }

    #[test]
    fn no_chapters_and_malformed_answers_are_distinguished() {
        assert_eq!(
            parse("{}").expect("an absent array is no chapters"),
            Vec::<Chapter>::new()
        );
        assert_eq!(
            parse(r#"{"chapters":[]}"#).expect("an empty array is no chapters"),
            Vec::<Chapter>::new()
        );

        assert_eq!(parse("not json"), Err(ChapterError::Schema));
        assert_eq!(parse("[]"), Err(ChapterError::Schema));
        assert_eq!(parse(r#"{"chapters":{}}"#), Err(ChapterError::Schema));
        assert_eq!(parse(""), Err(ChapterError::Schema));

        assert_eq!(
            parse(r#"{"chapters":[{"tags":{"title":"Intro"}}]}"#)
                .expect("an untimed chapter is skipped"),
            Vec::<Chapter>::new()
        );
        assert_eq!(
            parse(r#"{"chapters":[42]}"#).expect("a non-object chapter is skipped"),
            Vec::<Chapter>::new()
        );
        assert_eq!(
            parse(
                r#"{"chapters":[{"start_time":"oops","start":0,"time_base":"1/1000","end_time":"10.000000"}]}"#
            )
            .expect("an unreadable time is not retried through the ticks"),
            Vec::<Chapter>::new()
        );
    }

    #[test]
    fn an_invalid_interval_is_dropped() {
        let json = r#"{"chapters":[
            {"start_time":"10.000000","end_time":"10.000000","tags":{"title":"Intro"}},
            {"start_time":"20.000000","end_time":"15.000000","tags":{"title":"Recap"}},
            {"start_time":"30.000000","end_time":"40.000000","tags":{"title":"End Credits"}}
        ]}"#;

        assert_eq!(
            parse(json).expect("the answer parses"),
            vec![title_chapter("End Credits", 30_000, 40_000)]
        );

        assert_eq!(
            markers(&[title_chapter("Intro", 5_000, 5_000)]),
            Vec::<PlaybackMarker>::new()
        );
        assert_eq!(
            markers(&[title_chapter("Intro", -1, 5_000)]),
            Vec::<PlaybackMarker>::new()
        );
        assert_eq!(markers(&[]), Vec::<PlaybackMarker>::new());
    }

    #[test]
    fn every_documented_name_maps_to_its_kind() {
        for (titles, kind) in [
            (&["intro", "opening", "op", "open"][..], MarkerKind::Intro),
            (
                &["recap", "previously on", "previously"][..],
                MarkerKind::Recap,
            ),
            (
                &[
                    "credits",
                    "end credits",
                    "closing credits",
                    "ending",
                    "outro",
                    "ed",
                ][..],
                MarkerKind::Credits,
            ),
            (
                &["preview", "next episode", "next time", "next on"][..],
                MarkerKind::Preview,
            ),
        ] {
            for &title in titles {
                assert_eq!(kinds(&[title]), vec![kind], "{title}");
            }
        }
    }

    #[test]
    fn names_are_read_after_lowercasing_and_splitting_punctuation() {
        assert_eq!(kinds(&["  OPENING  "]), vec![MarkerKind::Intro]);
        assert_eq!(kinds(&["End-Credits!"]), vec![MarkerKind::Credits]);
        assert_eq!(kinds(&["Next_time?"]), vec![MarkerKind::Preview]);
    }

    #[test]
    fn markers_keep_the_order_the_container_declares() {
        let chapters = [
            title_chapter("End Credits", 100_000, 140_000),
            title_chapter("Intro", 10_000, 40_000),
        ];

        assert_eq!(
            markers(&chapters)
                .into_iter()
                .map(PlaybackMarker::kind)
                .collect::<Vec<_>>(),
            vec![MarkerKind::Credits, MarkerKind::Intro]
        );
    }

    #[tokio::test]
    async fn a_missing_probe_is_not_an_empty_answer() {
        let error = extract(
            Path::new("/nonexistent/peregrine-ffprobe"),
            Path::new("film.mkv"),
        )
        .await
        .expect_err("no such binary");

        assert_eq!(error, ChapterError::Probe);
    }
}
