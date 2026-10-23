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
//!
//! The preference order is explicit, and a fallback never crosses it:
//!
//! 1. **Declared and allowed** — every quality field the profile constrains is in
//!    the name and every value is allowed.
//! 2. **Declared but lower-ranked** — allowed, and loses weighted steps to the
//!    profile's preference order.
//! 3. **Undeclared** — the name does not state a resolution, source, video codec,
//!    or audio codec the profile constrains. Unknown is not disallowed: a release
//!    the indexers actually return must not become unusable because its title
//!    omits a tag, so it is a low-tier fallback that is offered only after every
//!    declared candidate and is marked as such ([`crate::rank::Undeclared`]).
//!
//! A hard refusal is outside that order. A value the profile disallows, a blocked
//! group, or a size outside the caps stays refused even when another field is
//! undeclared: tier 3 is for an unknowable field, never for a known-bad one.

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
    /// The candidate declares a resolution outside the profile's list.
    ResolutionNotAllowed {
        /// The refused resolution.
        value: Resolution,
    },
    /// The candidate declares a source outside the profile's list.
    SourceNotAllowed {
        /// The refused source.
        value: ReleaseSource,
    },
    /// The candidate declares a video codec outside the profile's list.
    VideoCodecNotAllowed {
        /// The refused codec.
        value: VideoCodec,
    },
    /// The profile constrains the audio and the candidate declares a codec outside the list.
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
            Self::ResolutionNotAllowed { .. } => ReasonKind::Resolution,
            Self::SourceNotAllowed { .. } => ReasonKind::Source,
            Self::VideoCodecNotAllowed { .. } => ReasonKind::VideoCodec,
            Self::AudioNotAllowed { .. } => ReasonKind::Audio,
            Self::GroupBlocked { .. } | Self::GroupNotAllowed { .. } => ReasonKind::Group,
            Self::BelowSizeFloor { .. } | Self::AboveSizeCeiling { .. } => ReasonKind::Size,
        }
    }

    /// The stable label the audit trail stores: `REASON_KIND` and an argument when
    /// the refusal has one, with no spaces so a log line stays parseable.
    pub fn label(&self) -> String {
        match self {
            Self::ResolutionNotAllowed { value } => {
                format!("resolution-not-allowed:{}", value.as_str())
            }
            Self::SourceNotAllowed { value } => format!("source-not-allowed:{}", value.as_str()),
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
            Self::ResolutionNotAllowed { .. } => 0,
            Self::SourceNotAllowed { .. } => 1,
            Self::VideoCodecNotAllowed { .. } => 2,
            Self::AudioNotAllowed { .. } => 3,
            Self::GroupBlocked { .. } => 4,
            Self::GroupNotAllowed { .. } => 5,
            Self::BelowSizeFloor { .. } => 6,
            Self::AboveSizeCeiling { .. } => 7,
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

/// The quality fields a release's name does not declare, when the profile constrains
/// them.
///
/// An undeclared field is unknown rather than disallowed, so the candidate is a
/// low-tier fallback instead of a refusal. The fields are only ever set for the
/// quality families whose absence the release name cannot express — the resolution,
/// the source, the video codec, and the audio codec. A missing source says the name
/// does not state the transfer, not that the profile refused it; a source the name
/// *does* state and the profile disallows stays a hard refusal.
#[allow(clippy::struct_excessive_bools)] // Four independent known/unknown facts the audit trail names separately.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Undeclared {
    /// The name states no resolution and the profile constrains the resolution.
    resolution: bool,
    /// The name states no source and the profile constrains the source.
    source: bool,
    /// The name states no video codec and the profile constrains the video codec.
    video_codec: bool,
    /// The name states no audio codec the profile accepts, and the profile
    /// constrains the audio.
    audio: bool,
}

impl Undeclared {
    /// True when no field is undeclared, which is the shape of a declared candidate.
    pub const fn is_empty(self) -> bool {
        !self.resolution && !self.source && !self.video_codec && !self.audio
    }

    /// The undeclared fields, in the ranker's rule order, as the audit trail and the
    /// acquire job's log spell them.
    pub fn fields(self) -> Vec<&'static str> {
        let mut fields = Vec::with_capacity(4);
        if self.resolution {
            fields.push("resolution");
        }
        if self.source {
            fields.push("source");
        }
        if self.video_codec {
            fields.push("video_codec");
        }
        if self.audio {
            fields.push("audio");
        }
        fields
    }
}

/// The outcome of ranking one candidate.
///
/// The comparison is total and states the winner rule in one place: a declared
/// candidate sorts above an undeclared fallback, a fallback sorts above a
/// rejection, and among candidates of one tier the higher score sorts first. Two
/// rankings that tie compare equal, because equal scores are a tie and nothing else
/// orders them.
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
    /// The name does not declare a quality field the profile constrains.
    ///
    /// The candidate is not refused: an unknown value is none of the profile's
    /// business to disallow. It is a low-tier fallback offered only after every
    /// declared candidate, and this variant is what marks it as one.
    Undeclared {
        /// `MAX_SCORE` less the points of the fields the name does declare.
        score: u64,
        /// One entry per rule the ranker could evaluate. A family the name does not
        /// declare is named by `undeclared` instead, because it has no position.
        reasons: Vec<RankReason>,
        /// Which quality fields the name does not declare.
        undeclared: Undeclared,
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
            Self::Ranked { score, .. } | Self::Undeclared { score, .. } => Some(*score),
            Self::Rejected { .. } => None,
        }
    }

    /// Every rule the ranker could evaluate, in the order it consulted them; empty
    /// for a rejection, which stops at the first rule that refuses it. A fallback's
    /// undeclared families are not reasons: [`Ranking::undeclared`] names them.
    pub fn reasons(&self) -> &[RankReason] {
        match self {
            Self::Ranked { reasons, .. } | Self::Undeclared { reasons, .. } => reasons,
            Self::Rejected { .. } => &[],
        }
    }

    /// The refusal, when the profile made one.
    pub const fn rejection(&self) -> Option<&Rejection> {
        match self {
            Self::Ranked { .. } | Self::Undeclared { .. } => None,
            Self::Rejected { rejection } => Some(rejection),
        }
    }

    /// The quality fields the name does not declare, when the profile accepts the
    /// candidate as a fallback.
    pub const fn undeclared(&self) -> Option<Undeclared> {
        match self {
            Self::Undeclared { undeclared, .. } => Some(*undeclared),
            Self::Ranked { .. } | Self::Rejected { .. } => None,
        }
    }

    /// True when this candidate is the low-tier fallback and not a declared one.
    pub const fn is_fallback(&self) -> bool {
        matches!(self, Self::Undeclared { .. })
    }

    /// The audit-trail line for this outcome: the score and every reason, the
    /// fallback marker and the fields it does not declare, or the named rejection.
    /// This is the only place a [`RankReason`] becomes text.
    pub fn describe(&self) -> String {
        match self {
            Self::Ranked { score, reasons } => {
                let reasons: Vec<String> = reasons.iter().map(describe_reason).collect();
                format!("score={score} {}", reasons.join("+"))
            }
            Self::Undeclared {
                score,
                reasons,
                undeclared,
            } => {
                let reasons: Vec<String> = reasons.iter().map(describe_reason).collect();
                let mut line = format!(
                    "fallback=undeclared:{} score={score}",
                    undeclared.fields().join("+")
                );
                if !reasons.is_empty() {
                    line.push(' ');
                    line.push_str(&reasons.join("+"));
                }
                line
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
        // The tier first, so a fallback can never outrank a declared candidate
        // however well its declared fields score.
        match self.tier().cmp(&other.tier()) {
            std::cmp::Ordering::Equal => match (self, other) {
                (Self::Ranked { score: left, .. }, Self::Ranked { score: right, .. })
                | (Self::Undeclared { score: left, .. }, Self::Undeclared { score: right, .. }) => {
                    left.cmp(right)
                }
                (Self::Rejected { rejection: left }, Self::Rejected { rejection: right }) => {
                    left.cmp(right)
                }
                // Unreachable: equal tiers are equal variants. Total rather than
                // panicking because this is an Ord implementation.
                _ => std::cmp::Ordering::Equal,
            },
            ordering => ordering,
        }
    }
}

impl Ranking {
    /// The tier this outcome belongs to: declared, undeclared fallback, or refused.
    const fn tier(&self) -> u8 {
        match self {
            Self::Ranked { .. } => 2,
            Self::Undeclared { .. } => 1,
            Self::Rejected { .. } => 0,
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

/// The index of the best candidate the profile accepts, or None when it refuses all of them.
///
/// The declared candidates come first, so this answers a fallback only when no declared candidate
/// is acceptable. Every admission rule a caller adds on top — a seeder floor, an indexer health
/// check, a size the metadata has not yet confirmed — filters the slice before it reaches this, so
/// the ranking is one rule in one place.
#[must_use]
pub fn best_candidate(candidates: &[Candidate], profile: &QualityProfile) -> Option<usize> {
    ranked_candidates(candidates, profile).into_iter().next()
}

/// The indices of every candidate the profile accepts, best first: every declared candidate, then
/// every undeclared fallback.
///
/// [`best_candidate`] is this function's first element, kept under its own name because most
/// callers want exactly one answer. A caller that has to step past a release the *inspection*
/// gate will refuse — a rule the ranker cannot evaluate, because it needs the destination and
/// the volume — walks this list instead, so one refusal costs one candidate rather than the
/// attempt.
///
/// The two tiers are sorted separately and concatenated, so a fallback never outranks a declared
/// candidate no matter what its declared fields score. The caller can tell the tiers apart with
/// [`Ranking::undeclared`], which is what makes the fallback's use visible in a log and in the
/// audit trail.
#[must_use]
pub fn ranked_candidates(candidates: &[Candidate], profile: &QualityProfile) -> Vec<usize> {
    let mut declared: Vec<(usize, u64)> = Vec::new();
    let mut fallbacks: Vec<(usize, u64)> = Vec::new();
    for (index, candidate) in candidates.iter().enumerate() {
        match ReleaseRanker::rank(candidate, profile) {
            Ranking::Ranked { score, .. } => declared.push((index, score)),
            Ranking::Undeclared { score, .. } => fallbacks.push((index, score)),
            Ranking::Rejected { .. } => {}
        }
    }
    // Ties resolve to the later index, which is what `max_by_key` chose before this existed, so
    // `best_candidate` keeps its exact former answer.
    let best_first = |scored: &mut Vec<(usize, u64)>| {
        scored.sort_by(|left, right| right.1.cmp(&left.1).then(right.0.cmp(&left.0)));
    };
    best_first(&mut declared);
    best_first(&mut fallbacks);
    declared
        .into_iter()
        .chain(fallbacks)
        .map(|(index, _)| index)
        .collect()
}

/// The indices of the candidates the profile refused for a *quality* reason only, best first.
///
/// This is what an alternative offer is made of: releases that exist — a 2160p encode when the
/// profile asks for 1080p — and could be downloaded today if the profile allowed them. A candidate
/// refused for its size, a blocked group, or a codec the profile does not want is not an
/// alternative to the preference; it is simply unwanted, so it is left out.
///
/// Ordered by the resolution's canonical position (highest first) then by index, so an offer lists
/// the best of what exists before the rest.
#[must_use]
pub fn alternative_candidates(candidates: &[Candidate], profile: &QualityProfile) -> Vec<usize> {
    let mut alternatives: Vec<(usize, usize)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            let Ranking::Rejected { rejection } = ReleaseRanker::rank(candidate, profile) else {
                return None;
            };
            if !matches!(
                rejection,
                Rejection::ResolutionNotAllowed { .. } | Rejection::SourceNotAllowed { .. }
            ) {
                return None;
            }
            let position = candidate
                .name
                .resolution()
                .and_then(|value| Resolution::ALL.iter().position(|other| *other == value))
                .unwrap_or(Resolution::ALL.len());
            Some((position, index))
        })
        .collect();
    alternatives.sort_by_key(|(position, index)| (*position, *index));
    alternatives.into_iter().map(|(_, index)| index).collect()
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
    /// Scores a candidate against a profile, marks it as an undeclared fallback, or
    /// names the rule that refuses it.
    pub fn rank(candidate: &Candidate, profile: &QualityProfile) -> Ranking {
        let rules = profile.rules();
        // A field the profile constrains and the name does not declare is collected
        // here rather than refused: unknown is not disallowed. Every other error is a
        // hard refusal and ends the ranking immediately.
        let mut undeclared = Undeclared::default();

        let resolution = match score_resolution(candidate.name.resolution(), rules.resolutions()) {
            Ok(position) => position,
            Err(FieldScore::NotDeclared) => {
                undeclared.resolution = true;
                None
            }
            Err(FieldScore::Refused(rejection)) => return Ranking::Rejected { rejection },
        };
        let source = match score_source(candidate.name.source(), rules.sources()) {
            Ok(position) => position,
            Err(FieldScore::NotDeclared) => {
                undeclared.source = true;
                None
            }
            Err(FieldScore::Refused(rejection)) => return Ranking::Rejected { rejection },
        };
        let video_codec =
            match score_video_codec(candidate.name.video_codec(), rules.video_codecs()) {
                Ok(position) => position,
                Err(FieldScore::NotDeclared) => {
                    undeclared.video_codec = true;
                    None
                }
                Err(FieldScore::Refused(rejection)) => return Ranking::Rejected { rejection },
            };
        let audio = match score_audio(candidate.name.audio(), rules.audio_codecs()) {
            Ok(position) => position,
            Err(FieldScore::NotDeclared) => {
                undeclared.audio = true;
                None
            }
            Err(FieldScore::Refused(rejection)) => return Ranking::Rejected { rejection },
        };
        if let Err(rejection) = check_group(candidate.name.group(), profile) {
            return Ranking::Rejected { rejection };
        }
        if let Err(rejection) = check_size(candidate.size_bytes, rules.size_caps()) {
            return Ranking::Rejected { rejection };
        }

        // One entry per evaluable family, in the order the rules are applied and the
        // weights descend, so the output order never depends on which rules happened
        // to match. Size is a gate, so it contributes a reason and no points. An
        // undeclared family has no position to report and is named by `undeclared`.
        let mut reasons = Vec::with_capacity(6);
        if !undeclared.resolution {
            reasons.push(RankReason::Resolution {
                position: resolution,
            });
        }
        if !undeclared.source {
            reasons.push(RankReason::Source { position: source });
        }
        if !undeclared.video_codec {
            reasons.push(RankReason::VideoCodec {
                position: video_codec,
            });
        }
        if !undeclared.audio {
            reasons.push(RankReason::Audio { position: audio });
        }
        reasons.push(RankReason::Group);
        reasons.push(RankReason::Size {
            bytes: candidate.size_bytes,
        });
        // The subtraction cannot underflow: `listed_position` is the first match in a
        // list whose entries are values of a family with a fixed number of variants,
        // so an index can only reach `MAX_*_INDEX`, which `MAX_SCORE` covers by
        // construction.
        let penalty: u64 = reasons.iter().map(RankReason::points).sum();
        let score = MAX_SCORE - penalty;

        if undeclared.is_empty() {
            Ranking::Ranked { score, reasons }
        } else {
            Ranking::Undeclared {
                score,
                reasons,
                undeclared,
            }
        }
    }
}

/// Why one quality field could not be scored.
///
/// The families the release name can omit — resolution, source, video codec, and
/// audio — separate an unknown value from a refused one. An unknown value makes the
/// candidate a fallback; a refused value ends the ranking.
enum FieldScore {
    /// The profile constrains the field and the name does not declare it.
    NotDeclared,
    /// The name declares a value the profile does not allow.
    Refused(Rejection),
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

/// Scores the resolution, or says whether it is undeclared or refused.
fn score_resolution(
    value: Option<Resolution>,
    list: &[Resolution],
) -> Result<Option<Position>, FieldScore> {
    if list.is_empty() {
        return Ok(None);
    }
    listed_position(list, value.as_ref()).map_or_else(
        || {
            Err(match value {
                Some(value) => FieldScore::Refused(Rejection::ResolutionNotAllowed { value }),
                None => FieldScore::NotDeclared,
            })
        },
        |position| Ok(Some(position)),
    )
}

/// Scores the source, or says whether it is undeclared or refused.
fn score_source(
    value: Option<ReleaseSource>,
    list: &[ReleaseSource],
) -> Result<Option<Position>, FieldScore> {
    if list.is_empty() {
        return Ok(None);
    }
    listed_position(list, value.as_ref()).map_or_else(
        || {
            Err(match value {
                Some(value) => FieldScore::Refused(Rejection::SourceNotAllowed { value }),
                None => FieldScore::NotDeclared,
            })
        },
        |position| Ok(Some(position)),
    )
}

/// Scores the video codec, or says whether it is undeclared or refused.
fn score_video_codec(
    value: Option<VideoCodec>,
    list: &[VideoCodec],
) -> Result<Option<Position>, FieldScore> {
    if list.is_empty() {
        return Ok(None);
    }
    listed_position(list, value.as_ref()).map_or_else(
        || {
            Err(match value {
                Some(value) => FieldScore::Refused(Rejection::VideoCodecNotAllowed { value }),
                None => FieldScore::NotDeclared,
            })
        },
        |position| Ok(Some(position)),
    )
}

/// Scores the audio by its best-matching codec, or says whether it is undeclared or refused.
///
/// A release states several audio codecs — `DTS-HD MA 5.1` also implies plain
/// `DTS` — so the best position any of them holds is the one that counts.
fn score_audio(values: &[AudioCodec], list: &[AudioCodec]) -> Result<Option<Position>, FieldScore> {
    if list.is_empty() {
        return Ok(None);
    }
    if values.is_empty() {
        return Err(FieldScore::NotDeclared);
    }
    values
        .iter()
        .filter_map(|value| list.iter().position(|entry| entry == value))
        .min()
        .and_then(|index| u32::try_from(index).ok())
        .map_or_else(
            || {
                Err(FieldScore::Refused(Rejection::AudioNotAllowed {
                    allowed: list.to_vec(),
                }))
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

    /// The default profile's shape: every quality family constrained, with no size
    /// or group gate in the way. The domain cannot depend on the storage crate that
    /// owns the default's exact lists, so the golden rules stand in for them; every
    /// value the tests use is on the corresponding list.
    fn default_shape() -> QualityProfile {
        profile_with(
            GOLDEN_RESOLUTIONS.to_vec(),
            GOLDEN_SOURCES.to_vec(),
            GOLDEN_VIDEO_CODECS.to_vec(),
            GOLDEN_AUDIO_CODECS.to_vec(),
            caps(None, None),
            groups(&[], &[]),
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
    fn a_configured_rule_marks_a_candidate_that_declares_nothing_as_a_fallback() {
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
        let fallback = ReleaseRanker::rank(&tagless, &rules(vec![Resolution::FullHd]));
        assert_eq!(
            fallback.undeclared(),
            Some(Undeclared {
                resolution: true,
                source: false,
                video_codec: false,
                audio: false,
            }),
            "a missing resolution is unknown, not disallowed"
        );
        assert_eq!(
            fallback.undeclared().map(Undeclared::fields),
            Some(vec!["resolution"])
        );
        assert_eq!(
            fallback.describe(),
            "fallback=undeclared:resolution score=28251 source@-+video_codec@-+audio@-+group+size:5",
            "the audit line marks the fallback and names the field"
        );
        assert_eq!(
            ReleaseRanker::rank(&tagless, &rules(Vec::new())).score(),
            Some(MAX_SCORE),
            "an empty rule accepts a name that declares nothing"
        );
    }

    /// The live Peppa Pig pack's name as the indexer states it: the name states 1080p
    /// and WEB (which parses as `WebDl`), but no video codec and no audio. Only the
    /// fields the profile constrains and the name omits are undeclared; the resolution
    /// and source it does state still score.
    #[test]
    fn a_declared_resolution_with_no_codec_or_audio_is_still_a_fallback() {
        let profile = profile_with(
            vec![Resolution::FullHd, Resolution::Hd],
            vec![ReleaseSource::WebDl, ReleaseSource::BluRay],
            vec![VideoCodec::H264],
            vec![AudioCodec::Aac],
            caps(None, None),
            groups(&[], &[]),
        );
        let candidate = Candidate::new(
            ReleaseName::parse("Peppa Pig Season 1 MULTI 1080p WEB"),
            9_090_000_000,
        );
        let ranking = ReleaseRanker::rank(&candidate, &profile);
        assert_eq!(
            ranking.undeclared(),
            Some(Undeclared {
                resolution: false,
                source: false,
                video_codec: true,
                audio: true,
            }),
            "the declared resolution and source are not undeclared: {}",
            ranking.describe()
        );
        assert_eq!(
            ranking.undeclared().map(Undeclared::fields),
            Some(vec!["video_codec", "audio"])
        );
        assert_eq!(
            ranking.reasons().first(),
            Some(&RankReason::Resolution {
                position: Some(Position::new(0))
            }),
            "the resolution the name does declare still scores"
        );
        assert_eq!(
            ranking.describe(),
            "fallback=undeclared:video_codec+audio score=28251 \
             resolution@0+source@0+group+size:9090000000"
        );
        assert_eq!(
            best_candidate(std::slice::from_ref(&candidate), &profile),
            Some(0),
            "a fallback is selectable when nothing declared exists"
        );
        assert!(
            alternative_candidates(std::slice::from_ref(&candidate), &profile).is_empty(),
            "a fallback is chosen directly, not offered as another quality"
        );
    }

    /// The pack that was refused live: the same Peppa shape with no source token at
    /// all. Missing source is unknown exactly like a missing codec, so every family
    /// the name omits is named and the pack is a selectable fallback rather than a
    /// refusal.
    #[test]
    fn a_pack_that_declares_no_source_is_an_undeclared_fallback() {
        let profile = default_shape();
        let candidate = Candidate::new(
            ReleaseName::parse("Peppa Pig Season 1 MULTI 1080p"),
            9_090_000_000,
        );
        let ranking = ReleaseRanker::rank(&candidate, &profile);
        assert_eq!(
            ranking.undeclared(),
            Some(Undeclared {
                resolution: false,
                source: true,
                video_codec: true,
                audio: true,
            }),
            "the missing source is unknown, not disallowed: {}",
            ranking.describe()
        );
        assert_eq!(
            ranking.undeclared().map(Undeclared::fields),
            Some(vec!["source", "video_codec", "audio"])
        );
        assert_eq!(
            ranking.describe(),
            "fallback=undeclared:source+video_codec+audio score=24155 resolution@1+group+size:9090000000"
        );
        assert_eq!(
            best_candidate(std::slice::from_ref(&candidate), &profile),
            Some(0),
            "the source-less pack is selectable when nothing declared exists"
        );
        assert!(
            alternative_candidates(std::slice::from_ref(&candidate), &profile).is_empty(),
            "a fallback is chosen directly, not offered as another quality"
        );
    }

    /// The actual title behind the live `source-not-declared` refusal: it states a
    /// season but no resolution, source, codec, or audio. Every constrained field is
    /// unknown, so the pack is a fallback and is selectable; none of it is refused.
    #[test]
    fn the_live_source_less_season_pack_is_an_undeclared_fallback() {
        let profile = default_shape();
        let candidate = Candidate::new(ReleaseName::parse("Peppa Pig S01 2 3 4 5"), 8_152_630_768);
        let ranking = ReleaseRanker::rank(&candidate, &profile);
        assert_eq!(
            ranking.undeclared(),
            Some(Undeclared {
                resolution: true,
                source: true,
                video_codec: true,
                audio: true,
            }),
            "every family the name omits is unknown: {}",
            ranking.describe()
        );
        assert_eq!(
            ranking.undeclared().map(Undeclared::fields),
            Some(vec!["resolution", "source", "video_codec", "audio"])
        );
        assert_eq!(
            ranking.describe(),
            "fallback=undeclared:resolution+source+video_codec+audio score=28251 group+size:8152630768"
        );
        assert_eq!(
            best_candidate(std::slice::from_ref(&candidate), &profile),
            Some(0),
            "the live source-less pack is selectable"
        );
    }

    /// A fully declared release wins over the source-less fallback, and a source
    /// the profile disallows is still a hard refusal even when the rest of the name
    /// is undeclared.
    #[test]
    fn a_declared_release_wins_and_a_disallowed_source_stays_refused() {
        let profile = default_shape();
        let fallback = Candidate::new(
            ReleaseName::parse("Peppa Pig Season 1 MULTI 1080p"),
            9_090_000_000,
        );
        let declared = Candidate::new(
            ReleaseName::parse("Show Season 1 1080p BluRay x264 AAC"),
            9_090_000_000,
        );
        assert!(
            matches!(
                ReleaseRanker::rank(&declared, &profile),
                Ranking::Ranked { .. }
            ),
            "every constrained family is declared"
        );
        assert_eq!(
            best_candidate(&[fallback, declared], &profile),
            Some(1),
            "the declared release outranks the source-less fallback"
        );

        let refused = Candidate::new(ReleaseName::parse("Show Season 1 1080p CAM"), 9_090_000_000);
        let ranking = ReleaseRanker::rank(&refused, &profile);
        assert_eq!(
            ranking.rejection(),
            Some(&Rejection::SourceNotAllowed {
                value: ReleaseSource::Cam,
            }),
            "a declared and disallowed source is never a fallback"
        );
        assert!(ranking.undeclared().is_none());
        assert_eq!(
            best_candidate(std::slice::from_ref(&refused), &profile),
            None,
            "the refused release is never selected"
        );
    }

    /// The tier boundary: a declared candidate wins even when the undeclared one is
    /// the better resolution, which is what "never outranking a declared candidate"
    /// means in practice.
    #[test]
    fn every_declared_candidate_sorts_before_every_undeclared_one() {
        let profile = profile_with(
            vec![Resolution::FullHd, Resolution::Hd],
            vec![ReleaseSource::WebDl],
            vec![VideoCodec::H264],
            vec![AudioCodec::Aac],
            caps(None, None),
            groups(&[], &[]),
        );
        let undeclared = Candidate::new(
            ReleaseName::parse("Movie.2024.1080p.WEB-DL-GROUP"),
            5_000_000_000,
        );
        let declared = Candidate::new(
            ReleaseName::parse("Movie.2024.720p.WEB-DL.x264.AAC-GROUP"),
            5_000_000_000,
        );
        assert_eq!(
            ranked_candidates(&[undeclared.clone(), declared.clone()], &profile),
            vec![1, 0],
            "the lower-ranked declared release still outranks the fallback"
        );
        assert_eq!(
            ranked_candidates(&[declared, undeclared], &profile),
            vec![0, 1],
            "and input order cannot change that"
        );
    }

    /// A value the profile disallows is a hard refusal whatever else the name omits.
    #[test]
    fn a_disallowed_field_is_refused_even_when_another_field_is_undeclared() {
        let profile = profile_with(
            vec![Resolution::FullHd],
            vec![ReleaseSource::WebDl],
            vec![VideoCodec::H264],
            vec![AudioCodec::Aac],
            caps(None, Some(5_000_000_000)),
            groups(&[], &["YIFY"]),
        );
        let cases = [
            (
                "Movie.2024.2160p.WEB-DL-GROUP",
                1_000_000_000,
                Rejection::ResolutionNotAllowed {
                    value: Resolution::UltraHd,
                },
            ),
            (
                "Movie.2024.1080p.WEB-DL.x265-GROUP",
                1_000_000_000,
                Rejection::VideoCodecNotAllowed {
                    value: VideoCodec::H265,
                },
            ),
            (
                "Movie.2024.1080p.WEB-DL.x264.AC3-GROUP",
                1_000_000_000,
                Rejection::AudioNotAllowed {
                    allowed: vec![AudioCodec::Aac],
                },
            ),
            (
                "Movie.2024.1080p.WEB-DL-YIFY",
                1_000_000_000,
                Rejection::GroupBlocked {
                    group: "YIFY".to_owned(),
                },
            ),
            (
                "Movie.2024.1080p.WEB-DL-GROUP",
                6_000_000_000,
                Rejection::AboveSizeCeiling {
                    bytes: 6_000_000_000,
                    ceiling: 5_000_000_000,
                },
            ),
        ];
        for (raw, size, expected) in cases {
            let candidate = Candidate::new(ReleaseName::parse(raw), size);
            let ranking = ReleaseRanker::rank(&candidate, &profile);
            assert_eq!(
                ranking.rejection(),
                Some(&expected),
                "{raw} must stay refused"
            );
            assert!(
                ranking.undeclared().is_none(),
                "{raw} must not become a fallback"
            );
        }
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

        // The fallback's wire form keeps the tier and the undeclared fields, so the audit trail
        // can still say why an unknown-quality release was chosen.
        let fallback = ReleaseRanker::rank(
            &Candidate::new(
                ReleaseName::parse("Movie.2024.1080p.BluRay-FRAMESTOR"),
                2_000_000_000,
            ),
            &profile,
        );
        let json = serde_json::to_string(&fallback).expect("a fallback serializes");
        assert_eq!(
            json,
            concat!(
                r#"{"outcome":"undeclared","score":23899,"reasons":["#,
                r#"{"rule":"resolution","position":1},"#,
                r#"{"rule":"source","position":1},"#,
                r#"{"rule":"group"},"#,
                r#"{"rule":"size","bytes":2000000000}],"#,
                r#""undeclared":{"resolution":false,"source":false,"video_codec":true,"audio":true}}"#,
            ),
            "the fallback is marked on the wire, not inferred from a low score"
        );
        assert_eq!(
            serde_json::from_str::<Ranking>(&json).expect("the fallback reads back"),
            fallback
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
            Rejection::ResolutionNotAllowed {
                value: Resolution::Hd,
            },
            Rejection::SourceNotAllowed {
                value: ReleaseSource::Cam,
            },
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
    fn a_missing_audio_track_is_a_fallback_not_a_refusal() {
        let tagless = Candidate::new(ReleaseName::parse("Some Random File Name"), 5);
        let constrained = profile_with(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![AudioCodec::Aac],
            caps(None, None),
            groups(&[], &[]),
        );
        let fallback = ReleaseRanker::rank(&tagless, &constrained);
        assert_eq!(
            fallback.undeclared(),
            Some(Undeclared {
                resolution: false,
                source: false,
                video_codec: false,
                audio: true,
            }),
            "no audio is unknown, not an audio codec the profile disallows"
        );
        assert!(
            fallback.describe().contains("undeclared:audio"),
            "{}",
            fallback.describe()
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
        let fallback = ReleaseRanker::rank(
            &Candidate::new(
                ReleaseName::parse("Movie.2024.1080p.BluRay-FRAMESTOR"),
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
        assert!(fallback.is_fallback());
        assert!(
            better
                .score()
                .is_some_and(|score| score > worse.score().unwrap_or(0))
        );
        assert!(
            worse > fallback,
            "a fallback never outranks a declared candidate, whatever it scores"
        );
        assert!(
            worse > rejected,
            "a ranked candidate outranks a rejected one"
        );
        assert_eq!(better.cmp(&better), std::cmp::Ordering::Equal);
        let mut all = [
            rejected.clone(),
            fallback.clone(),
            better.clone(),
            worse.clone(),
        ];
        all.sort();
        assert_eq!(all, [rejected, fallback, worse, better]);
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
                Ranking::Undeclared {
                    score,
                    reasons,
                    undeclared,
                } => {
                    prop_assert!(*score <= MAX_SCORE);
                    prop_assert!(!undeclared.is_empty());
                    prop_assert_eq!(
                        reasons.len(),
                        6 - undeclared.fields().len(),
                        "an undeclared family has no position to report"
                    );
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
