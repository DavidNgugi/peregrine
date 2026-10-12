//! Release-name parsing.
//!
//! Parsing is infallible and leaves unknown fields blank, because
//! `download_job` stores every parsed column as nullable and keeps
//! `release_title` as the raw text.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Vertical resolution a release declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Resolution {
    /// `2160p`, `4K`, or `UHD`.
    #[serde(rename = "2160p")]
    UltraHd,
    /// `1440p`.
    #[serde(rename = "1440p")]
    QuadHd,
    /// `1080p`.
    #[serde(rename = "1080p")]
    FullHd,
    /// `1080i`.
    #[serde(rename = "1080i")]
    FullHdInterlaced,
    /// `720p`.
    #[serde(rename = "720p")]
    Hd,
    /// `576p`.
    #[serde(rename = "576p")]
    Sd576,
    /// `480p`.
    #[serde(rename = "480p")]
    Sd480,
}

impl Resolution {
    /// Every resolution, highest first.
    pub const ALL: [Self; 7] = [
        Self::UltraHd,
        Self::QuadHd,
        Self::FullHd,
        Self::FullHdInterlaced,
        Self::Hd,
        Self::Sd576,
        Self::Sd480,
    ];

    /// The `<height>p` label the database stores.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UltraHd => "2160p",
            Self::QuadHd => "1440p",
            Self::FullHd => "1080p",
            Self::FullHdInterlaced => "1080i",
            Self::Hd => "720p",
            Self::Sd576 => "576p",
            Self::Sd480 => "480p",
        }
    }
}

/// Where the video came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReleaseSource {
    /// `REMUX` or `BDREMUX`.
    #[serde(rename = "remux")]
    Remux,
    /// `BluRay` or `BLU-RAY`.
    #[serde(rename = "bluray")]
    BluRay,
    /// `BDRip`.
    #[serde(rename = "bdrip")]
    BdRip,
    /// `BRRip`.
    #[serde(rename = "brrip")]
    BrRip,
    /// `WEB-DL`, `WEBDL`, or `WEB`.
    #[serde(rename = "web-dl")]
    WebDl,
    /// `WEBRip`.
    #[serde(rename = "webrip")]
    WebRip,
    /// `HDTV`.
    #[serde(rename = "hdtv")]
    Hdtv,
    /// `DVDRip`.
    #[serde(rename = "dvdrip")]
    DvdRip,
    /// `HDRip`.
    #[serde(rename = "hdrip")]
    HdRip,
    /// `SDTV`.
    #[serde(rename = "sdtv")]
    Sdtv,
    /// `PDTV`.
    #[serde(rename = "pdtv")]
    Pdtv,
    /// `CAM`.
    #[serde(rename = "cam")]
    Cam,
    /// `TS` (telesync).
    #[serde(rename = "ts")]
    Ts,
    /// `TC` (telecine).
    #[serde(rename = "tc")]
    Tc,
    /// `R5`.
    #[serde(rename = "r5")]
    R5,
}

impl ReleaseSource {
    /// The database label, also the token's canonical spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Remux => "remux",
            Self::BluRay => "bluray",
            Self::BdRip => "bdrip",
            Self::BrRip => "brrip",
            Self::WebDl => "web-dl",
            Self::WebRip => "webrip",
            Self::Hdtv => "hdtv",
            Self::DvdRip => "dvdrip",
            Self::HdRip => "hdrip",
            Self::Sdtv => "sdtv",
            Self::Pdtv => "pdtv",
            Self::Cam => "cam",
            Self::Ts => "ts",
            Self::Tc => "tc",
            Self::R5 => "r5",
        }
    }

    /// Quality rank, best first, used when a name carries two source tokens.
    const fn rank(self) -> u8 {
        match self {
            Self::Remux => 0,
            Self::BluRay => 1,
            Self::BdRip => 2,
            Self::BrRip => 3,
            Self::WebDl => 4,
            Self::WebRip => 5,
            Self::Hdtv => 6,
            Self::DvdRip => 7,
            Self::HdRip => 8,
            Self::Sdtv => 9,
            Self::Pdtv => 10,
            Self::Cam => 11,
            Self::Ts => 12,
            Self::Tc => 13,
            Self::R5 => 14,
        }
    }
}

/// Video codec family a release declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VideoCodec {
    /// `x264`, `H.264`, or `AVC`.
    #[serde(rename = "h264")]
    H264,
    /// `x265`, `H.265`, or `HEVC`.
    #[serde(rename = "h265")]
    H265,
    /// `AV1`.
    #[serde(rename = "av1")]
    Av1,
    /// `XviD`.
    #[serde(rename = "xvid")]
    Xvid,
    /// `DivX`.
    #[serde(rename = "divx")]
    Divx,
    /// `VP9`.
    #[serde(rename = "vp9")]
    Vp9,
}

impl VideoCodec {
    /// The database label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::H265 => "h265",
            Self::Av1 => "av1",
            Self::Xvid => "xvid",
            Self::Divx => "divx",
            Self::Vp9 => "vp9",
        }
    }
}

/// High dynamic range format, matching the `media_file.hdr_format` constraint.
// `Deserialize` is here as well as `Serialize` because a media file's row is read back into a
// value, and the constraint has to be enforced on that path too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HdrFormat {
    /// Standard dynamic range.
    #[serde(rename = "sdr")]
    Sdr,
    /// `HDR10`.
    #[serde(rename = "hdr10")]
    Hdr10,
    /// `HDR10+`.
    #[serde(rename = "hdr10_plus")]
    Hdr10Plus,
    /// Hybrid log-gamma.
    #[serde(rename = "hlg")]
    Hlg,
    /// Dolby Vision.
    #[serde(rename = "dolby_vision")]
    DolbyVision,
}

impl HdrFormat {
    /// Every format, in the order the database constraint lists them.
    pub const ALL: [Self; 5] = [
        Self::Sdr,
        Self::Hdr10,
        Self::Hdr10Plus,
        Self::Hlg,
        Self::DolbyVision,
    ];

    /// The database label, which is also the `hdr_format` `CHECK` value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sdr => "sdr",
            Self::Hdr10 => "hdr10",
            Self::Hdr10Plus => "hdr10_plus",
            Self::Hlg => "hlg",
            Self::DolbyVision => "dolby_vision",
        }
    }

    /// Precedence, most descriptive first, used when a name carries two tokens.
    const fn precedence(self) -> u8 {
        match self {
            Self::DolbyVision => 0,
            Self::Hdr10Plus => 1,
            Self::Hdr10 => 2,
            Self::Hlg => 3,
            Self::Sdr => 4,
        }
    }
}

/// Audio codec a release declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AudioCodec {
    /// `AAC`.
    #[serde(rename = "aac")]
    Aac,
    /// `AC3` or `DD`.
    #[serde(rename = "ac3")]
    Ac3,
    /// `EAC3`, `DDP`, or `DD+`.
    #[serde(rename = "eac3")]
    Eac3,
    /// `DTS`.
    #[serde(rename = "dts")]
    Dts,
    /// `DTS-HD`.
    #[serde(rename = "dts-hd")]
    DtsHd,
    /// `DTS:X`.
    #[serde(rename = "dts-x")]
    DtsX,
    /// `TrueHD`.
    #[serde(rename = "truehd")]
    TrueHd,
    /// `Atmos`.
    #[serde(rename = "atmos")]
    Atmos,
    /// `FLAC`.
    #[serde(rename = "flac")]
    Flac,
    /// `MP3`.
    #[serde(rename = "mp3")]
    Mp3,
    /// `Opus`.
    #[serde(rename = "opus")]
    Opus,
    /// `PCM` or `LPCM`.
    #[serde(rename = "pcm")]
    Pcm,
}

impl AudioCodec {
    /// The database label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Aac => "aac",
            Self::Ac3 => "ac3",
            Self::Eac3 => "eac3",
            Self::Dts => "dts",
            Self::DtsHd => "dts-hd",
            Self::DtsX => "dts-x",
            Self::TrueHd => "truehd",
            Self::Atmos => "atmos",
            Self::Flac => "flac",
            Self::Mp3 => "mp3",
            Self::Opus => "opus",
            Self::Pcm => "pcm",
        }
    }
}

/// A marker that supersedes an earlier release of the same title.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum RepackKind {
    /// `PROPER`.
    #[serde(rename = "PROPER")]
    Proper,
    /// `REPACK`.
    #[serde(rename = "REPACK")]
    Repack,
    /// `REAL`.
    #[serde(rename = "REAL")]
    Real,
    /// `RERIP`.
    #[serde(rename = "RERIP")]
    Rerip,
    /// `v2`, `v3`, and so on.
    #[serde(rename = "version")]
    Version(u32),
}

impl fmt::Display for RepackKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Proper => formatter.write_str("PROPER"),
            Self::Repack => formatter.write_str("REPACK"),
            Self::Real => formatter.write_str("REAL"),
            Self::Rerip => formatter.write_str("RERIP"),
            Self::Version(number) => write!(formatter, "v{number}"),
        }
    }
}

/// The episodes a release carries, as a closed range ordered low to high.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct EpisodeSpan {
    first: u32,
    last: u32,
}

impl EpisodeSpan {
    /// Builds a span from two episode numbers, ordering them.
    pub const fn new(a: u32, b: u32) -> Self {
        if a <= b {
            Self { first: a, last: b }
        } else {
            Self { first: b, last: a }
        }
    }

    /// The lowest episode in the span.
    pub const fn first(self) -> u32 {
        self.first
    }

    /// The highest episode in the span.
    pub const fn last(self) -> u32 {
        self.last
    }

    /// True when the span covers more than one episode.
    pub const fn is_multi(self) -> bool {
        self.last > self.first
    }

    /// How many episodes the span covers.
    pub const fn episode_count(self) -> u32 {
        self.last.saturating_sub(self.first).saturating_add(1)
    }

    /// Grows the span to include `other`.
    const fn union(self, other: Self) -> Self {
        Self {
            first: if self.first < other.first {
                self.first
            } else {
                other.first
            },
            last: if self.last > other.last {
                self.last
            } else {
                other.last
            },
        }
    }
}

/// A parsed release title, with the raw text kept for auditability.
// Serialization only: a derived `Deserialize` could build a release whose fields
// disagree with each other, so `parse` stays the one way in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReleaseName {
    raw: String,
    resolution: Option<Resolution>,
    source: Option<ReleaseSource>,
    video_codec: Option<VideoCodec>,
    hdr: Option<HdrFormat>,
    audio: Vec<AudioCodec>,
    group: Option<String>,
    repack: Option<RepackKind>,
    season: Option<u32>,
    episodes: Option<EpisodeSpan>,
    season_pack: bool,
}

impl ReleaseName {
    /// Parses a release title. Unknown fields stay blank; this never fails.
    pub fn parse(raw: &str) -> Self {
        let bytes = raw.as_bytes();
        let mut masked = bytes.to_vec();

        let scanned = scan_seasons_and_episodes(bytes);
        for (start, end) in &scanned.spans {
            mask_range(&mut masked, *start, *end);
        }

        let mut fields = ReleaseFields::default();
        scan_fields(bytes, &mut masked, &mut fields);

        let repack = scan_repack(bytes, &mut masked);
        let group = extract_group(raw, &masked);

        let season_pack =
            scanned.episodes.is_none() && (scanned.season.is_some() || scanned.complete);

        Self {
            raw: raw.to_owned(),
            resolution: fields.resolution,
            source: fields.source,
            video_codec: fields.video_codec,
            hdr: fields.hdr,
            audio: fields.audio,
            group,
            repack,
            season: scanned.season,
            episodes: scanned.episodes,
            season_pack,
        }
    }

    /// The unmodified input.
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// The declared resolution, if any.
    pub const fn resolution(&self) -> Option<Resolution> {
        self.resolution
    }

    /// The best-ranked declared source, if any.
    pub const fn source(&self) -> Option<ReleaseSource> {
        self.source
    }

    /// The declared video codec, if any.
    pub const fn video_codec(&self) -> Option<VideoCodec> {
        self.video_codec
    }

    /// The declared HDR format, if any.
    pub const fn hdr(&self) -> Option<HdrFormat> {
        self.hdr
    }

    /// The declared audio codecs, in the order they appear.
    pub fn audio(&self) -> &[AudioCodec] {
        &self.audio
    }

    /// The release group, when one is attached to a recognized tag.
    pub fn group(&self) -> Option<&str> {
        self.group.as_deref()
    }

    /// The superseding marker, if any.
    pub const fn repack(&self) -> Option<RepackKind> {
        self.repack
    }

    /// The season number, if any.
    pub const fn season(&self) -> Option<u32> {
        self.season
    }

    /// The episode span, if the release names episodes.
    pub const fn episodes(&self) -> Option<EpisodeSpan> {
        self.episodes
    }

    /// True when the release carries a whole season rather than single episodes.
    pub const fn is_season_pack(&self) -> bool {
        self.season_pack
    }
}

#[derive(Debug, Default)]
struct ReleaseFields {
    resolution: Option<Resolution>,
    source: Option<ReleaseSource>,
    video_codec: Option<VideoCodec>,
    hdr: Option<HdrFormat>,
    audio: Vec<AudioCodec>,
}

#[derive(Debug, Default)]
struct SeasonScan {
    season: Option<u32>,
    episodes: Option<EpisodeSpan>,
    complete: bool,
    spans: Vec<(usize, usize)>,
}

#[derive(Clone, Copy)]
enum Field {
    Resolution(Resolution),
    Source(ReleaseSource),
    Codec(VideoCodec),
    Hdr(HdrFormat),
    Audio(AudioCodec),
}

struct Pattern {
    text: &'static str,
    field: Field,
}

/// Every recognized tag, longest first so a more specific spelling wins.
const PATTERNS: &[Pattern] = &[
    Pattern {
        text: "dolby.vision",
        field: Field::Hdr(HdrFormat::DolbyVision),
    },
    Pattern {
        text: "dolby vision",
        field: Field::Hdr(HdrFormat::DolbyVision),
    },
    Pattern {
        text: "dolbyvision",
        field: Field::Hdr(HdrFormat::DolbyVision),
    },
    Pattern {
        text: "hdr10plus",
        field: Field::Hdr(HdrFormat::Hdr10Plus),
    },
    Pattern {
        text: "bdremux",
        field: Field::Source(ReleaseSource::Remux),
    },
    Pattern {
        text: "blu-ray",
        field: Field::Source(ReleaseSource::BluRay),
    },
    Pattern {
        text: "web-dl",
        field: Field::Source(ReleaseSource::WebDl),
    },
    Pattern {
        text: "webrip",
        field: Field::Source(ReleaseSource::WebRip),
    },
    Pattern {
        text: "hdr10+",
        field: Field::Hdr(HdrFormat::Hdr10Plus),
    },
    Pattern {
        text: "dts-hd",
        field: Field::Audio(AudioCodec::DtsHd),
    },
    Pattern {
        text: "bluray",
        field: Field::Source(ReleaseSource::BluRay),
    },
    Pattern {
        text: "truehd",
        field: Field::Audio(AudioCodec::TrueHd),
    },
    Pattern {
        text: "dvdrip",
        field: Field::Source(ReleaseSource::DvdRip),
    },
    Pattern {
        text: "brrip",
        field: Field::Source(ReleaseSource::BrRip),
    },
    Pattern {
        text: "bdrip",
        field: Field::Source(ReleaseSource::BdRip),
    },
    Pattern {
        text: "remux",
        field: Field::Source(ReleaseSource::Remux),
    },
    Pattern {
        text: "webdl",
        field: Field::Source(ReleaseSource::WebDl),
    },
    Pattern {
        text: "h.265",
        field: Field::Codec(VideoCodec::H265),
    },
    Pattern {
        text: "h.264",
        field: Field::Codec(VideoCodec::H264),
    },
    Pattern {
        text: "dtshd",
        field: Field::Audio(AudioCodec::DtsHd),
    },
    Pattern {
        text: "dts:x",
        field: Field::Audio(AudioCodec::DtsX),
    },
    Pattern {
        text: "dts.x",
        field: Field::Audio(AudioCodec::DtsX),
    },
    Pattern {
        text: "dts-x",
        field: Field::Audio(AudioCodec::DtsX),
    },
    Pattern {
        text: "hdr10",
        field: Field::Hdr(HdrFormat::Hdr10),
    },
    Pattern {
        text: "2160p",
        field: Field::Resolution(Resolution::UltraHd),
    },
    Pattern {
        text: "1440p",
        field: Field::Resolution(Resolution::QuadHd),
    },
    Pattern {
        text: "1080p",
        field: Field::Resolution(Resolution::FullHd),
    },
    Pattern {
        text: "1080i",
        field: Field::Resolution(Resolution::FullHdInterlaced),
    },
    Pattern {
        text: "hdrip",
        field: Field::Source(ReleaseSource::HdRip),
    },
    Pattern {
        text: "atmos",
        field: Field::Audio(AudioCodec::Atmos),
    },
    Pattern {
        text: "hevc",
        field: Field::Codec(VideoCodec::H265),
    },
    Pattern {
        text: "x265",
        field: Field::Codec(VideoCodec::H265),
    },
    Pattern {
        text: "h265",
        field: Field::Codec(VideoCodec::H265),
    },
    Pattern {
        text: "x264",
        field: Field::Codec(VideoCodec::H264),
    },
    Pattern {
        text: "h264",
        field: Field::Codec(VideoCodec::H264),
    },
    Pattern {
        text: "xvid",
        field: Field::Codec(VideoCodec::Xvid),
    },
    Pattern {
        text: "divx",
        field: Field::Codec(VideoCodec::Divx),
    },
    Pattern {
        text: "flac",
        field: Field::Audio(AudioCodec::Flac),
    },
    Pattern {
        text: "opus",
        field: Field::Audio(AudioCodec::Opus),
    },
    Pattern {
        text: "lpcm",
        field: Field::Audio(AudioCodec::Pcm),
    },
    Pattern {
        text: "eac3",
        field: Field::Audio(AudioCodec::Eac3),
    },
    Pattern {
        text: "dovi",
        field: Field::Hdr(HdrFormat::DolbyVision),
    },
    Pattern {
        text: "720p",
        field: Field::Resolution(Resolution::Hd),
    },
    Pattern {
        text: "576p",
        field: Field::Resolution(Resolution::Sd576),
    },
    Pattern {
        text: "480p",
        field: Field::Resolution(Resolution::Sd480),
    },
    Pattern {
        text: "hdtv",
        field: Field::Source(ReleaseSource::Hdtv),
    },
    Pattern {
        text: "sdtv",
        field: Field::Source(ReleaseSource::Sdtv),
    },
    Pattern {
        text: "pdtv",
        field: Field::Source(ReleaseSource::Pdtv),
    },
    Pattern {
        text: "ddp",
        field: Field::Audio(AudioCodec::Eac3),
    },
    Pattern {
        text: "dd+",
        field: Field::Audio(AudioCodec::Eac3),
    },
    Pattern {
        text: "ac3",
        field: Field::Audio(AudioCodec::Ac3),
    },
    Pattern {
        text: "aac",
        field: Field::Audio(AudioCodec::Aac),
    },
    Pattern {
        text: "pcm",
        field: Field::Audio(AudioCodec::Pcm),
    },
    Pattern {
        text: "mp3",
        field: Field::Audio(AudioCodec::Mp3),
    },
    Pattern {
        text: "dts",
        field: Field::Audio(AudioCodec::Dts),
    },
    Pattern {
        text: "hlg",
        field: Field::Hdr(HdrFormat::Hlg),
    },
    Pattern {
        text: "sdr",
        field: Field::Hdr(HdrFormat::Sdr),
    },
    Pattern {
        text: "hdr",
        field: Field::Hdr(HdrFormat::Hdr10),
    },
    Pattern {
        text: "cam",
        field: Field::Source(ReleaseSource::Cam),
    },
    Pattern {
        text: "web",
        field: Field::Source(ReleaseSource::WebDl),
    },
    Pattern {
        text: "av1",
        field: Field::Codec(VideoCodec::Av1),
    },
    Pattern {
        text: "vp9",
        field: Field::Codec(VideoCodec::Vp9),
    },
    Pattern {
        text: "avc",
        field: Field::Codec(VideoCodec::H264),
    },
    Pattern {
        text: "dd",
        field: Field::Audio(AudioCodec::Ac3),
    },
    Pattern {
        text: "dv",
        field: Field::Hdr(HdrFormat::DolbyVision),
    },
    Pattern {
        text: "4k",
        field: Field::Resolution(Resolution::UltraHd),
    },
    Pattern {
        text: "ts",
        field: Field::Source(ReleaseSource::Ts),
    },
    Pattern {
        text: "tc",
        field: Field::Source(ReleaseSource::Tc),
    },
    Pattern {
        text: "r5",
        field: Field::Source(ReleaseSource::R5),
    },
];

/// Markers that are recognized in their scene uppercase form only, because the
/// lowercase words collide with title words ("Real Steel").
const REPACK_MARKERS: &[(&str, RepackKind)] = &[
    ("PROPER", RepackKind::Proper),
    ("REPACK", RepackKind::Repack),
    ("REAL", RepackKind::Real),
    ("RERIP", RepackKind::Rerip),
];

/// Applies a field, keeping the best-ranked source and the most descriptive HDR.
fn apply(fields: &mut ReleaseFields, field: Field) {
    match field {
        Field::Resolution(resolution) => {
            fields.resolution.get_or_insert(resolution);
        }
        Field::Source(source) => {
            if fields
                .source
                .is_none_or(|current| source.rank() < current.rank())
            {
                fields.source = Some(source);
            }
        }
        Field::Codec(codec) => {
            fields.video_codec.get_or_insert(codec);
        }
        Field::Hdr(hdr) => {
            if fields
                .hdr
                .is_none_or(|current| hdr.precedence() < current.precedence())
            {
                fields.hdr = Some(hdr);
            }
        }
        Field::Audio(codec) => {
            if !fields.audio.contains(&codec) {
                fields.audio.push(codec);
            }
        }
    }
}

/// Replaces a byte range with a marker that cannot be part of a group name.
fn mask_range(bytes: &mut [u8], start: usize, end: usize) {
    if let Some(region) = bytes.get_mut(start..end) {
        for byte in region {
            *byte = b'#';
        }
    }
}

/// Returns the bytes of `pattern` at `index` when both edges are token boundaries.
///
/// A boundary is an edge of the string or a non-letter, so `ddp` matches inside
/// `DDP5.1` (the `5` is a channel suffix) while `dd` does not match inside `dvdrip`.
fn token_window<'a>(bytes: &'a [u8], index: usize, pattern: &[u8]) -> Option<&'a [u8]> {
    let end = index.checked_add(pattern.len())?;
    let window = bytes.get(index..end)?;
    let before_ok = index
        .checked_sub(1)
        .and_then(|before| bytes.get(before))
        .is_none_or(|byte| !byte.is_ascii_alphabetic());
    let after_ok = bytes
        .get(end)
        .is_none_or(|byte| !byte.is_ascii_alphabetic());
    (before_ok && after_ok).then_some(window)
}

/// True when `pattern` sits at `index`, ignoring case.
fn matches_at(bytes: &[u8], index: usize, pattern: &str) -> bool {
    token_window(bytes, index, pattern.as_bytes())
        .is_some_and(|window| window.eq_ignore_ascii_case(pattern.as_bytes()))
}

/// True when `pattern` sits at `index` with its exact spelling.
fn matches_at_exact(bytes: &[u8], index: usize, pattern: &[u8]) -> bool {
    token_window(bytes, index, pattern) == Some(pattern)
}

/// Scans the raw text for resolution, source, codec, HDR, and audio tags.
fn scan_fields(bytes: &[u8], masked: &mut [u8], fields: &mut ReleaseFields) {
    let mut index = 0;
    while index < bytes.len() {
        let Some(pattern) = PATTERNS
            .iter()
            .find(|pattern| matches_at(bytes, index, pattern.text))
        else {
            index += 1;
            continue;
        };
        apply(fields, pattern.field);
        mask_range(masked, index, index + pattern.text.len());
        index += pattern.text.len();
    }
}

/// Scans for the first superseding marker and masks every one it finds.
fn scan_repack(bytes: &[u8], masked: &mut [u8]) -> Option<RepackKind> {
    let mut found = None;
    let mut index = 0;
    while index < bytes.len() {
        if let Some((text, kind)) = REPACK_MARKERS
            .iter()
            .find(|(text, _)| matches_at_exact(bytes, index, text.as_bytes()))
        {
            found.get_or_insert(*kind);
            mask_range(masked, index, index + text.len());
            index += text.len();
            continue;
        }
        if let Some((end, version)) = match_version(bytes, index) {
            found.get_or_insert(RepackKind::Version(version));
            mask_range(masked, index, end);
            index = end;
            continue;
        }
        index += 1;
    }
    found
}

/// Matches a scene `v2`-style version suffix.
fn match_version(bytes: &[u8], index: usize) -> Option<(usize, u32)> {
    if !matches!(bytes.get(index), Some(b'v' | b'V')) {
        return None;
    }
    if index > 0 && bytes[index - 1].is_ascii_alphabetic() {
        return None;
    }
    let (version, end) = read_digits(bytes, index + 1, 3)?;
    if bytes.get(end).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    Some((end, version))
}

/// Extracts the trailing `-GROUP`, when it is attached to a recognized tag.
///
/// The scene convention is `...-GROUP`, and only that position is read: a
/// hyphen inside a title ("Spider-Man") must not become a group. A group may
/// follow a masked tag or a `5.1`/`7.1` channel layout, which is what a release
/// carries right before its group.
fn extract_group(raw: &str, masked: &[u8]) -> Option<String> {
    let hyphen = masked.iter().rposition(|byte| *byte == b'-')?;
    if !group_attaches(masked, hyphen) {
        return None;
    }
    let mut start = hyphen + 1;
    while matches!(masked.get(start), Some(b'[' | b'(')) {
        start += 1;
    }
    let mut end = start;
    while masked
        .get(end)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'.')
    {
        end += 1;
    }
    while end > start && masked[end - 1] == b'.' {
        end -= 1;
    }
    if end.saturating_sub(start) < 2 {
        return None;
    }
    let candidate = raw.get(start..end)?;
    let plain = candidate
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.');
    let named = candidate.bytes().any(|byte| byte.is_ascii_alphabetic());
    (plain && named).then(|| candidate.to_owned())
}

/// True when the bytes before a hyphen mark it as a group separator.
fn group_attaches(masked: &[u8], hyphen: usize) -> bool {
    match hyphen.checked_sub(1).and_then(|before| masked.get(before)) {
        Some(b'#') => true,
        Some(byte) if byte.is_ascii_digit() => {
            masked.get(hyphen.wrapping_sub(2)) == Some(&b'.')
                && hyphen
                    .checked_sub(3)
                    .and_then(|third| masked.get(third))
                    .is_some_and(u8::is_ascii_digit)
        }
        _ => false,
    }
}

/// Reads at most `max` ASCII digits, refusing a longer run.
fn read_digits(bytes: &[u8], start: usize, max: usize) -> Option<(u32, usize)> {
    let mut end = start;
    while end < bytes.len() && bytes[end].is_ascii_digit() && end - start < max {
        end += 1;
    }
    if end == start || bytes.get(end).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let value = std::str::from_utf8(bytes.get(start..end)?)
        .ok()?
        .parse()
        .ok()?;
    Some((value, end))
}

/// Scans for season, episode, and `COMPLETE` markers.
fn scan_seasons_and_episodes(bytes: &[u8]) -> SeasonScan {
    let mut scan = SeasonScan::default();
    let mut index = 0;
    while index < bytes.len() {
        if let Some((end, season, span)) = match_episode(bytes, index) {
            scan.season.get_or_insert(season);
            scan.episodes = Some(match scan.episodes {
                Some(existing) => existing.union(span),
                None => span,
            });
            scan.spans.push((index, end));
            index = end;
            continue;
        }
        if let Some((end, season)) = match_season(bytes, index) {
            scan.season.get_or_insert(season);
            scan.spans.push((index, end));
            index = end;
            continue;
        }
        if let Some((end, season)) = match_season_word(bytes, index) {
            scan.season.get_or_insert(season);
            scan.spans.push((index, end));
            index = end;
            continue;
        }
        if let Some(end) = match_complete(bytes, index) {
            scan.complete = true;
            scan.spans.push((index, end));
            index = end;
            continue;
        }
        index += 1;
    }
    scan
}

/// Matches `S01E02`, `S01E02E03`, and `S01E02-E03`.
fn match_episode(bytes: &[u8], index: usize) -> Option<(usize, u32, EpisodeSpan)> {
    if !matches!(bytes.get(index), Some(b's' | b'S')) {
        return None;
    }
    if index > 0 && bytes[index - 1].is_ascii_alphabetic() {
        return None;
    }
    let (season, after_season) = read_digits(bytes, index + 1, 4)?;
    if !matches!(bytes.get(after_season), Some(b'e' | b'E')) {
        return None;
    }
    let (first, mut end) = read_digits(bytes, after_season + 1, 4)?;
    let mut span = EpisodeSpan::new(first, first);
    while let Some((episode, next)) = match_episode_extension(bytes, end) {
        span = span.union(EpisodeSpan::new(episode, episode));
        end = next;
    }
    Some((end, season, span))
}

/// Matches the `E03`, `-E03`, or `-03` tail of a multi-episode release.
fn match_episode_extension(bytes: &[u8], index: usize) -> Option<(u32, usize)> {
    let mut cursor = index;
    if bytes.get(cursor) == Some(&b'-') {
        cursor += 1;
    } else if !matches!(bytes.get(cursor), Some(b'e' | b'E')) {
        return None;
    }
    if matches!(bytes.get(cursor), Some(b'e' | b'E')) {
        cursor += 1;
    }
    let (episode, end) = read_digits(bytes, cursor, 4)?;
    // A letter after the digits means the run was not an episode (`-1080p`),
    // except for the `E` of a following `E07`.
    if bytes
        .get(end)
        .is_some_and(|byte| byte.is_ascii_alphabetic() && !matches!(byte, b'e' | b'E'))
    {
        return None;
    }
    Some((episode, end))
}

/// Matches a standalone `S01` season marker.
fn match_season(bytes: &[u8], index: usize) -> Option<(usize, u32)> {
    if !matches!(bytes.get(index), Some(b's' | b'S')) {
        return None;
    }
    if index > 0 && bytes[index - 1].is_ascii_alphabetic() {
        return None;
    }
    let (season, end) = read_digits(bytes, index + 1, 4)?;
    if bytes.get(end).is_some_and(u8::is_ascii_alphanumeric) {
        return None;
    }
    Some((end, season))
}

/// Matches `Season 1`, `Season.1`, and `Season1`.
fn match_season_word(bytes: &[u8], index: usize) -> Option<(usize, u32)> {
    const WORD: &[u8] = b"season";
    if index > 0 && bytes[index - 1].is_ascii_alphabetic() {
        return None;
    }
    let end = index + WORD.len();
    if !bytes.get(index..end)?.eq_ignore_ascii_case(WORD) {
        return None;
    }
    if bytes.get(end).is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    let mut cursor = end;
    while matches!(bytes.get(cursor), Some(b'.' | b' ' | b'_' | b'-')) {
        cursor += 1;
    }
    let (season, end) = read_digits(bytes, cursor, 3)?;
    if bytes.get(end).is_some_and(u8::is_ascii_alphanumeric) {
        return None;
    }
    Some((end, season))
}

/// Matches the `COMPLETE` season-pack marker.
fn match_complete(bytes: &[u8], index: usize) -> Option<usize> {
    const WORD: &str = "complete";
    if matches_at(bytes, index, WORD) {
        return Some(index + WORD.len());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn parsed(name: &str) -> ReleaseName {
        ReleaseName::parse(name)
    }

    #[test]
    fn pattern_table_is_sorted_longest_first() {
        for pair in PATTERNS.windows(2) {
            assert!(
                pair[0].text.len() >= pair[1].text.len(),
                "`{}` must not precede `{}`",
                pair[1].text,
                pair[0].text
            );
        }
    }

    #[test]
    fn hdr_strings_match_the_media_file_check_constraint() {
        let documented = ["sdr", "hdr10", "hdr10_plus", "hlg", "dolby_vision"];
        let actual: Vec<&str> = HdrFormat::ALL.into_iter().map(HdrFormat::as_str).collect();
        assert_eq!(documented.as_slice(), actual.as_slice());
    }

    #[test]
    fn resolution_labels_are_unique() {
        let labels: Vec<&str> = Resolution::ALL
            .into_iter()
            .map(Resolution::as_str)
            .collect();
        for (index, label) in labels.iter().enumerate() {
            assert!(!labels[index + 1..].contains(label), "duplicate `{label}`");
        }
    }

    #[test]
    fn raw_text_is_preserved() {
        let name = "The.Matrix.1999.1080p.BluRay.x264-GROUP";
        assert_eq!(parsed(name).raw(), name);
    }

    #[test]
    fn parses_a_full_release_name() {
        let release =
            parsed("The.Matrix.1999.2160p.UHD.BluRay.REMUX.HDR.HEVC.TrueHD.7.1.Atmos-GROUP");
        assert_eq!(release.resolution(), Some(Resolution::UltraHd));
        assert_eq!(release.source(), Some(ReleaseSource::Remux));
        assert_eq!(release.video_codec(), Some(VideoCodec::H265));
        assert_eq!(release.hdr(), Some(HdrFormat::Hdr10));
        assert_eq!(release.audio(), [AudioCodec::TrueHd, AudioCodec::Atmos]);
        assert_eq!(release.group(), Some("GROUP"));
        assert_eq!(release.repack(), None);
        assert_eq!(release.season(), None);
        assert_eq!(release.episodes(), None);
        assert!(!release.is_season_pack());
    }

    #[test]
    fn parses_every_resolution_token() {
        let tokens = [
            ("2160p", Resolution::UltraHd),
            ("4K", Resolution::UltraHd),
            ("1440p", Resolution::QuadHd),
            ("1080p", Resolution::FullHd),
            ("1080i", Resolution::FullHdInterlaced),
            ("720p", Resolution::Hd),
            ("576p", Resolution::Sd576),
            ("480p", Resolution::Sd480),
        ];
        for (token, expected) in tokens {
            assert_eq!(
                parsed(&format!("Movie.2019.{token}.x264-GROUP")).resolution(),
                Some(expected),
                "token {token}"
            );
        }
    }

    #[test]
    fn parses_every_source_token() {
        let tokens = [
            ("REMUX", ReleaseSource::Remux),
            ("BDREMUX", ReleaseSource::Remux),
            ("BluRay", ReleaseSource::BluRay),
            ("BLU-RAY", ReleaseSource::BluRay),
            ("BDRip", ReleaseSource::BdRip),
            ("BRRip", ReleaseSource::BrRip),
            ("WEB-DL", ReleaseSource::WebDl),
            ("WEBDL", ReleaseSource::WebDl),
            ("WEB", ReleaseSource::WebDl),
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
        for (token, expected) in tokens {
            assert_eq!(
                parsed(&format!("Movie.2019.1080p.{token}.x264-GROUP")).source(),
                Some(expected),
                "token {token}"
            );
        }
    }

    #[test]
    fn parses_every_codec_token() {
        let tokens = [
            ("x264", VideoCodec::H264),
            ("H.264", VideoCodec::H264),
            ("AVC", VideoCodec::H264),
            ("x265", VideoCodec::H265),
            ("H.265", VideoCodec::H265),
            ("HEVC", VideoCodec::H265),
            ("AV1", VideoCodec::Av1),
            ("XviD", VideoCodec::Xvid),
            ("DivX", VideoCodec::Divx),
            ("VP9", VideoCodec::Vp9),
        ];
        for (token, expected) in tokens {
            assert_eq!(
                parsed(&format!("Movie.2019.1080p.BluRay.{token}-GROUP")).video_codec(),
                Some(expected),
                "token {token}"
            );
        }
    }

    #[test]
    fn parses_every_hdr_token() {
        let tokens = [
            ("SDR", HdrFormat::Sdr),
            ("HDR", HdrFormat::Hdr10),
            ("HDR10", HdrFormat::Hdr10),
            ("HDR10+", HdrFormat::Hdr10Plus),
            ("HLG", HdrFormat::Hlg),
            ("DV", HdrFormat::DolbyVision),
            ("DoVi", HdrFormat::DolbyVision),
            ("Dolby.Vision", HdrFormat::DolbyVision),
        ];
        for (token, expected) in tokens {
            assert_eq!(
                parsed(&format!("Movie.2019.2160p.BluRay.{token}.x265-GROUP")).hdr(),
                Some(expected),
                "token {token}"
            );
        }
    }

    #[test]
    fn parses_every_audio_token() {
        let tokens = [
            ("AAC", AudioCodec::Aac),
            ("AC3", AudioCodec::Ac3),
            ("DD", AudioCodec::Ac3),
            ("EAC3", AudioCodec::Eac3),
            ("DDP", AudioCodec::Eac3),
            ("DD+", AudioCodec::Eac3),
            ("DTS", AudioCodec::Dts),
            ("DTS-HD", AudioCodec::DtsHd),
            ("DTSHD", AudioCodec::DtsHd),
            ("DTS:X", AudioCodec::DtsX),
            ("TrueHD", AudioCodec::TrueHd),
            ("Atmos", AudioCodec::Atmos),
            ("FLAC", AudioCodec::Flac),
            ("MP3", AudioCodec::Mp3),
            ("Opus", AudioCodec::Opus),
            ("PCM", AudioCodec::Pcm),
            ("LPCM", AudioCodec::Pcm),
        ];
        for (token, expected) in tokens {
            assert_eq!(
                parsed(&format!("Movie.2019.1080p.BluRay.x264.{token}-GROUP")).audio(),
                [expected],
                "token {token}"
            );
        }
    }

    #[test]
    fn channel_suffixes_do_not_hide_audio_tokens() {
        assert_eq!(
            parsed("Movie.2019.1080p.WEB-DL.DDP5.1.H.264-GROUP").audio(),
            [AudioCodec::Eac3]
        );
        assert_eq!(
            parsed("Movie.2019.1080p.WEB-DL.DD5.1.H.264-GROUP").audio(),
            [AudioCodec::Ac3]
        );
        assert_eq!(
            parsed("Movie.2019.1080p.WEB-DL.AAC2.0.H.264-GROUP").audio(),
            [AudioCodec::Aac]
        );
    }

    #[test]
    fn a_higher_ranked_source_wins_over_a_lower_one() {
        let release = parsed("Movie.2019.2160p.BluRay.REMUX.HEVC.DTS-HD-GROUP");
        assert_eq!(release.source(), Some(ReleaseSource::Remux));
    }

    #[test]
    fn the_most_descriptive_hdr_token_wins() {
        let release = parsed("Movie.2019.2160p.WEB-DL.HDR.DV.H.265-GROUP");
        assert_eq!(release.hdr(), Some(HdrFormat::DolbyVision));
    }

    #[test]
    fn parses_every_repack_marker() {
        let tokens = [
            ("PROPER", RepackKind::Proper),
            ("REPACK", RepackKind::Repack),
            ("REAL", RepackKind::Real),
            ("RERIP", RepackKind::Rerip),
            ("v2", RepackKind::Version(2)),
        ];
        for (token, expected) in tokens {
            let release = parsed(&format!("Movie.2019.1080p.BluRay.x264.{token}-GROUP"));
            assert_eq!(release.repack(), Some(expected), "token {token}");
        }
    }

    #[test]
    fn a_lowercase_title_word_is_not_a_repack() {
        assert_eq!(
            parsed("Real.Steel.2011.1080p.BluRay.x264-GROUP").repack(),
            None
        );
    }

    #[test]
    fn a_repack_marker_after_the_group_is_still_reported() {
        let release = parsed("Movie.2019.1080p.BluRay.x264-GROUP.REPACK");
        assert_eq!(release.repack(), Some(RepackKind::Repack));
        assert_eq!(release.group(), Some("GROUP"));
    }

    #[test]
    fn parses_a_single_episode() {
        let release = parsed("Show.Name.S01E02.1080p.WEB-DL.DDP5.1.H.264-NTb");
        assert_eq!(release.season(), Some(1));
        assert_eq!(release.episodes().map(EpisodeSpan::first), Some(2));
        assert_eq!(release.episodes().map(EpisodeSpan::last), Some(2));
        assert!(release.episodes().is_some_and(|span| !span.is_multi()));
        assert!(!release.is_season_pack());
        assert_eq!(release.group(), Some("NTb"));
    }

    #[test]
    fn parses_multi_episode_forms() {
        let forms = [
            ("Show.S01E01E02.1080p.WEB-DL.x264-GROUP", 1, 2),
            ("Show.S01E01-E03.1080p.WEB-DL.x264-GROUP", 1, 3),
            ("Show.S02E05E06E07.1080p.WEB-DL.x264-GROUP", 5, 7),
            ("Show.S10E20-E22.1080p.WEB-DL.x264-GROUP", 20, 22),
        ];
        for (name, first, last) in forms {
            let span = parsed(name).episodes().expect("episodes");
            assert_eq!((span.first(), span.last()), (first, last), "{name}");
            assert!(span.is_multi(), "{name}");
            assert_eq!(span.episode_count(), last - first + 1, "{name}");
        }
    }

    #[test]
    fn an_out_of_order_range_is_ordered() {
        let span = parsed("Show.S01E05E03.1080p.WEB-DL.x264-GROUP")
            .episodes()
            .expect("episodes");
        assert_eq!((span.first(), span.last()), (3, 5));
    }

    #[test]
    fn parses_season_pack_forms() {
        let forms = [
            "Show.Name.S01.1080p.BluRay.x264-GROUP",
            "Show.Name.Season.1.1080p.WEB-DL.AAC2.0.H.264-NTb",
            "Show.Name.Season 2.720p.HDTV.x264-GROUP",
            "Show.Name.COMPLETE.1080p.BluRay.x264-GROUP",
        ];
        for name in forms {
            let release = parsed(name);
            assert!(release.episodes().is_none(), "{name}");
            assert!(release.is_season_pack(), "{name}");
        }
        assert_eq!(parsed(forms[0]).season(), Some(1));
        assert_eq!(parsed(forms[1]).season(), Some(1));
        assert_eq!(parsed(forms[2]).season(), Some(2));
        assert_eq!(parsed(forms[3]).season(), None);
    }

    #[test]
    fn episodes_take_precedence_over_a_pack_marker() {
        let release = parsed("Show.Name.S01E01E02.COMPLETE.1080p.WEB-DL.x264-GROUP");
        assert!(!release.is_season_pack());
        assert_eq!(release.episodes().map(EpisodeSpan::first), Some(1));
    }

    #[test]
    fn a_hyphenated_title_does_not_become_a_group() {
        assert_eq!(parsed("Spider-Man.2002.1080p.BluRay.x264").group(), None);
        assert_eq!(parsed("The-Office.S01E01.1080p.BluRay.x264").group(), None);
    }

    #[test]
    fn a_group_after_a_repack_marker_is_reported() {
        assert_eq!(
            parsed("Movie.2019.1080p.BluRay.x264.REPACK-GROUP").group(),
            Some("GROUP")
        );
    }

    #[test]
    fn bracketed_groups_are_reported_without_their_brackets() {
        assert_eq!(
            parsed("Movie.2019.1080p.BluRay.x264-[RARBG]").group(),
            Some("RARBG")
        );
        assert_eq!(
            parsed("Movie.2019.720p.WEBRip.x264-[YTS.MX]").group(),
            Some("YTS.MX")
        );
    }

    #[test]
    fn parse_never_panics_on_hostile_inputs() {
        let long = "a".repeat(200_000);
        let repeated = "S01E01-".repeat(10_000);
        let inputs = [
            "",
            ".",
            "-",
            "....",
            "----",
            "[]()",
            "映画-🎬-S01E01-🎬",
            "\u{202e}",
            long.as_str(),
            repeated.as_str(),
        ];
        for input in inputs {
            let parsed = ReleaseName::parse(input);
            assert_eq!(parsed.raw(), input);
        }
    }

    #[test]
    fn a_release_without_tags_parses_to_blank_fields() {
        let release = parsed("Some Random File Name");
        assert_eq!(release.resolution(), None);
        assert_eq!(release.source(), None);
        assert_eq!(release.video_codec(), None);
        assert_eq!(release.hdr(), None);
        assert!(
            release.audio().is_empty(),
            "a name with no audio token parses to no audio codecs: {:?}",
            release.audio()
        );
        assert_eq!(release.group(), None);
        assert_eq!(release.repack(), None);
        assert_eq!(release.season(), None);
        assert_eq!(release.episodes(), None);
        assert!(!release.is_season_pack());
    }

    #[test]
    fn underscore_and_space_separators_are_understood() {
        let release = parsed("Movie_2019_1080p_BluRay_x264-GROUP");
        assert_eq!(release.resolution(), Some(Resolution::FullHd));
        assert_eq!(release.source(), Some(ReleaseSource::BluRay));
        assert_eq!(release.video_codec(), Some(VideoCodec::H264));
        assert_eq!(release.group(), Some("GROUP"));
    }

    #[test]
    fn a_non_ascii_title_still_parses_its_tags() {
        let release = parsed("Матрица.1999.1080p.BluRay.x264-GROUP");
        assert_eq!(release.resolution(), Some(Resolution::FullHd));
        assert_eq!(release.video_codec(), Some(VideoCodec::H264));
        assert_eq!(release.group(), Some("GROUP"));
    }

    #[test]
    fn a_huge_episode_range_does_not_allocate_a_list() {
        let span = parsed("Show.S01E01-E9999.1080p.WEB-DL.x264-GROUP")
            .episodes()
            .expect("episodes");
        assert_eq!((span.first(), span.last()), (1, 9999));
        assert_eq!(span.episode_count(), 9999);
    }

    #[test]
    fn an_elevated_byte_count_is_not_an_episode() {
        let release = parsed("Show.S01E01-1080p.WEB-DL.x264-GROUP");
        assert_eq!(release.episodes().map(EpisodeSpan::last), Some(1));
    }

    proptest! {
        #[test]
        fn parse_never_panics_on_arbitrary_input(input in any::<String>()) {
            let _ = ReleaseName::parse(&input);
        }

        #[test]
        fn parse_never_panics_on_separator_soup(input in "[._\\-\\[\\]() ]{0,512}") {
            let _ = ReleaseName::parse(&input);
        }
    }
}
