//! Release quality profiles.
//!
//! A [`QualityProfile`] is the `quality_profile` row: the rules a release'ish
//! candidate is measured against, the size caps, the release-group allow and
//! block lists, and the rank at which upgrading stops. Construction is the only
//! way in, so two rules that cannot both hold — a size floor above its ceiling, or
//! a group that is both allowed and blocked — cannot exist as a value, and
//! `rules_json` cannot smuggle one in either.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::release::{AudioCodec, ReleaseSource, Resolution, VideoCodec};

/// A position in a quality ladder; a lower rank is the better release, matching
/// `release_decision.rank_position`, which numbers the chosen release first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct QualityRank(u16);

impl QualityRank {
    /// Builds a rank from its integer position.
    pub const fn new(position: u16) -> Self {
        Self(position)
    }

    /// The position the database stores in `quality_profile.cutoff_rank`.
    pub const fn position(self) -> u16 {
        self.0
    }
}

/// The byte sizes a release must fall between; an unset end is uncapped.
// The fields are `floor` and `ceiling` rather than `minimum_bytes` and
// `maximum_bytes` because clippy reads a shared `_bytes` suffix as a naming slip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SizeCapsDraft")]
pub struct SizeCaps {
    floor: Option<u64>,
    ceiling: Option<u64>,
}

impl SizeCaps {
    /// Builds caps, rejecting a floor above the ceiling.
    pub fn new(floor: Option<u64>, ceiling: Option<u64>) -> Result<Self, QualityProfileError> {
        if let (Some(floor), Some(ceiling)) = (floor, ceiling)
            && floor > ceiling
        {
            return Err(QualityProfileError::ContradictorySizeCaps { floor, ceiling });
        }
        Ok(Self { floor, ceiling })
    }

    /// The smallest accepted release, when a floor is set.
    pub const fn floor(self) -> Option<u64> {
        self.floor
    }

    /// The largest accepted release, when a ceiling is set.
    pub const fn ceiling(self) -> Option<u64> {
        self.ceiling
    }

    /// True when `bytes` is inside both set caps.
    pub fn allows(self, bytes: u64) -> bool {
        self.floor.is_none_or(|floor| bytes >= floor)
            && self.ceiling.is_none_or(|ceiling| bytes <= ceiling)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SizeCapsDraft {
    floor: Option<u64>,
    ceiling: Option<u64>,
}

impl TryFrom<SizeCapsDraft> for SizeCaps {
    type Error = QualityProfileError;

    fn try_from(draft: SizeCapsDraft) -> Result<Self, Self::Error> {
        Self::new(draft.floor, draft.ceiling)
    }
}

/// Release groups a profile allows and blocks; an empty allow list allows every
/// group that is not blocked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "GroupRulesDraft")]
pub struct GroupRules {
    allowed: Vec<String>,
    blocked: Vec<String>,
}

impl GroupRules {
    /// Builds group rules, rejecting an empty name and a group on both lists.
    pub fn new(allowed: Vec<String>, blocked: Vec<String>) -> Result<Self, QualityProfileError> {
        if allowed
            .iter()
            .chain(blocked.iter())
            .any(|group| group.trim().is_empty())
        {
            return Err(QualityProfileError::EmptyGroup);
        }
        if let Some(group) = allowed
            .iter()
            .find(|group| blocked.iter().any(|blocked| blocked == *group))
        {
            return Err(QualityProfileError::ContradictoryGroup {
                group: group.clone(),
            });
        }
        Ok(Self { allowed, blocked })
    }

    /// The allowed groups, empty when every group is allowed.
    pub fn allowed(&self) -> &[String] {
        &self.allowed
    }

    /// The blocked groups, always rejected.
    pub fn blocked(&self) -> &[String] {
        &self.blocked
    }

    /// True when a release's group passes both lists; `None` is a release with no
    /// group token.
    pub fn permits(&self, group: Option<&str>) -> bool {
        if group.is_some_and(|group| self.blocked.iter().any(|blocked| blocked == group)) {
            return false;
        }
        self.allowed.is_empty()
            || group.is_some_and(|group| self.allowed.iter().any(|allowed| allowed == group))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupRulesDraft {
    allowed: Vec<String>,
    blocked: Vec<String>,
}

impl TryFrom<GroupRulesDraft> for GroupRules {
    type Error = QualityProfileError;

    fn try_from(draft: GroupRulesDraft) -> Result<Self, Self::Error> {
        Self::new(draft.allowed, draft.blocked)
    }
}

/// The spellings of a preference list, in order.
fn spellings(values: impl Iterator<Item = &'static str>) -> Vec<&'static str> {
    values.collect()
}

/// The first spelling that appears more than once, if any.
///
/// A linear scan rather than a set: these lists hold a handful of entries, and this keeps the
/// check allocation-light and the reported value the one the user actually typed twice.
fn first_duplicate(spellings: &[&'static str]) -> Option<&'static str> {
    spellings
        .iter()
        .enumerate()
        .find_map(|(index, value)| spellings[..index].contains(value).then_some(*value))
}

/// The wire form of [`QualityRules`], validated on the way in.
///
/// `Vec` alone cannot refuse a duplicated entry, so deserialization goes through this and the
/// same check the constructor runs. Without it, a hand-edited profile file could carry a
/// preference order the constructor would have rejected.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QualityRulesDraft {
    resolutions: Vec<Resolution>,
    sources: Vec<ReleaseSource>,
    video_codecs: Vec<VideoCodec>,
    audio_codecs: Vec<AudioCodec>,
    size_caps: SizeCaps,
    groups: GroupRules,
}

impl TryFrom<QualityRulesDraft> for QualityRules {
    type Error = QualityProfileError;

    fn try_from(draft: QualityRulesDraft) -> Result<Self, Self::Error> {
        Self::new(
            draft.resolutions,
            draft.sources,
            draft.video_codecs,
            draft.audio_codecs,
            draft.size_caps,
            draft.groups,
        )
    }
}

/// The rules inside a profile: what it accepts and prefers, and its size caps.
// Unknown fields are refused rather than ignored, because a misspelled rule would
// silently widen the profile instead of failing the load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, try_from = "QualityRulesDraft")]
pub struct QualityRules {
    resolutions: Vec<Resolution>,
    sources: Vec<ReleaseSource>,
    video_codecs: Vec<VideoCodec>,
    audio_codecs: Vec<AudioCodec>,
    size_caps: SizeCaps,
    groups: GroupRules,
}

impl QualityRules {
    /// Builds rules from accept lists, caps, and group rules.
    ///
    /// An accept list is a *preference order*, so naming one value twice is a contradiction
    /// rather than a harmless repeat: the second entry can never be reached, and a user who
    /// typed it believes they asked for something. It is refused here and on the
    /// deserialization path, which is the only other way a `QualityRules` comes into being.
    ///
    /// # Errors
    ///
    /// [`QualityProfileError::DuplicatedRule`] when a list names one value twice.
    pub fn new(
        resolutions: Vec<Resolution>,
        sources: Vec<ReleaseSource>,
        video_codecs: Vec<VideoCodec>,
        audio_codecs: Vec<AudioCodec>,
        size_caps: SizeCaps,
        groups: GroupRules,
    ) -> Result<Self, QualityProfileError> {
        let rules = Self {
            resolutions,
            sources,
            video_codecs,
            audio_codecs,
            size_caps,
            groups,
        };
        rules.validate()?;
        Ok(rules)
    }

    /// Refuses a preference order that names one value twice.
    fn validate(&self) -> Result<(), QualityProfileError> {
        for (field, spellings) in [
            (
                "resolution",
                spellings(self.resolutions.iter().map(|value| value.as_str())),
            ),
            (
                "source",
                spellings(self.sources.iter().map(|value| value.as_str())),
            ),
            (
                "video codec",
                spellings(self.video_codecs.iter().map(|value| value.as_str())),
            ),
            (
                "audio codec",
                spellings(self.audio_codecs.iter().map(|value| value.as_str())),
            ),
        ] {
            if let Some(value) = first_duplicate(&spellings) {
                return Err(QualityProfileError::DuplicatedRule { field, value });
            }
        }
        Ok(())
    }

    /// The accepted resolutions, preferred first; empty means no constraint.
    pub fn resolutions(&self) -> &[Resolution] {
        &self.resolutions
    }

    /// The accepted sources, preferred first; empty means no constraint.
    pub fn sources(&self) -> &[ReleaseSource] {
        &self.sources
    }

    /// The accepted video codecs, preferred first; empty means no constraint.
    pub fn video_codecs(&self) -> &[VideoCodec] {
        &self.video_codecs
    }

    /// The accepted audio codecs, preferred first; empty means no constraint.
    pub fn audio_codecs(&self) -> &[AudioCodec] {
        &self.audio_codecs
    }

    /// The size caps every candidate is measured against.
    pub const fn size_caps(&self) -> SizeCaps {
        self.size_caps
    }

    /// The release-group rules.
    pub const fn groups(&self) -> &GroupRules {
        &self.groups
    }
}

/// A named quality policy: what to accept, what to prefer, and when to stop
/// upgrading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "QualityProfileDraft")]
pub struct QualityProfile {
    name: String,
    rules: QualityRules,
    cutoff_rank: QualityRank,
    upgrade_allowed: bool,
    is_default: bool,
}

impl QualityProfile {
    /// Builds a profile, rejecting an empty name.
    pub fn new(
        name: &str,
        rules: QualityRules,
        cutoff_rank: QualityRank,
        upgrade_allowed: bool,
        is_default: bool,
    ) -> Result<Self, QualityProfileError> {
        if name.trim().is_empty() {
            return Err(QualityProfileError::EmptyName);
        }
        Ok(Self {
            name: name.to_owned(),
            rules,
            cutoff_rank,
            upgrade_allowed,
            is_default,
        })
    }

    /// The profile name, unique per installation.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The profile's rules.
    pub const fn rules(&self) -> &QualityRules {
        &self.rules
    }

    /// The rank at which upgrading stops.
    pub const fn cutoff_rank(&self) -> QualityRank {
        self.cutoff_rank
    }

    /// Whether a better release may replace an existing file.
    pub const fn upgrade_allowed(&self) -> bool {
        self.upgrade_allowed
    }

    /// Whether new work uses this profile when the caller names none.
    pub const fn is_default(&self) -> bool {
        self.is_default
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QualityProfileDraft {
    name: String,
    rules: QualityRules,
    cutoff_rank: QualityRank,
    upgrade_allowed: bool,
    is_default: bool,
}

impl TryFrom<QualityProfileDraft> for QualityProfile {
    type Error = QualityProfileError;

    fn try_from(draft: QualityProfileDraft) -> Result<Self, Self::Error> {
        Self::new(
            &draft.name,
            draft.rules,
            draft.cutoff_rank,
            draft.upgrade_allowed,
            draft.is_default,
        )
    }
}

/// Why a quality profile or its rules were rejected.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum QualityProfileError {
    /// The profile name was empty or only whitespace.
    #[error("a quality profile needs a name")]
    EmptyName,
    /// The size floor is above the size ceiling, so no release can satisfy both.
    #[error("size caps contradict: a {floor}-byte floor is above the {ceiling}-byte ceiling")]
    ContradictorySizeCaps {
        /// The rejected floor, in bytes.
        floor: u64,
        /// The rejected ceiling, in bytes.
        ceiling: u64,
    },
    /// One group is on both the allow and the block list.
    #[error("release group `{group}` is both allowed and blocked")]
    ContradictoryGroup {
        /// The group named by both rules.
        group: String,
    },
    /// A group name in a rule was empty or only whitespace.
    #[error("a release group rule needs a name")]
    EmptyGroup,
    /// An accept list named the same value twice.
    #[error(
        "the {field} preference order names `{value}` twice, so the second entry can never be reached"
    )]
    DuplicatedRule {
        /// Which accept list held the duplicate.
        field: &'static str,
        /// The repeated value, as the schema spells it.
        value: &'static str,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const SOURCES: [ReleaseSource; 15] = [
        ReleaseSource::Remux,
        ReleaseSource::BluRay,
        ReleaseSource::BdRip,
        ReleaseSource::BrRip,
        ReleaseSource::WebDl,
        ReleaseSource::WebRip,
        ReleaseSource::Hdtv,
        ReleaseSource::DvdRip,
        ReleaseSource::HdRip,
        ReleaseSource::Sdtv,
        ReleaseSource::Pdtv,
        ReleaseSource::Cam,
        ReleaseSource::Ts,
        ReleaseSource::Tc,
        ReleaseSource::R5,
    ];
    const VIDEO_CODECS: [VideoCodec; 6] = [
        VideoCodec::H264,
        VideoCodec::H265,
        VideoCodec::Av1,
        VideoCodec::Xvid,
        VideoCodec::Divx,
        VideoCodec::Vp9,
    ];
    const AUDIO_CODECS: [AudioCodec; 12] = [
        AudioCodec::Aac,
        AudioCodec::Ac3,
        AudioCodec::Eac3,
        AudioCodec::Dts,
        AudioCodec::DtsHd,
        AudioCodec::DtsX,
        AudioCodec::TrueHd,
        AudioCodec::Atmos,
        AudioCodec::Flac,
        AudioCodec::Mp3,
        AudioCodec::Opus,
        AudioCodec::Pcm,
    ];

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

    fn rules() -> QualityRules {
        QualityRules::new(
            Resolution::ALL.to_vec(),
            SOURCES.to_vec(),
            VIDEO_CODECS.to_vec(),
            AUDIO_CODECS.to_vec(),
            caps(Some(1_000_000), Some(60_000_000_000)),
            groups(&["FRAMESTOR"], &["YIFY"]),
        )
        .expect("the fixture preference orders name no value twice")
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

    #[test]
    fn a_preference_order_that_names_a_value_twice_is_refused() {
        type Case = (
            Vec<Resolution>,
            Vec<ReleaseSource>,
            Vec<VideoCodec>,
            Vec<AudioCodec>,
            &'static str,
            &'static str,
        );
        let cases: [Case; 4] = [
            (
                vec![Resolution::FullHd, Resolution::UltraHd, Resolution::FullHd],
                Vec::new(),
                Vec::new(),
                Vec::new(),
                "resolution",
                "1080p",
            ),
            (
                Vec::new(),
                vec![
                    ReleaseSource::BluRay,
                    ReleaseSource::WebDl,
                    ReleaseSource::BluRay,
                ],
                Vec::new(),
                Vec::new(),
                "source",
                "bluray",
            ),
            (
                Vec::new(),
                Vec::new(),
                vec![VideoCodec::H264, VideoCodec::H265, VideoCodec::H264],
                Vec::new(),
                "video codec",
                "h264",
            ),
            (
                Vec::new(),
                Vec::new(),
                Vec::new(),
                vec![AudioCodec::Aac, AudioCodec::Eac3, AudioCodec::Aac],
                "audio codec",
                "aac",
            ),
        ];

        for (resolutions, sources, video_codecs, audio_codecs, field, value) in cases {
            let error = QualityRules::new(
                resolutions,
                sources,
                video_codecs,
                audio_codecs,
                caps(None, None),
                groups(&[], &[]),
            )
            .expect_err("a preference order cannot name one value twice");
            assert!(
                matches!(
                    error,
                    QualityProfileError::DuplicatedRule { field: named, value: repeated }
                        if named == field && repeated == value
                ),
                "expected the {field} list to be named, got: {error}"
            );
        }
    }

    #[test]
    fn a_repeated_order_is_refused_when_it_is_deserialized() {
        // Built from a real serialization rather than a hand-written literal, so the test
        // cannot pass because the wire shape was guessed wrong.
        let mut value = serde_json::to_value(rules()).expect("a fixture profile serializes");
        let resolutions = value["resolutions"]
            .as_array_mut()
            .expect("the resolutions field is a list");
        let first = resolutions
            .first()
            .cloned()
            .expect("the fixture names at least one resolution");
        resolutions.push(first);

        let error = serde_json::from_value::<QualityRules>(value)
            .expect_err("the wire form must refuse a repeated order");
        assert!(
            error.to_string().contains("twice"),
            "the message should say the value is named twice: {error}"
        );
    }

    #[test]
    fn a_valid_order_survives_the_wire_form() {
        let rules = rules();
        let json = serde_json::to_string(&rules).expect("a profile serializes");
        assert_eq!(
            serde_json::from_str::<QualityRules>(&json).expect("and comes back"),
            rules
        );
    }

    fn profile() -> QualityProfile {
        QualityProfile::new("Balanced", rules(), QualityRank::new(3), true, true)
            .expect("the fixture profile is valid")
    }

    #[test]
    fn ranks_order_by_position_with_the_lowest_first() {
        let ranks = [
            QualityRank::new(9),
            QualityRank::new(0),
            QualityRank::new(4),
            QualityRank::new(1),
        ];
        let mut sorted = ranks;
        sorted.sort();
        assert_eq!(
            sorted.map(QualityRank::position),
            [0, 1, 4, 9],
            "a lower rank is the better release"
        );
        assert!(QualityRank::new(0) < QualityRank::new(1));
        assert_eq!(QualityRank::new(7).position(), 7);
    }

    #[test]
    fn a_rank_serializes_as_a_bare_integer() {
        let json = serde_json::to_string(&QualityRank::new(7)).expect("serializes");
        assert_eq!(json, "7");
        assert_eq!(
            serde_json::from_str::<QualityRank>(&json).expect("deserializes"),
            QualityRank::new(7)
        );
    }

    #[test]
    fn every_rule_value_round_trips_through_json() {
        for resolution in Resolution::ALL {
            let json = serde_json::to_string(&resolution).expect("serializes");
            assert_eq!(json, format!("\"{}\"", resolution.as_str()));
            assert_eq!(
                serde_json::from_str::<Resolution>(&json).expect("deserializes"),
                resolution
            );
        }
        for source in SOURCES {
            let json = serde_json::to_string(&source).expect("serializes");
            assert_eq!(json, format!("\"{}\"", source.as_str()));
            assert_eq!(
                serde_json::from_str::<ReleaseSource>(&json).expect("deserializes"),
                source
            );
        }
        for codec in VIDEO_CODECS {
            let json = serde_json::to_string(&codec).expect("serializes");
            assert_eq!(json, format!("\"{}\"", codec.as_str()));
            assert_eq!(
                serde_json::from_str::<VideoCodec>(&json).expect("deserializes"),
                codec
            );
        }
        for codec in AUDIO_CODECS {
            let json = serde_json::to_string(&codec).expect("serializes");
            assert_eq!(json, format!("\"{}\"", codec.as_str()));
            assert_eq!(
                serde_json::from_str::<AudioCodec>(&json).expect("deserializes"),
                codec
            );
        }
    }

    #[test]
    fn a_profile_round_trips_through_json() {
        let original = profile();
        let json = serde_json::to_string(&original).expect("serializes");
        let parsed: QualityProfile = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(parsed, original);
        assert_eq!(parsed.name(), "Balanced");
        assert_eq!(parsed.cutoff_rank(), QualityRank::new(3));
        assert!(parsed.upgrade_allowed());
        assert!(parsed.is_default());
        assert_eq!(parsed.rules().resolutions(), Resolution::ALL);
        assert_eq!(
            parsed.rules().size_caps(),
            caps(Some(1_000_000), Some(60_000_000_000))
        );
        assert_eq!(parsed.rules().groups().allowed(), ["FRAMESTOR"]);
        assert_eq!(parsed.rules().groups().blocked(), ["YIFY"]);
    }

    #[test]
    fn rules_json_round_trips_on_its_own() {
        let original = rules();
        let json = serde_json::to_string(&original).expect("serializes");
        let parsed: QualityRules = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(parsed, original);
    }

    #[test]
    fn contradictory_size_caps_are_rejected_at_construction() {
        assert_eq!(
            SizeCaps::new(Some(5_000), Some(1_000)),
            Err(QualityProfileError::ContradictorySizeCaps {
                floor: 5_000,
                ceiling: 1_000,
            })
        );
    }

    #[test]
    fn a_floor_equal_to_the_ceiling_is_not_a_contradiction() {
        let equal = caps(Some(1_000), Some(1_000));
        assert!(equal.allows(1_000));
        assert!(!equal.allows(999));
        assert!(!equal.allows(1_001));
    }

    #[test]
    fn contradictory_group_rules_are_rejected_at_construction() {
        assert_eq!(
            GroupRules::new(vec!["SPARKS".to_owned()], vec!["SPARKS".to_owned()]),
            Err(QualityProfileError::ContradictoryGroup {
                group: "SPARKS".to_owned(),
            })
        );
    }

    #[test]
    fn an_empty_group_name_is_rejected() {
        assert_eq!(
            GroupRules::new(vec!["  ".to_owned()], Vec::new()),
            Err(QualityProfileError::EmptyGroup)
        );
        assert_eq!(
            GroupRules::new(Vec::new(), vec![String::new()]),
            Err(QualityProfileError::EmptyGroup)
        );
    }

    #[test]
    fn an_empty_profile_name_is_rejected() {
        assert_eq!(
            QualityProfile::new("   ", rules(), QualityRank::new(0), true, false),
            Err(QualityProfileError::EmptyName)
        );
    }

    #[test]
    fn contradictory_rules_are_rejected_during_deserialization() {
        let json = r#"{
            "name": "Broken",
            "rules": {
                "resolutions": ["1080p"],
                "sources": ["web-dl"],
                "video_codecs": ["h264"],
                "audio_codecs": ["aac"],
                "size_caps": { "floor": 9000, "ceiling": 1000 },
                "groups": { "allowed": [], "blocked": [] }
            },
            "cutoff_rank": 0,
            "upgrade_allowed": true,
            "is_default": false
        }"#;
        let error = serde_json::from_str::<QualityProfile>(json).expect_err("caps contradict");
        assert!(
            error.to_string().contains("size caps contradict"),
            "the typed reason survives the serde layer: {error}"
        );
    }

    #[test]
    fn a_group_on_both_lists_is_rejected_during_deserialization() {
        let json = r#"{
            "name": "Broken",
            "rules": {
                "resolutions": [],
                "sources": [],
                "video_codecs": [],
                "audio_codecs": [],
                "size_caps": { "floor": null, "ceiling": null },
                "groups": { "allowed": ["GROUP"], "blocked": ["GROUP"] }
            },
            "cutoff_rank": 0,
            "upgrade_allowed": true,
            "is_default": false
        }"#;
        let error = serde_json::from_str::<QualityProfile>(json).expect_err("groups contradict");
        assert!(
            error.to_string().contains("both allowed and blocked"),
            "the typed reason survives the serde layer: {error}"
        );
    }

    #[test]
    fn a_missing_rule_field_is_rejected() {
        let json = r#"{
            "name": "Broken",
            "rules": { "resolutions": [], "sources": [], "video_codecs": [], "audio_codecs": [] },
            "cutoff_rank": 0,
            "upgrade_allowed": true,
            "is_default": false
        }"#;
        assert!(serde_json::from_str::<QualityProfile>(json).is_err());
    }

    #[test]
    fn a_misspelled_rule_field_is_rejected_rather_than_ignored() {
        let json = r#"{
            "name": "Broken",
            "rules": {
                "resolution": ["1080p"],
                "sources": [],
                "video_codecs": [],
                "audio_codecs": [],
                "size_caps": { "floor": null, "ceiling": null },
                "groups": { "allowed": [], "blocked": [] }
            },
            "cutoff_rank": 0,
            "upgrade_allowed": true,
            "is_default": false
        }"#;
        assert!(serde_json::from_str::<QualityProfile>(json).is_err());
    }

    #[test]
    fn an_empty_accept_list_means_no_constraint() {
        let unrestricted = QualityRules::new(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            caps(None, None),
            groups(&[], &[]),
        )
        .expect("empty preference orders name nothing twice");
        assert!(
            unrestricted.resolutions().is_empty(),
            "a profile with no resolution rule accepts every resolution"
        );
        assert!(unrestricted.size_caps().allows(0));
        assert!(unrestricted.size_caps().allows(u64::MAX));
        assert!(unrestricted.groups().permits(Some("ANY")));
        assert!(unrestricted.groups().permits(None));
    }

    fn resolution_strategy() -> impl Strategy<Value = Resolution> {
        prop::sample::select(Resolution::ALL.to_vec())
    }

    fn source_strategy() -> impl Strategy<Value = ReleaseSource> {
        prop::sample::select(SOURCES.to_vec())
    }

    fn video_codec_strategy() -> impl Strategy<Value = VideoCodec> {
        prop::sample::select(VIDEO_CODECS.to_vec())
    }

    fn audio_codec_strategy() -> impl Strategy<Value = AudioCodec> {
        prop::sample::select(AUDIO_CODECS.to_vec())
    }

    fn group_strategy() -> impl Strategy<Value = String> {
        "[A-Za-z]{2,8}"
    }

    fn rules_strategy() -> impl Strategy<Value = QualityRules> {
        (
            prop::collection::vec(resolution_strategy(), 0..4),
            prop::collection::vec(source_strategy(), 0..4),
            prop::collection::vec(video_codec_strategy(), 0..3),
            prop::collection::vec(audio_codec_strategy(), 0..4),
            (any::<Option<u64>>(), any::<Option<u64>>()),
            (
                prop::collection::vec(group_strategy(), 0..3),
                prop::collection::vec(group_strategy(), 0..3),
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
                    // deduplicates before it builds: without this every generated profile
                    // would fail the very rule these tests exist to exercise.
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
        (
            "[A-Za-z][A-Za-z0-9]{0,23}",
            rules_strategy(),
            any::<u16>(),
            any::<bool>(),
            any::<bool>(),
        )
            .prop_map(|(name, rules, rank, upgrade_allowed, is_default)| {
                QualityProfile::new(
                    &name,
                    rules,
                    QualityRank::new(rank),
                    upgrade_allowed,
                    is_default,
                )
                .expect("the generator produces a valid profile")
            })
    }

    proptest! {
        #[test]
        fn a_profile_round_trips_through_json_for_generated_input(original in profile_strategy()) {
            let json = serde_json::to_string(&original).expect("serializes");
            let parsed: QualityProfile = serde_json::from_str(&json).expect("deserializes");
            prop_assert_eq!(parsed, original);
        }

        #[test]
        fn deserializing_arbitrary_text_never_panics(input in any::<String>()) {
            let _ = serde_json::from_str::<QualityProfile>(&input);
            let _ = serde_json::from_str::<QualityRules>(&input);
        }

        #[test]
        fn size_caps_allow_exactly_the_bounded_range(
            floor in any::<Option<u64>>(),
            ceiling in any::<Option<u64>>(),
            bytes in any::<u64>(),
        ) {
            let (floor, ceiling) = if floor <= ceiling { (floor, ceiling) } else { (ceiling, floor) };
            let caps = SizeCaps::new(floor, ceiling).expect("the bounds are ordered");
            let expected = floor.is_none_or(|floor| bytes >= floor)
                && ceiling.is_none_or(|ceiling| bytes <= ceiling);
            prop_assert_eq!(caps.allows(bytes), expected);
        }

        #[test]
        fn widening_the_size_caps_never_removes_an_allowed_size(
            floor in any::<u64>(),
            ceiling in any::<u64>(),
            slack in 0_u64..1_000,
            bytes in any::<u64>(),
        ) {
            let (floor, ceiling) = if floor <= ceiling { (floor, ceiling) } else { (ceiling, floor) };
            let before = SizeCaps::new(Some(floor), Some(ceiling)).expect("ordered");
            let after = SizeCaps::new(
                Some(floor.saturating_sub(slack)),
                Some(ceiling.saturating_add(slack)),
            )
            .expect("widening keeps the bounds ordered");
            if before.allows(bytes) {
                prop_assert!(after.allows(bytes));
            }
        }

        #[test]
        fn group_rules_permit_exactly_the_documented_lists(
            allowed in prop::collection::vec(group_strategy(), 0..4),
            blocked in prop::collection::vec(group_strategy(), 0..4),
            group in group_strategy(),
        ) {
            let blocked: Vec<String> = blocked
                .into_iter()
                .filter(|blocked| !allowed.contains(blocked))
                .collect();
            let rules = GroupRules::new(allowed.clone(), blocked.clone()).expect("overlap removed");
            let expected = !blocked.contains(&group)
                && (allowed.is_empty() || allowed.contains(&group));
            prop_assert_eq!(rules.permits(Some(&group)), expected);
            prop_assert_eq!(rules.permits(None), allowed.is_empty());
        }

        #[test]
        fn rank_order_matches_the_numeric_position(first in any::<u16>(), second in any::<u16>()) {
            let left = QualityRank::new(first);
            let right = QualityRank::new(second);
            prop_assert_eq!(left.cmp(&right), first.cmp(&second));
        }
    }
}
