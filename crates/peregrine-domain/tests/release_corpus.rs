//! Measures the release-name parser against the committed corpus.
//!
//! The corpus in `data/release_names.json` is ours: every name is real-shaped
//! and every expected parse was written from the canonical token list in
//! `docs/research/data-sources.md` §3.5, not read back out of the parser. The
//! 95% floor in `PG-E4-03` is therefore measured against our own data.

use peregrine_domain::release::{
    AudioCodec, EpisodeSpan, HdrFormat, ReleaseName, ReleaseSource, RepackKind, Resolution,
    VideoCodec,
};
use serde::Deserialize;

const CORPUS: &str = include_str!("data/release_names.json");
const REQUIRED_ACCURACY: f64 = 0.95;
const MINIMUM_CORPUS: usize = 200;

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    resolution: Option<String>,
    source: Option<String>,
    video_codec: Option<String>,
    hdr: Option<String>,
    #[serde(default)]
    audio: Vec<String>,
    group: Option<String>,
    repack: Option<String>,
    season: Option<u32>,
    #[serde(default)]
    episodes: Option<[u32; 2]>,
    season_pack: bool,
}

fn compare(case: &Case, parsed: &ReleaseName) -> Result<(), String> {
    let mut problems = Vec::new();
    let mut check = |field: &str, expected: String, actual: String| {
        if expected != actual {
            problems.push(format!("{field}: expected `{expected}`, parsed `{actual}`"));
        }
    };
    check(
        "resolution",
        case.resolution.clone().unwrap_or_default(),
        parsed
            .resolution()
            .map(Resolution::as_str)
            .unwrap_or_default()
            .to_owned(),
    );
    check(
        "source",
        case.source.clone().unwrap_or_default(),
        parsed
            .source()
            .map(ReleaseSource::as_str)
            .unwrap_or_default()
            .to_owned(),
    );
    check(
        "video_codec",
        case.video_codec.clone().unwrap_or_default(),
        parsed
            .video_codec()
            .map(VideoCodec::as_str)
            .unwrap_or_default()
            .to_owned(),
    );
    check(
        "hdr",
        case.hdr.clone().unwrap_or_default(),
        parsed
            .hdr()
            .map(HdrFormat::as_str)
            .unwrap_or_default()
            .to_owned(),
    );
    let expected_audio: Vec<&str> = case.audio.iter().map(String::as_str).collect();
    let actual_audio: Vec<&str> = parsed.audio().iter().map(|codec| codec.as_str()).collect();
    check("audio", expected_audio.join("+"), actual_audio.join("+"));
    check(
        "group",
        case.group.clone().unwrap_or_default(),
        parsed.group().unwrap_or_default().to_owned(),
    );
    check(
        "repack",
        case.repack.clone().unwrap_or_default(),
        parsed
            .repack()
            .map(|kind| kind.to_string())
            .unwrap_or_default(),
    );
    check(
        "season",
        case.season
            .map_or_else(String::new, |season| season.to_string()),
        parsed
            .season()
            .map_or_else(String::new, |season| season.to_string()),
    );
    let expected_span = case
        .episodes
        .map_or_else(String::new, |[first, last]| format!("{first}-{last}"));
    let actual_span = parsed.episodes().map_or_else(String::new, |span| {
        format!("{}-{}", span.first(), span.last())
    });
    check("episodes", expected_span, actual_span);
    check(
        "season_pack",
        case.season_pack.to_string(),
        parsed.is_season_pack().to_string(),
    );
    if parsed.raw() != case.name {
        problems.push("raw: input text was not preserved".to_owned());
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("; "))
    }
}

#[test]
fn every_token_maps_to_its_documented_string() {
    let resolution: Vec<&str> = Resolution::ALL
        .into_iter()
        .map(Resolution::as_str)
        .collect();
    assert_eq!(
        resolution,
        ["2160p", "1440p", "1080p", "1080i", "720p", "576p", "480p"]
    );
    let hdr: Vec<&str> = HdrFormat::ALL.into_iter().map(HdrFormat::as_str).collect();
    assert_eq!(hdr, ["sdr", "hdr10", "hdr10_plus", "hlg", "dolby_vision"]);
    assert_eq!(ReleaseSource::WebDl.as_str(), "web-dl");
    assert_eq!(VideoCodec::H265.as_str(), "h265");
    assert_eq!(AudioCodec::TrueHd.as_str(), "truehd");
    assert_eq!(RepackKind::Version(2).to_string(), "v2");
    assert_eq!(EpisodeSpan::new(9, 3).first(), 3);
    assert_eq!(EpisodeSpan::new(9, 3).last(), 9);
    assert!(EpisodeSpan::new(9, 3).is_multi());
}

#[test]
fn corpus_meets_the_accuracy_floor() {
    let cases: Vec<Case> = serde_json::from_str(CORPUS).expect("corpus is valid JSON");
    assert!(
        cases.len() >= MINIMUM_CORPUS,
        "corpus has {} names, expected at least {MINIMUM_CORPUS}",
        cases.len()
    );

    let mut correct = 0_usize;
    let mut failures = Vec::new();
    for case in &cases {
        let parsed = ReleaseName::parse(&case.name);
        match compare(case, &parsed) {
            Ok(()) => correct += 1,
            Err(problems) => failures.push(format!("{}: {problems}", case.name)),
        }
    }

    let total = cases.len();
    let accuracy = correct as f64 / total as f64;
    println!(
        "release corpus: {correct}/{total} = {:.1}% (floor {:.0}%)",
        accuracy * 100.0,
        REQUIRED_ACCURACY * 100.0
    );
    assert!(
        accuracy >= REQUIRED_ACCURACY,
        "accuracy {accuracy:.3} is below {REQUIRED_ACCURACY}; {} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
