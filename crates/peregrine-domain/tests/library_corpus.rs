//! Measures the library-name parser against the committed corpus.
//!
//! The corpus in `data/library_names.json` is ours: every name is real-shaped, and every
//! expected parse was written from the naming conventions `name.rs` documents rather than read
//! back out of the parser. A name the parser must refuse carries the `unresolved` reason it owes,
//! so the corpus measures the refusals and the fields found alongside them, not only the parses.
//! The 95% floor in `PG-E4-04` is therefore measured against our own data.

use peregrine_domain::name::{LibraryName, NameError, Numbering, UnresolvedReason};
use peregrine_domain::release::{HdrFormat, ReleaseName, ReleaseSource, Resolution, VideoCodec};
use serde::Deserialize;

const CORPUS: &str = include_str!("data/library_names.json");
const REQUIRED_ACCURACY: f64 = 0.95;
const MINIMUM_CORPUS: usize = 200;

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    title: Option<String>,
    year: Option<u16>,
    season: Option<u32>,
    #[serde(default)]
    episodes: Option<[u32; 2]>,
    absolute: Option<u32>,
    numbering: Option<String>,
    extension: Option<String>,
    resolution: Option<String>,
    source: Option<String>,
    video_codec: Option<String>,
    hdr: Option<String>,
    #[serde(default)]
    audio: Vec<String>,
    repack: Option<String>,
    group: Option<String>,
    unresolved: Option<String>,
}

/// The label the corpus writes for a numbering, which is also the serialized form.
fn numbering_label(numbering: Numbering) -> &'static str {
    match numbering {
        Numbering::SeasonEpisode => "season_episode",
        Numbering::CrossSeason => "cross_season",
        Numbering::PrefixedAbsolute => "prefixed_absolute",
        Numbering::BareAbsolute => "bare_absolute",
    }
}

/// The label the corpus writes for a refusal, covering every reason the parser can state.
fn reason_label(reason: &UnresolvedReason) -> &'static str {
    match reason {
        UnresolvedReason::Empty => "empty",
        UnresolvedReason::NoTitle => "no_title",
        UnresolvedReason::AmbiguousYear { .. } => "ambiguous_year",
        UnresolvedReason::AmbiguousAbsolute { .. } => "ambiguous_absolute",
        UnresolvedReason::ConflictingSeasons { .. } => "conflicting_seasons",
        UnresolvedReason::ConflictingEpisodes { .. } => "conflicting_episodes",
        UnresolvedReason::NotAName { .. } => "not_a_name",
    }
}

fn compare(case: &Case, parsed: &Result<LibraryName, NameError>) -> Result<(), String> {
    let mut problems = Vec::new();
    let found = match (parsed, case.unresolved.as_deref()) {
        (Ok(name), None) => name,
        (Err(error), Some(reason)) => {
            let actual = reason_label(&error.reason);
            if actual != reason {
                problems.push(format!("reason: expected `{reason}`, read `{actual}`"));
            }
            &error.name
        }
        (Ok(_), Some(reason)) => {
            return Err(format!("expected the name to be refused as `{reason}`"));
        }
        (Err(error), None) => return Err(format!("expected a parse, but: {error}")),
    };

    check_fields(case, found, &mut problems);
    check_release(case, found.release(), &mut problems);
    if found.raw() != case.name {
        problems.push("raw: input text was not preserved".to_owned());
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("; "))
    }
}

/// Compares everything the name itself states, whether it resolved or was refused.
fn check_fields(case: &Case, found: &LibraryName, problems: &mut Vec<String>) {
    let mut check = |field: &str, expected: String, actual: String| {
        if expected != actual {
            problems.push(format!("{field}: expected `{expected}`, parsed `{actual}`"));
        }
    };
    check(
        "title",
        case.title.clone().unwrap_or_default(),
        found.title().unwrap_or_default().to_owned(),
    );
    check(
        "year",
        case.year.map_or_else(String::new, |year| year.to_string()),
        found
            .year()
            .map_or_else(String::new, |year| year.to_string()),
    );
    check(
        "season",
        case.season
            .map_or_else(String::new, |season| season.to_string()),
        found
            .season()
            .map_or_else(String::new, |season| season.to_string()),
    );
    let expected_span = case
        .episodes
        .map_or_else(String::new, |[first, last]| format!("{first}-{last}"));
    let actual_span = found.episodes().map_or_else(String::new, |span| {
        format!("{}-{}", span.first(), span.last())
    });
    check("episodes", expected_span, actual_span);
    check(
        "absolute",
        case.absolute
            .map_or_else(String::new, |number| number.to_string()),
        found
            .absolute()
            .map_or_else(String::new, |number| number.to_string()),
    );
    check(
        "numbering",
        case.numbering.clone().unwrap_or_default(),
        found.numbering().map_or_else(String::new, |numbering| {
            numbering_label(numbering).to_owned()
        }),
    );
    check(
        "extension",
        case.extension.clone().unwrap_or_default(),
        found.extension().unwrap_or_default().to_owned(),
    );
}

/// Compares the release tail the name hands to [`ReleaseName`].
fn check_release(case: &Case, release: &ReleaseName, problems: &mut Vec<String>) {
    let mut check = |field: &str, expected: String, actual: String| {
        if expected != actual {
            problems.push(format!("{field}: expected `{expected}`, parsed `{actual}`"));
        }
    };
    check(
        "resolution",
        case.resolution.clone().unwrap_or_default(),
        release
            .resolution()
            .map(Resolution::as_str)
            .unwrap_or_default()
            .to_owned(),
    );
    check(
        "source",
        case.source.clone().unwrap_or_default(),
        release
            .source()
            .map(ReleaseSource::as_str)
            .unwrap_or_default()
            .to_owned(),
    );
    check(
        "video_codec",
        case.video_codec.clone().unwrap_or_default(),
        release
            .video_codec()
            .map(VideoCodec::as_str)
            .unwrap_or_default()
            .to_owned(),
    );
    check(
        "hdr",
        case.hdr.clone().unwrap_or_default(),
        release
            .hdr()
            .map(HdrFormat::as_str)
            .unwrap_or_default()
            .to_owned(),
    );
    let expected_audio: Vec<&str> = case.audio.iter().map(String::as_str).collect();
    let actual_audio: Vec<&str> = release.audio().iter().map(|codec| codec.as_str()).collect();
    check("audio", expected_audio.join("+"), actual_audio.join("+"));
    check(
        "group",
        case.group.clone().unwrap_or_default(),
        release.group().unwrap_or_default().to_owned(),
    );
    check(
        "repack",
        case.repack.clone().unwrap_or_default(),
        release
            .repack()
            .map(|kind| kind.to_string())
            .unwrap_or_default(),
    );
}

#[test]
fn the_corpus_labels_are_the_serialized_numbering() {
    let labelled = [
        (Numbering::SeasonEpisode, "season_episode"),
        (Numbering::CrossSeason, "cross_season"),
        (Numbering::PrefixedAbsolute, "prefixed_absolute"),
        (Numbering::BareAbsolute, "bare_absolute"),
    ];
    for (numbering, label) in labelled {
        let json = serde_json::to_string(&numbering).expect("serializes");
        assert_eq!(json, format!("\"{label}\""));
        assert_eq!(numbering_label(numbering), label);
    }
}

#[test]
fn the_corpus_covers_every_rejection_path() {
    let cases: Vec<Case> = serde_json::from_str(CORPUS).expect("corpus is valid JSON");
    let stated: Vec<&str> = cases
        .iter()
        .filter_map(|case| case.unresolved.as_deref())
        .collect();
    let paths = [
        "empty",
        "no_title",
        "ambiguous_year",
        "ambiguous_absolute",
        "conflicting_seasons",
        "conflicting_episodes",
        "not_a_name",
    ];
    for path in paths {
        assert!(
            stated.contains(&path),
            "the corpus states no `{path}` case; the rejection path is untested"
        );
    }
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
        let parsed = LibraryName::parse(&case.name);
        match compare(case, &parsed) {
            Ok(()) => correct += 1,
            Err(problems) => failures.push(format!("{}: {problems}", case.name)),
        }
    }

    let total = cases.len();
    let accuracy = correct as f64 / total as f64;
    println!(
        "library corpus: {correct}/{total} = {:.1}% (floor {:.0}%)",
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
