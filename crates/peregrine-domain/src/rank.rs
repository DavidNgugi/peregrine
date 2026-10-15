//! Release ranking.
//!
//! [`ReleaseRanker::rank`] is a pure function of a candidate and a
//! [`QualityProfile`]: same inputs, same score and same ordered reasons, always —
//! no clock, no randomness, no iteration over a map. It is the scoring half of F5,
//! and its reasons are what `release_decision.reasons_json` stores, so a user can
//! read *why* a release won rather than only that it did.
//!
//! Two decisions keep the arithmetic trustworthy:
//!
//! * Every score is an integer. A candidate loses one step for each entry a
//!   profile's preference list ranks above the value it declares — or, for audio,
//!   above the best position any of the codecs the *whole name* mentions holds,
//!   because the release parser reads a codec tag wherever it appears, including in
//!   a group called `dd`. The steps are weighted so that the resolution outranks
//!   every source, the source every video codec, and the video codec every audio
//!   codec. No step of that comparison involves a float, so no rounding decides a
//!   winner.
//! * A candidate the profile does not accept is a typed [`Rejection`] naming the
//!   rule that refused it, never a low score. A silent zero would look like a
//!   legitimate result to the caller and to the audit trail.

use serde::{Deserialize, Serialize};

use crate::quality::{QualityProfile, SizeCaps};
use crate::release::{AudioCodec, ReleaseName, ReleaseSource, Resolution, VideoCodec};

/// The relative weight of a resolution position.
const RESOLUTION_STEP: u64 = 1 << 12;
/// The relative weight of a source position.
const SOURCE_STEP: u64 = 1 << 8;
/// The relative weight of a video-codec position.
const VIDEO_CODEC_STEP: u64 = 1 << 4;
/// The relative weight of an audio-codec position.
const AUDIO_STEP: u64 = 1;

/// The largest index a family's preference list can reach.
///
/// This is also the base of the step weights: one index at a step is worth more
/// than every index of every lower step combined only while an index stays under
/// it. `the_step_bounds_cover_every_family` keeps these bounds equal to the number
/// of variants their family has, because a variant added without raising the bound
/// is exactly what would break that inequality.
///
/// `Resolution` supplies its own `ALL`; the other three families have no constant
/// list, because their variants do not sort into the quality order a preference
/// list needs.
const MAX_RESOLUTION_INDEX: u64 = Resolution::ALL.len() as u64 - 1;
/// The index bound of a source preference list, from the `ReleaseSource` variants.
const MAX_SOURCE_INDEX: u64 = 14;
/// The index bound of a video-codec preference list, from the `VideoCodec` variants.
const MAX_VIDEO_CODEC_INDEX: u64 = 5;
/// The index bound of an audio-codec preference list, from the `AudioCodec` variants.
const MAX_AUDIO_INDEX: u64 = 11;

/// The score of a candidate that sits at the top of every preference list, which
/// is also the most a candidate can lose.
pub const MAX_SCORE: u64 = RESOLUTION_STEP * MAX_RESOLUTION_INDEX
    + SOURCE_STEP * MAX_SOURCE_INDEX
    + VIDEO_CODEC_STEP * MAX_VIDEO_CODEC_INDEX
    + AUDIO_STEP * MAX_AUDIO_INDEX;

/// A candidate's position in one of a profile's preference lists; `0` is the most
/// preferred entry, matching the ordering `QualityRules` documents for its lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Position(u32);

impl Position {
    /// Builds a position from its index in a preference list.
    pub const fn new(index: u32) -> Self {
        Self(index)
    }

    /// The index in the profile's preference list.
    pub const fn index(self) -> u32 {
        self.0
    }

    /// The loss this position costs, at one step per entry below the preferred one.
    const fn penalty(self, step: u64) -> u64 {
        step * self.0 as u64
    }
}

/// The rule a reason or a rejection belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonKind {
    /// The resolution rule.
    Resolution,
    /// The source rule.
    Source,
    /// The video-codec rule.
    VideoCodec,
    /// The audio-codec rule.
    Audio,
    /// The release-group rule.
    Group,
    /// The size caps.
    Size,
}

impl ReasonKind {
    /// The stable label the `release_decision` audit trail stores.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Resolution => "resolution",
            Self::Source => "source",
            Self::VideoCodec => "video_codec",
            Self::Audio => "audio",
            Self::Group => "group",
            Self::Size => "size",
        }
    }
}

/// One rule's contribution to a score.
///
/// The reasons are values rather than sentences so the audit trail can be read
/// back, diffed, and translated; [`Ranking::describe`] is the one place a reason
/// becomes text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "rule", rename_all = "snake_case")]
pub enum RankReason {
    /// Where the candidate's resolution sits in the profile's resolution list.
    Resolution {
        /// The position in the profile's list, `None` when the profile has no rule.
        position: Option<Position>,
    },
    /// Where the candidate's source sits in the profile's source list.
    Source {
        /// The position in the profile's list, `None` when the profile has no rule.
        position: Option<Position>,
    },
    /// Where the candidate's video codec sits in the profile's codec list.
    VideoCodec {
        /// The position in the profile's list, `None` when the profile has no rule.
        position: Option<Position>,
    },
    /// Where the candidate's best audio codec sits in the profile's audio list.
    Audio {
        /// The position in the profile's list, `None` when the profile has no rule.
        position: Option<Position>,
    },
    /// The release-group rule, which gates rather than scores; the group itself is
    /// in the candidate's release name, so the reason does not repeat it.
    Group,
    /// The size caps, which gate rather than score.
    Size {
        /// The candidate's size in bytes.
        bytes: u64,
    },
}

impl RankReason {
    /// The rule this reason belongs to.
    pub const fn kind(&self) -> ReasonKind {
        match self {
            Self::Resolution { .. } => ReasonKind::Resolution,
            Self::Source { .. } => ReasonKind::Source,
            Self::VideoCodec { .. } => ReasonKind::VideoCodec,
            Self::Audio { .. } => ReasonKind::Audio,
            Self::Group => ReasonKind::Group,
            Self::Size { .. } => ReasonKind::Size,
        }
    }

    /// The points this rule deducted from [`MAX_SCORE`]; a gate deducts nothing.
    pub const fn points(&self) -> u64 {
        match self {
            Self::Resolution { position } => match position {
                Some(position) => position.penalty(RESOLUTION_STEP),
                None => 0,
            },
            Self::Source { position } => match position {
                Some(position) => position.penalty(SOURCE_STEP),
                None => 0,
            },
            Self::VideoCodec { position } => match position {
                Some(position) => position.penalty(VIDEO_CODEC_STEP),
                None => 0,
            },
            Self::Audio { position } => match position {
                Some(position) => position.penalty(AUDIO_STEP),
                None => 0,
            },
            Self::Group | Self::Size { .. } => 0,
        }
    }

    /// The position this reason found, when it scored one.
    pub const fn position(&self) -> Option<Position> {
        match self {
            Self::Resolution { position }
            | Self::Source { position }
            | Self::VideoCodec { position }
            | Self::Audio { position } => *position,
            Self::Group | Self::Size { .. } => None,
        }
    }
}

/// Why a candidate cannot be ranked.
// The payloads of two different refusals are not comparable and the release enums
// carry no `Ord` to derive one from, so the order below is written by hand: it is
// the declaration order, which is all a caller sorting refusals ever needs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Rejection {
    /// The profile constrains the resolution and the candidate declares none.
    ResolutionNotDeclared,
    /// The candidate declares a resolution outside the profile's list.
    ResolutionNotAllowed {
        /// The refused resolution.
        value: Resolution,
    },
    /// The profile constrains the source and the candidate declares none.
    SourceNotDeclared,
    /// The candidate declares a source outside the profile's list.
    SourceNotAllowed {
        /// The refused source.
        value: ReleaseSource,
    },
    /// The profile constrains the video codec and the candidate declares none.
    VideoCodecNotDeclared,
    /// The candidate declares a video codec outside the profile's list.
    VideoCodecNotAllowed {
        /// The refused codec.
        value: VideoCodec,
    },
    /// The profile constrains the audio and the candidate declares no codec on the list.
    AudioNotAllowed {
        /// The profile's audio list, in preference order.
        allowed: Vec<AudioCodec>,
    },
    /// The candidate's group is on the profile's block list.
    GroupBlocked {
        /// The blocked group the candidate declares.
        group: String,
    },
    /// The profile allows only listed groups and the candidate's group is not one.
    GroupNotAllowed {
        /// The candidate's group, if it declares one.
        group: Option<String>,
    },
    /// The candidate is smaller than the profile's floor.
    BelowSizeFloor {
        /// The candidate's size.
        bytes: u64,
        /// The profile's floor.
        floor: u64,
    },
    /// The candidate is larger than the profile's ceiling.
    AboveSizeCeiling {
        /// The candidate's size.
        bytes: u64,
        /// The profile's ceiling.
        ceiling: u64,
    },
}

impl Rejection {
    /// The rule that refused the candidate.
    pub const fn kind(&self) -> ReasonKind {
        match self {
            Self::ResolutionNotDeclared | Self::ResolutionNotAllowed { .. } => {
                ReasonKind::Resolution
            }
            Self::SourceNotDeclared | Self::SourceNotAllowed { .. } => ReasonKind::Source,
            Self::VideoCodecNotDeclared | Self::VideoCodecNotAllowed { .. } => {
                ReasonKind::VideoCodec
            }
            Self::AudioNotAllowed { .. } => ReasonKind::Audio,
            Self::GroupBlocked { .. } | Self::GroupNotAllowed { .. } => ReasonKind::Group,
            Self::BelowSizeFloor { .. } | Self::AboveSizeCeiling { .. } => ReasonKind::Size,
        }
    }

    /// The stable label the audit trail stores: `REASON_KIND` and an argument when
    /// the refusal has one, with no spaces so a log line stays parseable.
    pub fn label(&self) -> String {
        match self {
            Self::ResolutionNotDeclared => "resolution-not-declared".to_owned(),
            Self::ResolutionNotAllowed { value } => {
                format!("resolution-not-allowed:{}", value.as_str())
            }
            Self::SourceNotDeclared => "source-not-declared".to_owned(),
            Self::SourceNotAllowed { value } => format!("source-not-allowed:{}", value.as_str()),
            Self::VideoCodecNotDeclared => "video-codec-not-declared".to_owned(),
            Self::VideoCodecNotAllowed { value } => {
                format!("video-codec-not-allowed:{}", value.as_str())
            }
            Self::AudioNotAllowed { allowed } => {
                let allowed: Vec<&str> = allowed.iter().map(|codec| codec.as_str()).collect();
                format!("audio-not-allowed:{}", allowed.join("|"))
            }
            Self::GroupBlocked { group } => format!("group-blocked:{group}"),
            Self::GroupNotAllowed { group } => {
                format!("group-not-allowed:{}", group.as_deref().unwrap_or("none"))
            }
            Self::BelowSizeFloor { bytes, floor } => format!("below-size-floor:{bytes}<{floor}"),
            Self::AboveSizeCeiling { bytes, ceiling } => {
                format!("above-size-ceiling:{bytes}>{ceiling}")
            }
        }
    }
}

impl Rejection {
    /// The declaration index, which is the only thing two refusals compare on.
    const fn order(&self) -> u8 {
        match self {
            Self::ResolutionNotDeclared => 0,
            Self::ResolutionNotAllowed { .. } => 1,
            Self::SourceNotDeclared => 2,
            Self::SourceNotAllowed { .. } => 3,
            Self::VideoCodecNotDeclared => 4,
            Self::VideoCodecNotAllowed { .. } => 5,
            Self::AudioNotAllowed { .. } => 6,
            Self::GroupBlocked { .. } => 7,
            Self::GroupNotAllowed { .. } => 8,
            Self::BelowSizeFloor { .. } => 9,
            Self::AboveSizeCeiling { .. } => 10,
        }
    }
}

impl PartialOrd for Rejection {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Rejection {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.order().cmp(&other.order())
    }
}

/// The outcome of ranking one candidate.
///
/// The comparison is total and states the winner rule in one place: a ranked
/// candidate sorts above a rejected one, and among ranked candidates the higher
/// score sorts first. Two rankings that tie compare equal, because equal scores are
/// a tie and nothing else orders them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Ranking {
    /// The candidate satisfies every rule the profile states.
    Ranked {
        /// `MAX_SCORE` less the sum of the reasons' [`RankReason::points`].
        score: u64,
        /// One entry per rule the ranker consulted, in a fixed order.
        reasons: Vec<RankReason>,
    },
    /// The profile refuses the candidate.
    Rejected {
        /// The rule that refused it, named.
        rejection: Rejection,
    },
}

impl Ranking {
    /// The candidate's score, or `None` when the profile refused it.
    pub const fn score(&self) -> Option<u64> {
        match self {
            Self::Ranked { score, .. } => Some(*score),
            Self::Rejected { .. } => None,
        }
    }

    /// Every rule the ranker consulted, in the order it consulted them; empty for a
    /// rejection, which stops at the first rule that refuses it.
    pub fn reasons(&self) -> &[RankReason] {
        match self {
            Self::Ranked { reasons, .. } => reasons,
            Self::Rejected { .. } => &[],
        }
    }

    /// The refusal, when the profile made one.
    pub const fn rejection(&self) -> Option<&Rejection> {
        match self {
            Self::Ranked { .. } => None,
            Self::Rejected { rejection } => Some(rejection),
        }
    }

    /// The audit-trail line for this outcome: the score and every reason, or the
    /// named rejection. This is the only place a [`RankReason`] becomes text.
    pub fn describe(&self) -> String {
        match self {
            Self::Ranked { score, reasons } => {
                let reasons: Vec<String> = reasons.iter().map(describe_reason).collect();
                format!("score={score} {}", reasons.join("+"))
            }
            Self::Rejected { rejection } => format!("rejected={}", rejection.label()),
        }
    }
}

/// Renders one reason as `rule@position` for a scored family, or `rule` and
/// `rule:value` for the two gates.
fn describe_reason(reason: &RankReason) -> String {
    match reason {
        RankReason::Resolution { position } => {
            format!("resolution@{}", position_label(*position))
        }
        RankReason::Source { position } => format!("source@{}", position_label(*position)),
        RankReason::VideoCodec { position } => {
            format!("video_codec@{}", position_label(*position))
        }
        RankReason::Audio { position } => format!("audio@{}", position_label(*position)),
        RankReason::Group => "group".to_owned(),
        RankReason::Size { bytes } => format!("size:{bytes}"),
    }
}

/// A position's index, or `-` when the profile states no rule for that criterion.
fn position_label(position: Option<Position>) -> String {
    position.map_or_else(|| "-".to_owned(), |position| position.index().to_string())
}

impl PartialOrd for Ranking {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Ranking {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Self::Rejected { .. }, Self::Ranked { .. }) => std::cmp::Ordering::Less,
            (Self::Ranked { .. }, Self::Rejected { .. }) => std::cmp::Ordering::Greater,
            (Self::Rejected { rejection: left }, Self::Rejected { rejection: right }) => {
                left.cmp(right)
            }
            (Self::Ranked { score: left, .. }, Self::Ranked { score: right, .. }) => {
                left.cmp(right)
            }
        }
    }
}

/// Everything about a candidate that the profile can score.
///
/// The name carries the parsed tags; the size is the indexer's byte count, which no
/// release title states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The parsed release title.
    pub name: ReleaseName,
    /// The release's size in bytes.
    pub size_bytes: u64,
}

impl Candidate {
    /// Pairs a parsed release name with the size its indexer reported.
    pub const fn new(name: ReleaseName, size_bytes: u64) -> Self {
        Self { name, size_bytes }
    }
}

/// The scoring policy: a pure function from a candidate and a profile to a
/// [`Ranking`].
///
/// This is a unit type rather than a free function because F5 names it as the
/// domain's ranking entry point, and because a caller can hold it while swapping
/// profiles.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReleaseRanker;

impl ReleaseRanker {
    /// Scores a candidate against a profile, or names the rule that refuses it.
    pub fn rank(candidate: &Candidate, profile: &QualityProfile) -> Ranking {
        let rules = profile.rules();

        let resolution = match score_resolution(candidate.name.resolution(), rules.resolutions()) {
            Ok(position) => position,
            Err(rejection) => return Ranking::Rejected { rejection },
        };
        let source = match score_source(candidate.name.source(), rules.sources()) {
            Ok(position) => position,
            Err(rejection) => return Ranking::Rejected { rejection },
        };
        let video_codec =
            match score_video_codec(candidate.name.video_codec(), rules.video_codecs()) {
                Ok(position) => position,
                Err(rejection) => return Ranking::Rejected { rejection },
            };
        let audio = match score_audio(candidate.name.audio(), rules.audio_codecs()) {
            Ok(position) => position,
            Err(rejection) => return Ranking::Rejected { rejection },
        };
        if let Err(rejection) = check_group(candidate.name.group(), profile) {
            return Ranking::Rejected { rejection };
        }
        if let Err(rejection) = check_size(candidate.size_bytes, rules.size_caps()) {
            return Ranking::Rejected { rejection };
        }

        // One entry per family, in the order the rules are applied and the weights
        // descend, so the output order never depends on which rules happened to
        // match. Size is a gate, so it contributes a reason and no points.
        let reasons = vec![
            RankReason::Resolution {
                position: resolution,
            },
            RankReason::Source { position: source },
            RankReason::VideoCodec {
                position: video_codec,
            },
            RankReason::Audio { position: audio },
            RankReason::Group,
            RankReason::Size {
                bytes: candidate.size_bytes,
            },
        ];
        // The subtraction cannot underflow: `listed_position` is the first match in a
        // list whose entries are values of a family with a fixed number of variants,
        // so an index can only reach `MAX_*_INDEX`, which `MAX_SCORE` covers by
        // construction.
        let penalty: u64 = reasons.iter().map(RankReason::points).sum();

        Ranking::Ranked {
            score: MAX_SCORE - penalty,
            reasons,
        }
    }
}

/// The index of `value` in a profile's preference list.
///
/// `None` means the list does not hold the value, which the caller turns into a
/// rejection. An empty list never reaches here: it is no constraint at all, so it
/// accepts every value and awards the top position, because the profile has stated
/// no preference to lose points to.
fn listed_position<T: PartialEq>(list: &[T], value: Option<&T>) -> Option<Position> {
    let index = list.iter().position(|entry| Some(entry) == value)?;
    u32::try_from(index).ok().map(Position::new)
}

/// Scores the resolution, or names why it cannot be scored.
fn score_resolution(
    value: Option<Resolution>,
    list: &[Resolution],
) -> Result<Option<Position>, Rejection> {
    if list.is_empty() {
        return Ok(None);
    }
    listed_position(list, value.as_ref()).map_or_else(
        || {
            Err(match value {
                Some(value) => Rejection::ResolutionNotAllowed { value },
                None => Rejection::ResolutionNotDeclared,
            })
        },
        |position| Ok(Some(position)),
    )
}

/// Scores the source, or names why it cannot be scored.
fn score_source(
    value: Option<ReleaseSource>,
    list: &[ReleaseSource],
) -> Result<Option<Position>, Rejection> {
    if list.is_empty() {
        return Ok(None);
    }
    listed_position(list, value.as_ref()).map_or_else(
        || {
            Err(match value {
                Some(value) => Rejection::SourceNotAllowed { value },
                None => Rejection::SourceNotDeclared,
            })
        },
        |position| Ok(Some(position)),
    )
}

/// Scores the video codec, or names why it cannot be scored.
fn score_video_codec(
    value: Option<VideoCodec>,
    list: &[VideoCodec],
) -> Result<Option<Position>, Rejection> {
    if list.is_empty() {
        return Ok(None);
    }
    listed_position(list, value.as_ref()).map_or_else(
        || {
            Err(match value {
                Some(value) => Rejection::VideoCodecNotAllowed { value },
                None => Rejection::VideoCodecNotDeclared,
            })
        },
        |position| Ok(Some(position)),
    )
}

/// Scores the audio by its best-matching codec, or names why none matches.
///
/// A release states several audio codecs — `DTS-HD MA 5.1` also implies plain
/// `DTS` — so the best position any of them holds is the one that counts.
fn score_audio(values: &[AudioCodec], list: &[AudioCodec]) -> Result<Option<Position>, Rejection> {
    if list.is_empty() {
        return Ok(None);
    }
    values
        .iter()
        .filter_map(|value| list.iter().position(|entry| entry == value))
        .min()
        .and_then(|index| u32::try_from(index).ok())
        .map_or_else(
            || {
                Err(Rejection::AudioNotAllowed {
                    allowed: list.to_vec(),
                })
            },
            |index| Ok(Some(Position::new(index))),
        )
}

/// Applies the group rules, which gate rather than score.
fn check_group(group: Option<&str>, profile: &QualityProfile) -> Result<(), Rejection> {
    let rules = profile.rules().groups();
    if rules.permits(group) {
        return Ok(());
    }
    if let Some(blocked) = group.filter(|group| rules.blocked().iter().any(|rule| rule == group)) {
        return Err(Rejection::GroupBlocked {
            group: blocked.to_owned(),
        });
    }
    Err(Rejection::GroupNotAllowed {
        group: group.map(str::to_owned),
    })
}

/// Applies the size caps, which gate rather than score.
fn check_size(bytes: u64, caps: SizeCaps) -> Result<(), Rejection> {
    if let Some(floor) = caps.floor()
        && bytes < floor
    {
        return Err(Rejection::BelowSizeFloor { bytes, floor });
    }
    if let Some(ceiling) = caps.ceiling()
        && bytes > ceiling
    {
        return Err(Rejection::AboveSizeCeiling { bytes, ceiling });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality::{GroupRules, QualityRank, QualityRules};
    use proptest::prelude::*;

    /// The registry token each family's variants are parsed from, so a fixture can
    /// render a release name that parses back to the tags it meant.
    const RESOLUTION_TAG: [(&str, Resolution); 7] = [
        ("2160p", Resolution::UltraHd),
        ("1440p", Resolution::QuadHd),
        ("1080p", Resolution::FullHd),
        ("1080i", Resolution::FullHdInterlaced),
        ("720p", Resolution::Hd),
        ("576p", Resolution::Sd576),
        ("480p", Resolution::Sd480),
    ];
    const SOURCE_TAG: [(&str, ReleaseSource); 15] = [
        ("REMUX", ReleaseSource::Remux),
        ("BluRay", ReleaseSource::BluRay),
        ("BDRip", ReleaseSource::BdRip),
        ("BRRip", ReleaseSource::BrRip),
        ("WEB-DL", ReleaseSource::WebDl),
        ("WEBRip", ReleaseSource::WebRip),
        ("HDTV", ReleaseSource::Hdtv),
        ("DVDRip", ReleaseSource::DvdRip),
        ("HDRip", ReleaseSource::HdRip),
        ("SDTV", ReleaseSource::Sdtv),
        ("PDTV", ReleaseSource::Pdtv),
        ("CAM", ReleaseSource::Cam),
        ("TS", ReleaseSource::Ts),
        ("TC", ReleaseSource::Tc),
        ("R5", ReleaseSource::R5),
    ];
    const VIDEO_CODEC_TAG: [(&str, VideoCodec); 6] = [
        ("x264", VideoCodec::H264),
        ("x265", VideoCodec::H265),
        ("AV1", VideoCodec::Av1),
        ("XviD", VideoCodec::Xvid),
        ("DivX", VideoCodec::Divx),
        ("VP9", VideoCodec::Vp9),
    ];
    const AUDIO_CODEC_TAG: [(&str, AudioCodec); 12] = [
        ("AAC", AudioCodec::Aac),
        ("AC3", AudioCodec::Ac3),
        ("EAC3", AudioCodec::Eac3),
        ("DTS", AudioCodec::Dts),
        ("DTS-HD", AudioCodec::DtsHd),
        ("DTS:X", AudioCodec::DtsX),
        ("TrueHD", AudioCodec::TrueHd),
        ("Atmos", AudioCodec::Atmos),
        ("FLAC", AudioCodec::Flac),
        ("MP3", AudioCodec::Mp3),
        ("Opus", AudioCodec::Opus),
        ("PCM", AudioCodec::Pcm),
    ];

    /// The pieces a fixture name is rendered from.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Parts {
        resolution: Resolution,
        source: ReleaseSource,
        video_codec: VideoCodec,
        audio: Vec<AudioCodec>,
        group: Option<String>,
    }

    impl Parts {
        /// The fixture every other one is derived from: the top of each family's
        /// registry order, a broadcast group, and no group.
        fn baseline() -> Self {
            Self {
                resolution: Resolution::UltraHd,
                source: ReleaseSource::Remux,
                video_codec: VideoCodec::H265,
                audio: vec![AudioCodec::TrueHd],
                group: None,
            }
        }
    }

    fn tag_of<T: Copy + PartialEq>(table: &'static [(&'static str, T)], value: T) -> &'static str {
        table
            .iter()
            .find(|(_, candidate)| *candidate == value)
            .map(|(tag, _)| *tag)
            .expect("the registry table covers every variant")
    }

    /// Renders a release name from its parts, in the order a scene name uses.
    fn render(parts: &Parts) -> String {
        let mut name = format!(
            "Movie.2024.{}.{}.{}",
            tag_of(&RESOLUTION_TAG, parts.resolution),
            tag_of(&SOURCE_TAG, parts.source),
            tag_of(&VIDEO_CODEC_TAG, parts.video_codec),
        );
        for codec in &parts.audio {
            name.push('.');
            name.push_str(tag_of(&AUDIO_CODEC_TAG, *codec));
        }
        if let Some(group) = &parts.group {
            name.push('-');
            name.push_str(group);
        }
        name
    }

    /// Renders and parses, so a fixture exercises the same path a caller does.
    fn name_of(parts: &Parts) -> ReleaseName {
        ReleaseName::parse(&render(parts))
    }

    fn candidate(parts: &Parts, size_bytes: u64) -> Candidate {
        Candidate::new(name_of(parts), size_bytes)
    }

    fn caps(floor: Option<u64>, ceiling: Option<u64>) -> SizeCaps {
        SizeCaps::new(floor, ceiling).expect("the fixture caps are consistent")
    }

    fn groups(allowed: &[&str], blocked: &[&str]) -> GroupRules {
        GroupRules::new(
            allowed.iter().map(|group| (*group).to_owned()).collect(),
            blocked.iter().map(|group| (*group).to_owned()).collect(),
        )
        .expect("the fixture groups are consistent")
    }

    fn profile_with(
        resolutions: Vec<Resolution>,
        sources: Vec<ReleaseSource>,
        video_codecs: Vec<VideoCodec>,
        audio_codecs: Vec<AudioCodec>,
        size_caps: SizeCaps,
        group_rules: GroupRules,
    ) -> QualityProfile {
        QualityProfile::new(
            "Fixture",
            QualityRules::new(
                resolutions,
                sources,
                video_codecs,
                audio_codecs,
                size_caps,
                group_rules,
            )
            .expect("the fixture preference orders name no value twice"),
            QualityRank::new(3),
            true,
            true,
        )
        .expect("the fixture profile is valid")
    }

    /// A profile that states no preference at all, so every candidate passes.
    fn permissive() -> QualityProfile {
        profile_with(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            caps(None, None),
            groups(&[], &[]),
        )
    }

    /// The preference lists the golden profile states, so a case that must be scored
    /// rather than rejected can be built from exactly what the profile allows.
    const GOLDEN_RESOLUTIONS: [Resolution; 4] = [
        Resolution::UltraHd,
        Resolution::FullHd,
        Resolution::Hd,
        Resolution::Sd480,
    ];
    const GOLDEN_SOURCES: [ReleaseSource; 4] = [
        ReleaseSource::Remux,
        ReleaseSource::BluRay,
        ReleaseSource::WebDl,
        ReleaseSource::Hdtv,
    ];
    const GOLDEN_VIDEO_CODECS: [VideoCodec; 3] =
        [VideoCodec::Av1, VideoCodec::H265, VideoCodec::H264];
    const GOLDEN_AUDIO_CODECS: [AudioCodec; 3] =
        [AudioCodec::TrueHd, AudioCodec::Eac3, AudioCodec::Aac];
    const GOLDEN_FLOOR: u64 = 1_000_000_000;
    const GOLDEN_CEILING: u64 = 20_000_000_000;
    const GOLDEN_GROUP: &str = "FRAMESTOR";

    /// The profile the golden table is written against: an explicit preference
    /// order per family, caps, and both halves of the group rules.
    fn golden_profile() -> QualityProfile {
        profile_with(
            GOLDEN_RESOLUTIONS.to_vec(),
            GOLDEN_SOURCES.to_vec(),
            GOLDEN_VIDEO_CODECS.to_vec(),
            GOLDEN_AUDIO_CODECS.to_vec(),
            caps(Some(GOLDEN_FLOOR), Some(GOLDEN_CEILING)),
            groups(&[GOLDEN_GROUP], &["YIFY"]),
        )
    }

    /// The points a reason deducted, for the golden table's audit.
    fn points(ranking: &Ranking, kind: ReasonKind) -> u64 {
        ranking
            .reasons()
            .iter()
            .find(|reason| reason.kind() == kind)
            .map_or(0, RankReason::points)
    }

    #[test]
    fn every_registry_tag_parses_into_the_field_the_fixture_meant() {
        for (tag, expected) in RESOLUTION_TAG {
            let name = ReleaseName::parse(&format!("Movie.2024.{tag}.BluRay.x264"));
            assert_eq!(name.resolution(), Some(expected), "resolution tag {tag}");
        }
        for (tag, expected) in SOURCE_TAG {
            let name = ReleaseName::parse(&format!("Movie.2024.1080p.{tag}.x264"));
            assert_eq!(name.source(), Some(expected), "source tag {tag}");
        }
        for (tag, expected) in VIDEO_CODEC_TAG {
            let name = ReleaseName::parse(&format!("Movie.2024.1080p.BluRay.{tag}"));
            assert_eq!(name.video_codec(), Some(expected), "codec tag {tag}");
        }
        for (tag, expected) in AUDIO_CODEC_TAG {
            let name = ReleaseName::parse(&format!("Movie.2024.1080p.BluRay.x264.{tag}"));
            assert_eq!(name.audio(), [expected], "audio tag {tag}");
        }
    }

    #[test]
    fn the_step_bounds_cover_every_family_and_outrank_every_lower_step() {
        assert_eq!(
            usize::try_from(MAX_RESOLUTION_INDEX).expect("a small index"),
            Resolution::ALL.len() - 1
        );
        for (listed, expected) in [
            (MAX_SOURCE_INDEX, SOURCE_TAG.len()),
            (MAX_VIDEO_CODEC_INDEX, VIDEO_CODEC_TAG.len()),
            (MAX_AUDIO_INDEX, AUDIO_CODEC_TAG.len()),
        ] {
            assert_eq!(
                usize::try_from(listed).expect("a small index"),
                expected - 1,
                "a family gained a variant; raise its index bound"
            );
        }
        // A const block, so a broken weight is a compile error rather than a test
        // that only fails once somebody runs it.
        const {
            assert!(
                RESOLUTION_STEP
                    > SOURCE_STEP * MAX_SOURCE_INDEX
                        + VIDEO_CODEC_STEP * MAX_VIDEO_CODEC_INDEX
                        + AUDIO_STEP * MAX_AUDIO_INDEX,
                "one resolution index must outrank every lower index combined"
            );
            assert!(
                SOURCE_STEP
                    > VIDEO_CODEC_STEP * MAX_VIDEO_CODEC_INDEX + AUDIO_STEP * MAX_AUDIO_INDEX,
                "one source index must outrank every lower index combined"
            );
            assert!(
                VIDEO_CODEC_STEP > AUDIO_STEP * MAX_AUDIO_INDEX,
                "one video-codec index must outrank every audio index combined"
            );
        }
    }

    #[test]
    fn a_rendered_fixture_parses_back_to_the_parts_it_came_from() {
        let parts = Parts {
            resolution: Resolution::FullHd,
            source: ReleaseSource::WebDl,
            video_codec: VideoCodec::H264,
            audio: vec![AudioCodec::Eac3, AudioCodec::Aac],
            group: Some("FRAMESTOR".to_owned()),
        };
        let name = name_of(&parts);
        assert_eq!(name.resolution(), Some(parts.resolution));
        assert_eq!(name.source(), Some(parts.source));
        assert_eq!(name.video_codec(), Some(parts.video_codec));
        assert_eq!(name.audio(), parts.audio);
        assert_eq!(name.group(), parts.group.as_deref());
    }

    #[test]
    fn a_permissive_profile_accepts_a_tagless_name() {
        let tagless = Candidate::new(ReleaseName::parse("Some Random File Name"), 0);
        let ranking = ReleaseRanker::rank(&tagless, &permissive());
        assert_eq!(
            ranking,
            Ranking::Ranked {
                score: MAX_SCORE,
                reasons: vec![
                    RankReason::Resolution { position: None },
                    RankReason::Source { position: None },
                    RankReason::VideoCodec { position: None },
                    RankReason::Audio { position: None },
                    RankReason::Group,
                    RankReason::Size { bytes: 0 },
                ],
            },
            "with no rule stated, the tagless candidate scores the ceiling"
        );
    }

    #[test]
    fn a_configured_rule_rejects_a_candidate_that_declares_nothing() {
        let rules = |resolutions: Vec<Resolution>| {
            profile_with(
                resolutions,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                caps(None, None),
                groups(&[], &[]),
            )
        };
        let tagless = Candidate::new(ReleaseName::parse("Some Random File Name"), 5);
        assert_eq!(
            ReleaseRanker::rank(&tagless, &rules(vec![Resolution::FullHd])).rejection(),
            Some(&Rejection::ResolutionNotDeclared)
        );
        assert_eq!(
            ReleaseRanker::rank(&tagless, &rules(vec![Resolution::Hd])).describe(),
            "rejected=resolution-not-declared"
        );
        assert_eq!(
            ReleaseRanker::rank(&tagless, &rules(Vec::new())).score(),
            Some(MAX_SCORE),
            "an empty rule accepts a name that declares nothing"
        );
    }

    #[test]
    fn a_golden_table_of_candidates_scores_and_explains_each_one() {
        let profile = golden_profile();
        let mut baseline = Parts::baseline();
        baseline.group = Some("FRAMESTOR".to_owned());

        let table: [(Candidate, &str); 6] = [
            (
                candidate(&baseline, 15_000_000_000),
                "score=28235 resolution@0+source@0+video_codec@1+audio@0+group+size:15000000000",
            ),
            (
                candidate(
                    &Parts {
                        resolution: Resolution::FullHd,
                        ..baseline.clone()
                    },
                    15_000_000_000,
                ),
                "score=24139 resolution@1+source@0+video_codec@1+audio@0+group+size:15000000000",
            ),
            (
                candidate(
                    &Parts {
                        source: ReleaseSource::Hdtv,
                        ..baseline.clone()
                    },
                    15_000_000_000,
                ),
                "score=27467 resolution@0+source@3+video_codec@1+audio@0+group+size:15000000000",
            ),
            (
                candidate(
                    &Parts {
                        video_codec: VideoCodec::H264,
                        ..baseline.clone()
                    },
                    15_000_000_000,
                ),
                "score=28219 resolution@0+source@0+video_codec@2+audio@0+group+size:15000000000",
            ),
            (
                candidate(
                    &Parts {
                        audio: vec![AudioCodec::Aac],
                        ..baseline.clone()
                    },
                    15_000_000_000,
                ),
                "score=28233 resolution@0+source@0+video_codec@1+audio@2+group+size:15000000000",
            ),
            (
                candidate(
                    &Parts {
                        group: Some("YIFY".to_owned()),
                        ..baseline.clone()
                    },
                    15_000_000_000,
                ),
                "rejected=group-blocked:YIFY",
            ),
        ];

        let seen: Vec<String> = table
            .iter()
            .map(|(candidate, expected)| {
                let actual = ReleaseRanker::rank(candidate, &profile);
                assert_eq!(
                    actual.describe(),
                    *expected,
                    "candidate {}",
                    candidate.name.raw()
                );
                format!("{}\t{}", candidate.name.raw(), actual.describe())
            })
            .collect();
        assert_eq!(seen.len(), 6, "the table is the whole golden set");
    }

    #[test]
    fn the_score_decomposes_into_the_points_its_reasons_report() {
        let profile = golden_profile();
        let parts = Parts {
            resolution: Resolution::Hd,
            source: ReleaseSource::WebDl,
            video_codec: VideoCodec::H264,
            audio: vec![AudioCodec::Aac],
            group: Some("FRAMESTOR".to_owned()),
        };
        let ranking = ReleaseRanker::rank(&candidate(&parts, 2_000_000_000), &profile);
        let Some(score) = ranking.score() else {
            panic!("the candidate is allowed");
        };
        let deducted: u64 = ranking.reasons().iter().map(RankReason::points).sum();
        assert_eq!(score, MAX_SCORE - deducted);
        assert_eq!(
            points(&ranking, ReasonKind::Resolution),
            2 * RESOLUTION_STEP
        );
        assert_eq!(points(&ranking, ReasonKind::Source), 2 * SOURCE_STEP);
        assert_eq!(
            points(&ranking, ReasonKind::VideoCodec),
            2 * VIDEO_CODEC_STEP
        );
        assert_eq!(points(&ranking, ReasonKind::Audio), 2 * AUDIO_STEP);
        assert_eq!(points(&ranking, ReasonKind::Group), 0);
        assert_eq!(points(&ranking, ReasonKind::Size), 0);
        assert_eq!(
            ranking.describe(),
            "score=19513 resolution@2+source@2+video_codec@2+audio@2+group+size:2000000000"
        );
    }

    #[test]
    fn the_reasons_are_always_in_the_documented_order() {
        let profile = golden_profile();
        let parts = Parts {
            resolution: Resolution::Sd480,
            source: ReleaseSource::Hdtv,
            video_codec: VideoCodec::H264,
            audio: vec![AudioCodec::Aac],
            group: Some("FRAMESTOR".to_owned()),
        };
        let ranking = ReleaseRanker::rank(&candidate(&parts, 1_500_000_000), &profile);
        let kinds: Vec<ReasonKind> = ranking.reasons().iter().map(RankReason::kind).collect();
        assert_eq!(
            kinds,
            [
                ReasonKind::Resolution,
                ReasonKind::Source,
                ReasonKind::VideoCodec,
                ReasonKind::Audio,
                ReasonKind::Group,
                ReasonKind::Size,
            ]
        );
    }

    #[test]
    fn a_ranking_and_its_reasons_round_trip_through_json() {
        let profile = golden_profile();
        let allowed = Parts {
            resolution: Resolution::Hd,
            group: Some(GOLDEN_GROUP.to_owned()),
            ..Parts::baseline()
        };
        let ranked = ReleaseRanker::rank(&candidate(&allowed, 2_000_000_000), &profile);
        let json = serde_json::to_string(&ranked).expect("a ranking serializes");
        assert_eq!(
            json,
            concat!(
                r#"{"outcome":"ranked","score":20043,"reasons":["#,
                r#"{"rule":"resolution","position":2},"#,
                r#"{"rule":"source","position":0},"#,
                r#"{"rule":"video_codec","position":1},"#,
                r#"{"rule":"audio","position":0},"#,
                r#"{"rule":"group"},"#,
                r#"{"rule":"size","bytes":2000000000}]}"#,
            ),
            "the audit trail's wire form is a decision, not an accident"
        );
        assert_eq!(
            serde_json::from_str::<Ranking>(&json).expect("the audit trail reads back"),
            ranked
        );

        let refused = ReleaseRanker::rank(
            &candidate(
                &Parts {
                    group: Some("YIFY".to_owned()),
                    ..Parts::baseline()
                },
                2_000_000_000,
            ),
            &profile,
        );
        let json = serde_json::to_string(&refused).expect("a rejection serializes");
        assert_eq!(
            json,
            r#"{"outcome":"rejected","rejection":{"reason":"group_blocked","group":"YIFY"}}"#
        );
        assert_eq!(
            serde_json::from_str::<Ranking>(&json).expect("the rejection reads back"),
            refused
        );
    }

    #[test]
    fn every_reason_and_rejection_names_the_rule_it_belongs_to() {
        // The taxonomy the audit trail is queried by, and the thing a new variant is
        // most likely to be added without: a reason whose `kind` lies, or a rejection
        // with no label, makes `release_decision.reasons_json` unreadable.
        let profile = golden_profile();
        let ranked = ReleaseRanker::rank(
            &candidate(
                &Parts {
                    group: Some(GOLDEN_GROUP.to_owned()),
                    ..Parts::baseline()
                },
                5_000_000_000,
            ),
            &profile,
        );
        let kinds: Vec<&str> = ranked
            .reasons()
            .iter()
            .map(|reason| reason.kind().as_str())
            .collect();
        assert_eq!(
            kinds,
            [
                "resolution",
                "source",
                "video_codec",
                "audio",
                "group",
                "size"
            ],
            "the taxonomy covers every reason the ranker emits"
        );

        for rejection in [
            Rejection::ResolutionNotDeclared,
            Rejection::ResolutionNotAllowed {
                value: Resolution::Hd,
            },
            Rejection::SourceNotDeclared,
            Rejection::SourceNotAllowed {
                value: ReleaseSource::Cam,
            },
            Rejection::VideoCodecNotDeclared,
            Rejection::VideoCodecNotAllowed {
                value: VideoCodec::Vp9,
            },
            Rejection::AudioNotAllowed {
                allowed: vec![AudioCodec::Aac],
            },
            Rejection::GroupBlocked {
                group: "YIFY".to_owned(),
            },
            Rejection::GroupNotAllowed { group: None },
            Rejection::BelowSizeFloor { bytes: 1, floor: 2 },
            Rejection::AboveSizeCeiling {
                bytes: 3,
                ceiling: 2,
            },
        ] {
            assert!(
                !rejection.label().is_empty(),
                "{rejection:?} needs a label for the audit trail"
            );
            assert!(
                !rejection.label().contains(char::is_whitespace),
                "{} must stay one parseable token",
                rejection.label()
            );
            assert!(
                serde_json::to_string(&rejection).is_ok(),
                "{rejection:?} must serialize"
            );
            let _ = rejection.kind();
        }
    }

    #[test]
    fn every_score_is_an_exact_integer_in_the_database_real_column() {
        const {
            assert!(
                MAX_SCORE < (1_u64 << 53),
                "a score past 2^53 would stop surviving the REAL round trip"
            );
        }
        let maximum = MAX_SCORE as f64;
        assert_eq!(maximum as u64, MAX_SCORE);
        assert_eq!((0_u64 as f64) as u64, 0);
    }

    #[test]
    fn a_group_that_is_neither_blocked_nor_allowed_is_rejected() {
        let profile = golden_profile();
        let parts = Parts {
            group: Some("SOMEGROUP".to_owned()),
            ..Parts::baseline()
        };
        assert_eq!(
            ReleaseRanker::rank(&candidate(&parts, 5_000_000_000), &profile).rejection(),
            Some(&Rejection::GroupNotAllowed {
                group: Some("SOMEGROUP".to_owned()),
            })
        );
        let groupless = Parts {
            group: None,
            ..Parts::baseline()
        };
        assert_eq!(
            ReleaseRanker::rank(&candidate(&groupless, 5_000_000_000), &profile).rejection(),
            Some(&Rejection::GroupNotAllowed { group: None }),
            "an allow list that names groups refuses a release that names none"
        );
        assert_eq!(
            ReleaseRanker::rank(&candidate(&groupless, 5_000_000_000), &permissive()).score(),
            Some(MAX_SCORE)
        );
    }

    #[test]
    fn a_size_exactly_at_a_cap_is_allowed_and_one_byte_past_it_is_not() {
        let at_floor = profile_with(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            caps(Some(1_000), None),
            groups(&[], &[]),
        );
        assert!(
            ReleaseRanker::rank(&candidate(&Parts::baseline(), 1_000), &at_floor)
                .score()
                .is_some()
        );
        assert_eq!(
            ReleaseRanker::rank(&candidate(&Parts::baseline(), 999), &at_floor).rejection(),
            Some(&Rejection::BelowSizeFloor {
                bytes: 999,
                floor: 1_000,
            })
        );

        let at_ceiling = profile_with(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            caps(None, Some(2_000)),
            groups(&[], &[]),
        );
        assert!(
            ReleaseRanker::rank(&candidate(&Parts::baseline(), 2_000), &at_ceiling)
                .score()
                .is_some()
        );
        assert_eq!(
            ReleaseRanker::rank(&candidate(&Parts::baseline(), 2_001), &at_ceiling).rejection(),
            Some(&Rejection::AboveSizeCeiling {
                bytes: 2_001,
                ceiling: 2_000,
            })
        );

        let pinned = profile_with(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            caps(Some(4_096), Some(4_096)),
            groups(&[], &[]),
        );
        assert!(
            ReleaseRanker::rank(&candidate(&Parts::baseline(), 4_096), &pinned)
                .score()
                .is_some()
        );
        assert!(
            ReleaseRanker::rank(&candidate(&Parts::baseline(), 4_095), &pinned)
                .score()
                .is_none()
        );
        assert!(
            ReleaseRanker::rank(&candidate(&Parts::baseline(), 4_097), &pinned)
                .score()
                .is_none()
        );
    }

    #[test]
    fn a_size_just_inside_each_cap_is_allowed() {
        let profile = golden_profile();
        let allowed = Parts {
            group: Some("FRAMESTOR".to_owned()),
            ..Parts::baseline()
        };
        for bytes in [1_000_000_000, 1_000_000_001, 19_999_999_999, 20_000_000_000] {
            let ranking = ReleaseRanker::rank(&candidate(&allowed, bytes), &profile);
            assert!(
                ranking.score().is_some(),
                "{bytes} bytes is inside the caps: {}",
                ranking.describe()
            );
        }
        for bytes in [999_999_999, 20_000_000_001, u64::MAX] {
            let ranking = ReleaseRanker::rank(&candidate(&allowed, bytes), &profile);
            assert!(
                ranking.rejection().is_some(),
                "{bytes} bytes is outside the caps: {}",
                ranking.describe()
            );
        }
    }

    #[test]
    fn a_position_at_the_top_of_a_list_scores_the_ceiling() {
        let profile = golden_profile();
        let parts = Parts {
            resolution: Resolution::UltraHd,
            source: ReleaseSource::Remux,
            video_codec: VideoCodec::Av1,
            audio: vec![AudioCodec::TrueHd],
            group: Some("FRAMESTOR".to_owned()),
        };
        let ranking = ReleaseRanker::rank(&candidate(&parts, 5_000_000_000), &profile);
        assert_eq!(ranking.score(), Some(MAX_SCORE));
    }

    #[test]
    fn the_last_position_of_every_list_loses_exactly_its_family_step() {
        let parts = Parts {
            resolution: Resolution::Sd480,
            source: ReleaseSource::Hdtv,
            video_codec: VideoCodec::H264,
            audio: vec![AudioCodec::Aac],
            group: Some("FRAMESTOR".to_owned()),
        };
        let profile = golden_profile();
        let ranking = ReleaseRanker::rank(&candidate(&parts, 1_000_000_000), &profile);
        assert_eq!(
            ranking.score(),
            Some(
                MAX_SCORE
                    - 3 * RESOLUTION_STEP
                    - 3 * SOURCE_STEP
                    - 2 * VIDEO_CODEC_STEP
                    - 2 * AUDIO_STEP
            )
        );
    }

    #[test]
    fn the_best_declared_audio_codec_is_the_one_that_counts() {
        let profile = golden_profile();
        let parts = Parts {
            audio: vec![AudioCodec::Aac, AudioCodec::TrueHd],
            group: Some("FRAMESTOR".to_owned()),
            ..Parts::baseline()
        };
        let ranking = ReleaseRanker::rank(&candidate(&parts, 5_000_000_000), &profile);
        assert_eq!(
            ranking.reasons()[3],
            RankReason::Audio {
                position: Some(Position::new(0))
            },
            "the highest-ranked declared codec wins: {}",
            ranking.describe()
        );
    }

    #[test]
    fn an_audio_list_with_no_matching_codec_names_the_whole_list() {
        let profile = profile_with(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![AudioCodec::TrueHd, AudioCodec::Flac],
            caps(None, None),
            groups(&[], &[]),
        );
        let parts = Parts {
            audio: vec![AudioCodec::Aac],
            ..Parts::baseline()
        };
        let ranking = ReleaseRanker::rank(&candidate(&parts, 5), &profile);
        assert_eq!(
            ranking.rejection(),
            Some(&Rejection::AudioNotAllowed {
                allowed: vec![AudioCodec::TrueHd, AudioCodec::Flac],
            })
        );
        assert_eq!(ranking.describe(), "rejected=audio-not-allowed:truehd|flac");
    }

    #[test]
    fn a_missing_audio_track_is_rejected_only_when_audio_is_constrained() {
        let tagless = Candidate::new(ReleaseName::parse("Some Random File Name"), 5);
        let constrained = profile_with(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![AudioCodec::Aac],
            caps(None, None),
            groups(&[], &[]),
        );
        assert_eq!(
            ReleaseRanker::rank(&tagless, &constrained).rejection(),
            Some(&Rejection::AudioNotAllowed {
                allowed: vec![AudioCodec::Aac],
            })
        );
        assert_eq!(
            ReleaseRanker::rank(&tagless, &permissive()).score(),
            Some(MAX_SCORE)
        );
    }

    #[test]
    fn the_first_refusing_rule_in_order_is_the_one_reported() {
        let profile = profile_with(
            vec![Resolution::FullHd],
            vec![ReleaseSource::Remux],
            Vec::new(),
            Vec::new(),
            caps(Some(1_000_000_000), None),
            groups(&[], &["YIFY"]),
        );
        let parts = Parts {
            resolution: Resolution::Sd480,
            source: ReleaseSource::Hdtv,
            video_codec: VideoCodec::H264,
            audio: vec![AudioCodec::Aac],
            group: Some("YIFY".to_owned()),
        };
        assert_eq!(
            ReleaseRanker::rank(&candidate(&parts, 1), &profile).rejection(),
            Some(&Rejection::ResolutionNotAllowed {
                value: Resolution::Sd480,
            }),
            "resolution is consulted before source, group, and size"
        );

        let allowed_resolution = Parts {
            resolution: Resolution::FullHd,
            ..parts.clone()
        };
        assert_eq!(
            ReleaseRanker::rank(&candidate(&allowed_resolution, 1), &profile).rejection(),
            Some(&Rejection::SourceNotAllowed {
                value: ReleaseSource::Hdtv,
            })
        );

        let allowed_source = Parts {
            source: ReleaseSource::Remux,
            ..allowed_resolution
        };
        assert_eq!(
            ReleaseRanker::rank(&candidate(&allowed_source, 1), &profile).rejection(),
            Some(&Rejection::GroupBlocked {
                group: "YIFY".to_owned(),
            }),
            "a blocked group outranks the size floor"
        );

        let allowed_group = Parts {
            group: None,
            ..allowed_source
        };
        assert_eq!(
            ReleaseRanker::rank(&candidate(&allowed_group, 999), &profile).rejection(),
            Some(&Rejection::BelowSizeFloor {
                bytes: 999,
                floor: 1_000_000_000,
            })
        );
    }

    #[test]
    fn a_ranking_order_is_score_first_and_ranked_before_rejected() {
        let profile = golden_profile();
        let allowed = Parts {
            group: Some(GOLDEN_GROUP.to_owned()),
            ..Parts::baseline()
        };
        let better = ReleaseRanker::rank(&candidate(&allowed, 5_000_000_000), &profile);
        let worse = ReleaseRanker::rank(
            &candidate(
                &Parts {
                    resolution: Resolution::FullHd,
                    ..allowed
                },
                5_000_000_000,
            ),
            &profile,
        );
        let rejected = ReleaseRanker::rank(
            &candidate(
                &Parts {
                    group: Some("YIFY".to_owned()),
                    ..Parts::baseline()
                },
                5_000_000_000,
            ),
            &profile,
        );
        assert!(
            better
                .score()
                .is_some_and(|score| score > worse.score().unwrap_or(0))
        );
        assert!(
            worse > rejected,
            "a ranked candidate outranks a rejected one"
        );
        assert_eq!(better.cmp(&better), std::cmp::Ordering::Equal);
        let mut all = [rejected.clone(), better.clone(), worse.clone()];
        all.sort();
        assert_eq!(all, [rejected, worse, better]);
    }

    fn resolution_strategy() -> impl Strategy<Value = Resolution> {
        prop::sample::select(
            RESOLUTION_TAG
                .iter()
                .map(|(_, value)| *value)
                .collect::<Vec<_>>(),
        )
    }

    fn source_strategy() -> impl Strategy<Value = ReleaseSource> {
        prop::sample::select(
            SOURCE_TAG
                .iter()
                .map(|(_, value)| *value)
                .collect::<Vec<_>>(),
        )
    }

    fn video_codec_strategy() -> impl Strategy<Value = VideoCodec> {
        prop::sample::select(
            VIDEO_CODEC_TAG
                .iter()
                .map(|(_, value)| *value)
                .collect::<Vec<_>>(),
        )
    }

    fn audio_codec_strategy() -> impl Strategy<Value = AudioCodec> {
        prop::sample::select(
            AUDIO_CODEC_TAG
                .iter()
                .map(|(_, value)| *value)
                .collect::<Vec<_>>(),
        )
    }

    /// Parts with one audio codec at most, which keeps a generated name's audio
    /// field exactly the codec the generator chose.
    fn parts_strategy() -> impl Strategy<Value = Parts> {
        (
            resolution_strategy(),
            source_strategy(),
            video_codec_strategy(),
            audio_codec_strategy(),
            prop::option::of("[A-Za-z]{2,8}".prop_map(String::from)),
        )
            .prop_map(|(resolution, source, video_codec, audio, group)| Parts {
                resolution,
                source,
                video_codec,
                audio: vec![audio],
                group,
            })
    }

    /// A generated list with repeats removed, keeping the order the generator produced.
    fn deduplicated<T: Copy + PartialEq>(values: Vec<T>) -> Vec<T> {
        let mut unique = Vec::with_capacity(values.len());
        for value in values {
            if !unique.contains(&value) {
                unique.push(value);
            }
        }
        unique
    }

    fn rules_strategy() -> impl Strategy<Value = QualityRules> {
        (
            prop::collection::vec(resolution_strategy(), 0..4),
            prop::collection::vec(source_strategy(), 0..4),
            prop::collection::vec(video_codec_strategy(), 0..3),
            prop::collection::vec(audio_codec_strategy(), 0..4),
            (any::<Option<u64>>(), any::<Option<u64>>()),
            (
                prop::collection::vec("[A-Za-z]{2,8}", 0..3),
                prop::collection::vec("[A-Za-z]{2,8}", 0..3),
            ),
        )
            .prop_map(
                |(
                    resolutions,
                    sources,
                    video_codecs,
                    audio_codecs,
                    (first, second),
                    (allowed, blocked),
                )| {
                    let (floor, ceiling) = if first <= second {
                        (first, second)
                    } else {
                        (second, first)
                    };
                    let blocked: Vec<String> = blocked
                        .into_iter()
                        .filter(|group| !allowed.contains(group))
                        .collect();
                    // A preference order cannot name one value twice, so the generator
                    // deduplicates before it builds — a duplicate would make every generated
                    // profile invalid rather than exercising the ranker.
                    QualityRules::new(
                        deduplicated(resolutions),
                        deduplicated(sources),
                        deduplicated(video_codecs),
                        deduplicated(audio_codecs),
                        SizeCaps::new(floor, ceiling).expect("the generator orders the caps"),
                        GroupRules::new(allowed, blocked).expect("the generator removes overlap"),
                    )
                    .expect("the generator deduplicates every preference order")
                },
            )
    }

    fn profile_strategy() -> impl Strategy<Value = QualityProfile> {
        (rules_strategy(), any::<u16>()).prop_map(|(rules, rank)| {
            QualityProfile::new("Generated", rules, QualityRank::new(rank), true, false)
                .expect("the generator produces a valid profile")
        })
    }

    /// The only field of a parts value that a swap may touch, so a monotonicity
    /// case changes exactly one criterion and nothing else.
    #[derive(Debug, Clone, Copy)]
    enum Criterion {
        Resolution,
        Source,
        VideoCodec,
        Audio,
    }

    /// Replaces one criterion with the value the profile ranks directly above the
    /// original, leaving every other criterion alone, or `None` when no single-field
    /// swap moves the criterion the scorer actually sees — either because the
    /// candidate already sits at the top of that family, or because a group spelling
    /// already puts the better codec in the candidate's declared set.
    fn improve(parts: &Parts, criterion: Criterion, profile: &QualityProfile) -> Option<Parts> {
        let rules = profile.rules();
        let mut better = parts.clone();
        match criterion {
            Criterion::Resolution => {
                let at = rules
                    .resolutions()
                    .iter()
                    .position(|value| *value == parts.resolution)
                    .expect("the preference list covers every resolution");
                let above = at.checked_sub(1)?;
                better.resolution = rules.resolutions()[above];
            }
            Criterion::Source => {
                let at = rules
                    .sources()
                    .iter()
                    .position(|value| *value == parts.source)
                    .expect("the preference list covers every source");
                let above = at.checked_sub(1)?;
                better.source = rules.sources()[above];
            }
            Criterion::VideoCodec => {
                let at = rules
                    .video_codecs()
                    .iter()
                    .position(|value| *value == parts.video_codec)
                    .expect("the preference list covers every codec");
                let above = at.checked_sub(1)?;
                better.video_codec = rules.video_codecs()[above];
            }
            Criterion::Audio => {
                // The position to improve is the one the *scorer* sees, which is not the fixture's
                // list: the release parser reads a codec tag anywhere in the name, so a group that
                // happens to spell one — PG-E4-06's counterexample has `group: Some("dd")`, and
                // `dd` is Dolby Digital — puts that codec in the candidate's audio set as well.
                // Reading `parts.audio` here asked the generator to raise a position the scorer
                // was already holding: `[Eac3]` with a group of `dd` scores at `AC3`'s position,
                // and swapping the stated codec for `AC3` changes nothing.
                let declared = name_of(parts).audio().to_vec();
                let at = rules
                    .audio_codecs()
                    .iter()
                    .position(|value| declared.contains(value))
                    .expect("the preference list covers every audio codec");
                let above = at.checked_sub(1)?;
                better.audio = vec![rules.audio_codecs()[above]];
            }
        }
        if better == *parts {
            // The group can spell the very codec the swap would write. The struct is
            // unchanged, so the candidate the scorer sees is unchanged with it, and
            // there is no single-field move to claim.
            return None;
        }
        Some(better)
    }

    /// Rotates a table so the fixture order does not lead, which is what gives every
    /// generated candidate room to improve.
    fn rotated<T: Copy>(table: &[(&str, T)], by: usize) -> Vec<T> {
        let values: Vec<T> = table.iter().map(|(_, value)| *value).collect();
        values[by..].iter().chain(&values[..by]).copied().collect()
    }

    /// A profile that ranks every variant of every family in a fixed, rotated order,
    /// with no size cap and no group rule, so a monotonicity case can improve any one
    /// criterion and stay accepted.
    fn preference_profile() -> QualityProfile {
        profile_with(
            rotated(&RESOLUTION_TAG, 4),
            rotated(&SOURCE_TAG, 9),
            rotated(&VIDEO_CODEC_TAG, 3),
            rotated(&AUDIO_CODEC_TAG, 7),
            caps(None, None),
            groups(&[], &[]),
        )
    }

    /// Parts built only from what the golden profile lists, so a case that must be
    /// scored is never rejected for a tag the profile does not know.
    fn golden_parts_strategy() -> impl Strategy<Value = Parts> {
        (
            prop::sample::select(GOLDEN_RESOLUTIONS.to_vec()),
            prop::sample::select(GOLDEN_SOURCES.to_vec()),
            prop::sample::select(GOLDEN_VIDEO_CODECS.to_vec()),
            prop::sample::select(GOLDEN_AUDIO_CODECS.to_vec()),
        )
            .prop_map(|(resolution, source, video_codec, audio)| Parts {
                resolution,
                source,
                video_codec,
                audio: vec![audio],
                group: Some(GOLDEN_GROUP.to_owned()),
            })
    }

    /// PG-E4-06's counterexample, kept as a test because what it found is surprising and correct,
    /// and the next reader deserves to meet it here rather than in a property failure.
    ///
    /// The scorer's audio position is the best position held by any codec the **name** mentions,
    /// and the release parser reads a codec tag wherever it appears — including in the group. The
    /// counterexample's candidate is `Movie.2024.2160p.REMUX.x264.EAC3-dd`, whose group `dd` is
    /// also how a release name spells Dolby Digital, so the candidate holds `EAC3` *and* `AC3` and
    /// is scored at whichever this profile ranks higher. Under this profile that is `AC3`, which is
    /// why the property's `improve` could not raise the score by swapping the stated codec: the
    /// position it was trying to improve was already held.
    ///
    /// What was wrong was the generator, not the scorer: `improve` read the fixture's own list and
    /// asked for one step better, which is not necessarily a step the *parsed* name can take. It now
    /// measures against the parsed set, and this test holds the model still either way.
    #[test]
    fn a_codec_the_group_spells_still_decides_the_audio_position() {
        let profile = preference_profile();
        let list = profile.rules().audio_codecs();
        let position = |codec: AudioCodec| {
            list.iter()
                .position(|entry| *entry == codec)
                .expect("the preference profile lists every audio codec")
        };
        let ac3 = position(AudioCodec::Ac3);
        let eac3 = position(AudioCodec::Eac3);
        assert!(
            ac3 < eac3,
            "this test's premise is that the fixture profile ranks AC3 above EAC3"
        );

        let named = |group: &str| Parts {
            resolution: Resolution::UltraHd,
            source: ReleaseSource::Remux,
            video_codec: VideoCodec::H264,
            audio: vec![AudioCodec::Eac3],
            group: Some(group.to_owned()),
        };
        let score = |parts: &Parts| {
            let candidate = candidate(parts, 0);
            let ranking = ReleaseRanker::rank(&candidate, &profile);
            let audio = ranking
                .reasons()
                .iter()
                .find(|reason| reason.kind() == ReasonKind::Audio)
                .and_then(RankReason::position)
                .expect("the profile accepts the fixture's audio");
            (
                candidate.name.audio().to_vec(),
                audio.index(),
                ranking.score(),
            )
        };

        let (parsed, at, with_group) = score(&named("dd"));
        let (_, _, without) = score(&named(GOLDEN_GROUP));

        assert_eq!(
            parsed,
            vec![AudioCodec::Eac3, AudioCodec::Ac3],
            "the group's spelling is read as a codec, so the name declares both"
        );
        assert_eq!(
            at,
            u32::try_from(ac3).expect("an index fits"),
            "and the position is the better of the two, not the stated one"
        );
        assert_ne!(
            with_group, without,
            "so the group's spelling reaches the score, not only the candidate's tags"
        );
        assert!(
            with_group > without,
            "and it *helps* here only because this fixture profile happens to rank AC3 above EAC3: \
             the direction is the profile's, not a rule about groups"
        );
        assert_eq!(
            parsed.first(),
            Some(&AudioCodec::Eac3),
            "whatever the position, the stated codec is still the first one parsed"
        );
    }

    proptest! {
        #[test]
        fn the_same_candidate_and_profile_always_rank_the_same(
            parts in parts_strategy(),
            size in any::<u64>(),
            profile in profile_strategy(),
        ) {
            let candidate = candidate(&parts, size);
            let first = ReleaseRanker::rank(&candidate, &profile);
            let second = ReleaseRanker::rank(&candidate, &profile);
            prop_assert_eq!(&first, &second, "the ranker is a pure function");
            prop_assert_eq!(first.describe(), second.describe());
        }

        #[test]
        fn every_combination_of_tags_and_rules_produces_a_ranking(
            parts in parts_strategy(),
            size in any::<u64>(),
            profile in profile_strategy(),
        ) {
            let ranking = ReleaseRanker::rank(&candidate(&parts, size), &profile);
            match &ranking {
                Ranking::Ranked { score, reasons } => {
                    prop_assert!(*score <= MAX_SCORE);
                    prop_assert_eq!(reasons.len(), 6, "one reason per family");
                }
                Ranking::Rejected { rejection } => {
                    prop_assert!(ranking.score().is_none());
                    prop_assert!(!rejection.label().is_empty());
                }
            }
        }

        #[test]
        fn arbitrary_text_never_panics_the_ranker(
            raw in any::<String>(),
            size in any::<u64>(),
            profile in profile_strategy(),
        ) {
            let candidate = Candidate::new(ReleaseName::parse(&raw), size);
            let _ = ReleaseRanker::rank(&candidate, &profile);
        }

        /// Monotonicity, stated exactly: for a candidate the profile accepts and one
        /// criterion of it replaced by the value the profile ranks directly above,
        /// with every other criterion unchanged, the score is strictly higher. Only
        /// the four preference-ordered families are claimed; size cannot be, and the
        /// rustdoc says why.
        #[test]
        fn a_strictly_better_position_strictly_raises_the_score(
            parts in parts_strategy(),
            size in 0_u64..1_000,
            criterion in prop::sample::select(vec![
                Criterion::Resolution,
                Criterion::Source,
                Criterion::VideoCodec,
                Criterion::Audio,
            ]),
        ) {
            let profile = preference_profile();
            let current = ReleaseRanker::rank(&candidate(&parts, size), &profile);
            prop_assert!(current.score().is_some(), "the preference profile accepts every tag");
            let Some(better) = improve(&parts, criterion, &profile) else {
                return Ok(());
            };
            prop_assert_ne!(&better, &parts, "the swap moved the criterion");
            let improved = ReleaseRanker::rank(&candidate(&better, size), &profile);
            prop_assert!(improved.score().is_some());
            prop_assert!(
                improved.score() > current.score(),
                "{:?} must not lose to {:?} for {:?}",
                better,
                parts,
                criterion
            );
        }

        #[test]
        fn size_never_changes_a_score_inside_the_caps(
            parts in golden_parts_strategy(),
            first in GOLDEN_FLOOR..GOLDEN_CEILING,
            second in GOLDEN_FLOOR..GOLDEN_CEILING,
        ) {
            let profile = golden_profile();
            let left = ReleaseRanker::rank(&candidate(&parts, first), &profile);
            let right = ReleaseRanker::rank(&candidate(&parts, second), &profile);
            prop_assert!(left.score().is_some(), "the fixture is built from the profile's lists");
            prop_assert_eq!(left.score(), right.score(), "size is a gate, not a preference");
        }
    }
}
