//! Library file and folder names.
//!
//! [`ReleaseName`] reads a scene release. This reads a name that is already on disk:
//! `Movie Title (2005) [1080p]`, `Show.Name.S01E02.1080p.WEB-DL`,
//! `Show Name - 1x02 - Episode Title`, `Show Name - 012`,
//! `Show/Season 01/Show - S01E02.mkv`. The quality, source, codec, group, and version tail is
//! [`ReleaseName`]'s, and it is read on the whole text so a tag that lives in a folder name
//! (`Movie Title (2005) [1080p]/Movie Title.mkv`) still counts. Everything specific to a
//! library name is read here: the crossed `1x02` form, the anime absolute number, the season
//! folder, the year, and the file extension. Numbering is scanned here rather than delegated
//! for the same reason: a library name adds the crossed and running forms, takes a season from
//! a folder, and has to report a disagreement instead of silently widening a span.
//!
//! Three conventions decide what the parser will and will not say.
//!
//! - A year is read when it is bracketed (`(1999)`) or when it sits before the release tags
//!   (`The.Matrix.1999.1080p`). A bare four-digit number at the end is reported as ambiguous,
//!   because `The Matrix 1999` and `Blade Runner 2049` are the same shape and only a provider
//!   match can tell a year from a title numeral.
//! - A number with a dash, an underscore, or a dot between it and the title is a running
//!   episode number (`Show Name - 012`). A number separated by a space alone is reported as
//!   ambiguous for the same reason: `Show Name 012` and `Mobile Suit Gundam 0083` are the same
//!   shape. A number with title words after it is part of the title, which is what keeps
//!   `12 Monkeys` and `Oceans.11.2001` whole. The one reading left to the convention rather than
//!   to the evidence is a dot-separated trailing number with no year and no tag after it, where
//!   `Oceans.11` reads as episode 11; a provider match is what settles it (PG-E14-05).
//! - A year never decides whether a name is a film or an episode, and neither does the absence
//!   of numbering. Only numbering decides, so a name without it reports no shape rather than
//!   calling itself a film: a series folder (`Show (2019)`) looks exactly the same.
//!
//! A name the evidence cannot resolve is a [`NameError`] that still carries every field the
//! parse read, because `unmatched_file` exists to hold exactly that for a human to match.
//! Confidence is deliberately not here: the matching pipeline (PG-E14-05) scores the fields
//! this returns. There is no edition field either, because no table has a column for one, so a
//! version marker stays visible through [`ReleaseName::repack`] on [`LibraryName::release`].
//!
//! The module is pure text. It takes a `&str` and splits it on `/` and `\` itself, because
//! `std::path` is platform-dependent — on Unix a `\` is an ordinary character — and the same
//! library text has to parse the same way wherever it was scanned. Containment is
//! `PathGuard`'s rule (AGENTS R21), not this module's; text that names a path rather than a
//! name (`.` or `..`) is refused rather than read.

use serde::Serialize;
use thiserror::Error;

use crate::release::{EpisodeSpan, ReleaseName};

/// The video container extensions a library file may carry.
///
/// The list exists only to tell a file name from a folder name, because a folder's name may
/// contain dots (`The.Matrix.1999.1080p.BluRay`). It is not a safety allowlist: that is
/// `TorrentInspector`'s (PG-E11-01).
const MEDIA_EXTENSIONS: &[&str] = &[
    "3gp", "avi", "flv", "m2ts", "m4v", "mkv", "mov", "mp4", "mpeg", "mpg", "mts", "ogm", "ogv",
    "rmvb", "ts", "vob", "webm", "wmv",
];

/// How many separator-delimited parts one release tag may span.
///
/// `H.265` spans two, and four is enough for every spelling [`ReleaseName`] knows. The cap is
/// what keeps a window probe from reaching back into the title.
const MAX_TAG_PARTS: usize = 4;

/// What `ReleaseName` writes over a span it consumed, reused so both scanners count a byte
/// position the same way.
const MASK: u8 = b'#';

/// How a name numbers its episodes.
///
/// The variant is the evidence itself: a name that states numbering is episodic, and the form
/// it used is recorded rather than flattened, because `1x02` and `012` disagree about whether a
/// season exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Numbering {
    /// `S01E02` or `Season 1`: a season and the episodes inside it.
    SeasonEpisode,
    /// `1x02`: the crossed form some libraries write.
    CrossSeason,
    /// `EP12` or `E12`: a running number the name itself introduces.
    PrefixedAbsolute,
    /// A running number the name separates from its title, such as `Show - 012`.
    BareAbsolute,
}

/// One library file or folder name, parsed as far as its evidence goes.
// Serialization only: a derived `Deserialize` could build a name whose fields disagree with
// each other, so `parse` stays the one way in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LibraryName {
    raw: String,
    title: Option<String>,
    year: Option<u16>,
    season: Option<u32>,
    episodes: Option<EpisodeSpan>,
    absolute: Option<u32>,
    numbering: Option<Numbering>,
    extension: Option<String>,
    release: ReleaseName,
}

impl LibraryName {
    /// Parses a library file or folder name.
    ///
    /// Every accessor answers `None` for a value the name does not state. On `Ok`, [`title`]
    /// is always `Some`; on `Err`, [`NameError::name`] carries every field the parse did read
    /// so the caller can park it in `unmatched_file` without losing what was found.
    ///
    /// [`title`]: Self::title
    // The error carries the whole parse so that `unmatched_file` loses nothing, which makes it
    // larger than clippy's threshold. The path is taken once per file, not in a loop, and boxing
    // it would only make the caller's own error type harder to handle.
    #[allow(clippy::result_large_err)]
    pub fn parse(text: &str) -> Result<Self, NameError> {
        let release = ReleaseName::parse(text);
        if text.trim().is_empty() {
            return Err(NameError {
                name: Self::blank(text, release),
                reason: UnresolvedReason::Empty,
            });
        }
        let parts = match components(text) {
            Ok(parts) => parts,
            Err(reason) => {
                return Err(NameError {
                    name: Self::blank(text, release),
                    reason,
                });
            }
        };
        let Some((subject, folders)) = parts.split_last() else {
            return Err(NameError {
                name: Self::blank(text, release),
                reason: UnresolvedReason::Empty,
            });
        };

        let subject_read = read_component(subject);
        let mut evidence = Scan::default();
        let mut folder_title = None;
        for folder in folders.iter().rev() {
            let read = read_component(folder);
            folder_title = folder_title.or(read.title);
            evidence.absorb(read.scan, false);
        }
        let extension = subject_read.extension;
        evidence.absorb(subject_read.scan, true);
        let numbers = evidence.finish();

        let name = Self {
            raw: text.to_owned(),
            title: subject_read.title.or(folder_title),
            year: numbers.year,
            season: numbers.season,
            episodes: numbers.episodes,
            absolute: numbers.absolute,
            numbering: numbers.numbering,
            extension,
            release,
        };
        match numbers.problem {
            Some(reason) => Err(NameError { name, reason }),
            None if name.title.is_none() => Err(NameError {
                name,
                reason: UnresolvedReason::NoTitle,
            }),
            None => Ok(name),
        }
    }

    /// A name with only its raw text and release tail known, for an early refusal.
    fn blank(text: &str, release: ReleaseName) -> Self {
        Self {
            raw: text.to_owned(),
            title: None,
            year: None,
            season: None,
            episodes: None,
            absolute: None,
            numbering: None,
            extension: None,
            release,
        }
    }

    /// The unmodified input.
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// The title, cleaned of tags, separators, and the year.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// The year, when the name states one unambiguously.
    pub fn year(&self) -> Option<u16> {
        self.year
    }

    /// The season number, when the name states one.
    pub fn season(&self) -> Option<u32> {
        self.season
    }

    /// The episodes inside the stated season; `None` also for a season pack.
    pub fn episodes(&self) -> Option<EpisodeSpan> {
        self.episodes
    }

    /// The running number of a name with no season, as anime absolute ordering uses.
    pub fn absolute(&self) -> Option<u32> {
        self.absolute
    }

    /// How the name numbers its episodes, which is the only shape evidence there is.
    pub fn numbering(&self) -> Option<Numbering> {
        self.numbering
    }

    /// The file extension, lowercased, when the name ends in a container this library holds.
    pub fn extension(&self) -> Option<&str> {
        self.extension.as_deref()
    }

    /// The quality, source, codec, HDR, audio, group, and version tail, read from the whole
    /// input so that a tag in a folder name still counts.
    pub fn release(&self) -> &ReleaseName {
        &self.release
    }
}

/// Why a name could not be resolved, carrying everything the parse did find.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{reason}")]
pub struct NameError {
    /// The fields the parse read before it stopped. Their accessors answer `None` for what the
    /// name does not state, and a conflicting value is the first one the name stated.
    pub name: LibraryName,
    /// What stopped the parse from resolving the name.
    pub reason: UnresolvedReason,
}

/// Why a name cannot be matched without help.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UnresolvedReason {
    /// The input held no characters, or only whitespace.
    #[error("the name is empty")]
    Empty,
    /// Nothing in the name reads as a title (`S01E02.mkv`, `1080p`, `.mkv`).
    #[error("no title could be read from the name")]
    NoTitle,
    /// A bare four-digit number is a year in `The Matrix 1999` and part of the title in
    /// `Blade Runner 2049`; only a provider match can tell them apart.
    #[error("`{year}` could be a year or part of the title")]
    AmbiguousYear {
        /// The four digits that were left unassigned.
        year: u16,
    },
    /// A number separated by a space alone is an episode in `Show 012` and part of the title
    /// in `Mobile Suit Gundam 0083`.
    #[error("`{number}` could be an absolute episode or part of the title")]
    AmbiguousAbsolute {
        /// The number that was left unassigned.
        number: u32,
    },
    /// The name states two different seasons, so neither can be believed.
    #[error("the name states both season {first} and season {second}")]
    ConflictingSeasons {
        /// The season read first.
        first: u32,
        /// The season that disagreed with it.
        second: u32,
    },
    /// The name states two different episodes at the same position.
    #[error("the name states both episode {first} and episode {second}")]
    ConflictingEpisodes {
        /// The first episode of the span read first.
        first: u32,
        /// The first episode of the span that disagreed with it.
        second: u32,
    },
    /// The text names a path rather than a file or folder: it contains `.` or `..`.
    #[error("`{component}` is path syntax rather than part of a name")]
    NotAName {
        /// The component that was refused.
        component: String,
    },
}

/// One component of the input, with what it states.
struct Component {
    title: Option<String>,
    extension: Option<String>,
    scan: Scan,
}

/// A run of bytes between separators.
struct Token {
    start: usize,
    end: usize,
}

/// A release-relevant marker found in a component.
struct Marker {
    start: usize,
    end: usize,
    kind: Kind,
}

/// What a marker states.
#[derive(Clone, Copy)]
enum Kind {
    SeasonEpisode { season: u32, span: EpisodeSpan },
    CrossSeason { season: u32, span: EpisodeSpan },
    Season { season: u32, episode: Option<u32> },
    Prefixed { number: u32 },
    Bare(Bare),
    Year { value: u16, definite: bool },
}

/// A running number with no season, and what separates it from the title.
#[derive(Clone, Copy)]
struct Bare {
    number: u32,
    /// The number is the whole component, as `01.mkv` is under `Season 1/`.
    whole: bool,
    /// A dash, an underscore, or a dot separates it from the title.
    separated: bool,
}

/// Everything a component stated about numbering and the year.
#[derive(Default)]
struct Scan {
    seasons: Vec<u32>,
    spans: Vec<EpisodeSpan>,
    episode_kind: Option<Numbering>,
    prefixed: Option<u32>,
    bare: Option<Bare>,
    year: Option<u16>,
    ambiguous_year: Option<u16>,
    boundary: Option<usize>,
}

impl Scan {
    /// Folds another component's evidence in, preferring this one's when it has any.
    fn absorb(&mut self, other: Self, prefer: bool) {
        for season in other.seasons {
            self.push_season(season);
        }
        self.spans.extend(other.spans);
        take(&mut self.episode_kind, other.episode_kind, prefer);
        take(&mut self.prefixed, other.prefixed, prefer);
        take(&mut self.bare, other.bare, prefer);
        take(&mut self.year, other.year, prefer);
        take(&mut self.ambiguous_year, other.ambiguous_year, prefer);
    }

    fn push_season(&mut self, season: u32) {
        if !self.seasons.contains(&season) {
            self.seasons.push(season);
        }
    }

    fn record(&mut self, start: usize, kind: Kind) {
        self.boundary.get_or_insert(start);
        match kind {
            Kind::SeasonEpisode { season, span } => {
                self.push_season(season);
                self.spans.push(span);
                self.episode_kind.get_or_insert(Numbering::SeasonEpisode);
            }
            Kind::CrossSeason { season, span } => {
                self.push_season(season);
                self.spans.push(span);
                self.episode_kind.get_or_insert(Numbering::CrossSeason);
            }
            Kind::Season { season, episode } => {
                self.push_season(season);
                if let Some(number) = episode {
                    self.spans.push(EpisodeSpan::new(number, number));
                }
                self.episode_kind.get_or_insert(Numbering::SeasonEpisode);
            }
            Kind::Prefixed { number } => {
                self.prefixed.get_or_insert(number);
            }
            Kind::Bare(bare) => {
                self.bare.get_or_insert(bare);
            }
            Kind::Year { value, definite } => {
                if definite {
                    self.year.get_or_insert(value);
                } else {
                    self.ambiguous_year.get_or_insert(value);
                }
            }
        }
    }

    /// Decides the numbers, naming the first disagreement or ambiguity it found.
    fn finish(self) -> Numbers {
        let mut problem = None;
        if let [first, second, ..] = self.seasons.as_slice() {
            problem = Some(UnresolvedReason::ConflictingSeasons {
                first: *first,
                second: *second,
            });
        }
        let season = self.seasons.first().copied();

        let spans = distinct_spans(&self.spans);
        let mut episodes = spans.first().copied();
        if let [first, second, ..] = spans.as_slice() {
            problem.get_or_insert(UnresolvedReason::ConflictingEpisodes {
                first: first.first(),
                second: second.first(),
            });
        }

        let mut running = None;
        match (self.prefixed, self.bare) {
            (Some(prefixed), Some(bare)) if prefixed != bare.number => {
                problem.get_or_insert(UnresolvedReason::ConflictingEpisodes {
                    first: prefixed,
                    second: bare.number,
                });
            }
            (Some(prefixed), _) => running = Some((prefixed, Numbering::PrefixedAbsolute)),
            (None, Some(bare)) => running = Some((bare.number, Numbering::BareAbsolute)),
            (None, None) => {}
        }

        let mut absolute = None;
        let mut numbering = self.episode_kind;
        if season.is_some() {
            if episodes.is_none()
                && let Some((number, _)) = running
            {
                episodes = Some(EpisodeSpan::new(number, number));
            }
            numbering = Some(self.episode_kind.unwrap_or(Numbering::SeasonEpisode));
        } else if let Some((number, kind)) = running {
            let ambiguous = kind == Numbering::BareAbsolute
                && self.bare.is_some_and(|bare| !bare.whole && !bare.separated);
            if ambiguous {
                problem.get_or_insert(UnresolvedReason::AmbiguousAbsolute { number });
            } else {
                absolute = Some(number);
                numbering = Some(kind);
            }
        }

        let year = self.year;
        if year.is_none()
            && let Some(value) = self.ambiguous_year
        {
            problem.get_or_insert(UnresolvedReason::AmbiguousYear { year: value });
        }

        Numbers {
            year,
            season,
            episodes,
            absolute,
            numbering,
            problem,
        }
    }
}

/// What a whole input's evidence decided.
struct Numbers {
    year: Option<u16>,
    season: Option<u32>,
    episodes: Option<EpisodeSpan>,
    absolute: Option<u32>,
    numbering: Option<Numbering>,
    problem: Option<UnresolvedReason>,
}

/// Splits the input into components, refusing the ones that are path syntax rather than names.
fn components(text: &str) -> Result<Vec<&str>, UnresolvedReason> {
    let mut parts = Vec::new();
    for part in text.split(['/', '\\']) {
        if part.is_empty() {
            continue;
        }
        if matches!(part, "." | "..") {
            return Err(UnresolvedReason::NotAName {
                component: part.to_owned(),
            });
        }
        parts.push(part);
    }
    Ok(parts)
}

/// Reads one component's title, extension, and numbering evidence.
fn read_component(component: &str) -> Component {
    let (extension, stem) = split_extension(component);
    if stem.starts_with('.') {
        // A leading dot names a hidden file, so it is a name with no stem rather than a stem
        // and an extension (`.mkv` states nothing to match).
        return Component {
            title: None,
            extension: None,
            scan: Scan::default(),
        };
    }
    let tokens = tokenize(stem);
    let ranges = covered_ranges(stem, &tokens);
    let masked = mask(stem, &ranges);
    let mut scan = Scan::default();
    let markers = scan_markers(stem, &masked, &mut scan);
    let cut = [scan.boundary, tail_start(stem, &tokens, &markers)]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(stem.len());
    Component {
        title: clean_title(&stem[..cut]),
        extension: extension.map(str::to_ascii_lowercase),
        scan,
    }
}

/// Splits a trailing media extension off a component, leaving dotted folders whole.
fn split_extension(component: &str) -> (Option<&str>, &str) {
    let Some(dot) = component.rfind('.') else {
        return (None, component);
    };
    if dot == 0 {
        return (None, component);
    }
    let candidate = &component[dot + 1..];
    if MEDIA_EXTENSIONS
        .iter()
        .any(|extension| candidate.eq_ignore_ascii_case(extension))
    {
        (Some(candidate), &component[..dot])
    } else {
        (None, component)
    }
}

/// Splits a stem into runs of bytes between separators.
fn tokenize(stem: &str) -> Vec<Token> {
    let bytes = stem.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if is_separator(bytes, index) {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && !is_separator(bytes, index) {
            index += 1;
        }
        tokens.push(Token { start, end: index });
    }
    tokens
}

/// True when the byte at `index` is not part of a token.
///
/// A dash is a separator only when a space, a bracket, another separator, or an edge touches it,
/// which keeps `x264-GROUP` and `Spider-Man` whole while `Show - 012` splits.
fn is_separator(bytes: &[u8], index: usize) -> bool {
    match bytes.get(index) {
        Some(byte) if is_break(*byte) => true,
        Some(b'-') => {
            let before = index
                .checked_sub(1)
                .and_then(|previous| bytes.get(previous));
            let after = bytes.get(index + 1);
            let soft =
                |byte: Option<&u8>| byte.is_none_or(|value| is_break(*value) || *value == b'-');
            soft(before) || soft(after)
        }
        _ => false,
    }
}

/// The separator bytes that never belong to a token.
fn is_break(byte: u8) -> bool {
    byte.is_ascii() && is_break_char(char::from(byte))
}

/// The separator characters, for the title cleaner that walks characters rather than bytes.
fn is_break_char(character: char) -> bool {
    matches!(character, '.' | '_' | ' ' | '\t' | '[' | ']' | '(' | ')')
}

/// The byte ranges the marker scan must skip because [`ReleaseName`] reads them as a tag.
///
/// A tag token covers itself. Beyond that a tag may span several parts (`H.265`, `Dolby Vision`),
/// so a window ending at a token also covers it when the window parses as a tag and the window
/// *without* its last token does not: the last part is then the one that needed the rest. That
/// is what stops a leading tag from swallowing what follows it — `H.265` covers both parts,
/// while `TC.012` covers `TC` alone, because `TC` already parses without the number.
fn covered_ranges(stem: &str, tokens: &[Token]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    for end in 0..tokens.len() {
        if is_tag(&stem[tokens[end].start..tokens[end].end]) {
            ranges.push((tokens[end].start, tokens[end].end));
            continue;
        }
        let first = end.saturating_sub(MAX_TAG_PARTS - 1);
        let found = (first..end).rev().find(|start| {
            is_tag(&stem[tokens[*start].start..tokens[end].end])
                && !is_tag(&stem[tokens[*start].start..tokens[end - 1].end])
        });
        if let Some(start) = found {
            ranges.push((tokens[start].start, tokens[end].end));
        }
    }
    ranges
}

/// True when the text is a release tag, using [`ReleaseName`]'s own vocabulary.
///
/// A video dimension (`1920x1080`) counts as a tag even though [`ReleaseName`] does not model
/// one: it is a release field, so it ends a title rather than joining it.
fn is_tag(text: &str) -> bool {
    if is_dimension(text) {
        return true;
    }
    let release = ReleaseName::parse(text);
    release.resolution().is_some()
        || release.source().is_some()
        || release.video_codec().is_some()
        || release.hdr().is_some()
        || !release.audio().is_empty()
        || release.repack().is_some()
}

/// True when the text is a `<width>x<height>` pair rather than a crossed season marker.
fn is_dimension(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut width = 0;
    while bytes.get(width).is_some_and(u8::is_ascii_digit) {
        width += 1;
    }
    // Three digits or more is a width; `1x02` keeps its first run short and is read as numbering.
    if width < 3 || !matches!(bytes.get(width), Some(b'x' | b'X')) {
        return false;
    }
    let mut height = width + 1;
    while bytes.get(height).is_some_and(u8::is_ascii_digit) {
        height += 1;
    }
    height > width + 1 && height == bytes.len()
}

/// Blanks the tag ranges so the marker scan cannot read numbering out of a tag.
fn mask(stem: &str, ranges: &[(usize, usize)]) -> Vec<u8> {
    let mut masked = stem.as_bytes().to_vec();
    for (start, end) in ranges {
        if let Some(region) = masked.get_mut(*start..*end) {
            region.fill(MASK);
        }
    }
    masked
}

/// True when a token is a fragment of a release tail rather than a word of the title.
///
/// A fragment carries no lowercase ASCII letter (`265`, `MA`, `IMAX`), which is what lets a
/// path-crossing tail such as `DTS-HD.MA.5.1` stay together. It is never enough on its own: the
/// caller also refuses a fragment that states a marker, so `S01E02` and a bare `012` end a tail
/// instead of joining one.
fn is_fragment(text: &str) -> bool {
    !text.chars().any(|character| character.is_ascii_lowercase())
        && text
            .chars()
            .any(|character| character.is_ascii_alphanumeric())
}

/// The byte offset where the trailing run of tag tokens begins, or `None` when there is none.
///
/// The run walks right to left over tag and fragment tokens and stops at anything else, and it
/// stops at a marker because a marker belongs to the numbering rather than to the tail. The
/// answer is the leftmost *tag* of that run, so a title word is never cut away for the sake of a
/// fragment.
fn tail_start(stem: &str, tokens: &[Token], markers: &[(usize, usize)]) -> Option<usize> {
    let mut start = None;
    for token in tokens.iter().rev() {
        if markers
            .iter()
            .any(|(from, to)| *from < token.end && token.start < *to)
        {
            break;
        }
        let text = &stem[token.start..token.end];
        let tag = is_tag(text);
        if !tag && !is_fragment(text) {
            break;
        }
        if tag {
            start = Some(token.start);
        }
    }
    start
}

/// Scans a masked stem for every marker it states, reporting where they were.
fn scan_markers(stem: &str, masked: &[u8], scan: &mut Scan) -> Vec<(usize, usize)> {
    let bytes = stem.as_bytes();
    let mut markers = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if masked.get(index) == Some(&MASK) {
            index += 1;
            continue;
        }
        let Some(marker) = match_marker(stem, masked, index) else {
            index += 1;
            continue;
        };
        index = marker.end.max(index + 1);
        markers.push((marker.start, marker.end));
        scan.record(marker.start, marker.kind);
    }
    markers
}

/// Tries every marker shape at one byte offset.
fn match_marker(stem: &str, masked: &[u8], index: usize) -> Option<Marker> {
    let bytes = stem.as_bytes();
    if let Some((end, season, span)) = match_season_episode(bytes, index) {
        return Some(Marker {
            start: index,
            end,
            kind: Kind::SeasonEpisode { season, span },
        });
    }
    if let Some((end, season)) = match_season(bytes, index) {
        return Some(Marker {
            start: index,
            end,
            kind: Kind::Season {
                season,
                episode: None,
            },
        });
    }
    if let Some((end, season, episode)) = match_season_word(bytes, index) {
        return Some(Marker {
            start: index,
            end,
            kind: Kind::Season { season, episode },
        });
    }
    if let Some((end, number)) = match_episode_word(bytes, index) {
        return Some(Marker {
            start: index,
            end,
            kind: Kind::Prefixed { number },
        });
    }
    if let Some((end, value)) = match_bracketed_year(bytes, index) {
        return Some(Marker {
            start: index,
            end,
            kind: Kind::Year {
                value,
                definite: true,
            },
        });
    }
    if let Some((end, season, span)) = match_cross_season(bytes, index) {
        return Some(Marker {
            start: index,
            end,
            kind: Kind::CrossSeason { season, span },
        });
    }
    match_number(bytes, masked, index)
}

/// Reads at most `max` ASCII digits, refusing a longer run.
fn read_number(bytes: &[u8], start: usize, max: usize) -> Option<(u32, usize)> {
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

/// True when a byte is part of a word, so a marker may not start or end beside it.
fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte >= 0x80
}

fn match_season_episode(bytes: &[u8], index: usize) -> Option<(usize, u32, EpisodeSpan)> {
    if !matches!(bytes.get(index), Some(b's' | b'S')) {
        return None;
    }
    if index > 0 && is_word_byte(bytes[index - 1]) {
        return None;
    }
    let (season, after_season) = read_number(bytes, index + 1, 4)?;
    if !matches!(bytes.get(after_season), Some(b'e' | b'E')) {
        return None;
    }
    let (first, mut end) = read_number(bytes, after_season + 1, 4)?;
    let mut span = EpisodeSpan::new(first, first);
    while let Some((episode, next)) = match_episode_tail(bytes, end) {
        span = EpisodeSpan::new(span.first().min(episode), span.last().max(episode));
        end = next;
    }
    Some((end, season, span))
}

/// Matches the `E03`, `-E03`, or `-03` tail of a multi-episode marker.
fn match_episode_tail(bytes: &[u8], index: usize) -> Option<(u32, usize)> {
    let mut cursor = index;
    if bytes.get(cursor) == Some(&b'-') {
        cursor += 1;
    } else if !matches!(bytes.get(cursor), Some(b'e' | b'E')) {
        return None;
    }
    if matches!(bytes.get(cursor), Some(b'e' | b'E')) {
        cursor += 1;
    }
    let (episode, end) = read_number(bytes, cursor, 4)?;
    // A letter after the digits means the run was not an episode (`-1080p`), except the `E` of
    // a following `E07`.
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
    if index > 0 && is_word_byte(bytes[index - 1]) {
        return None;
    }
    let (season, end) = read_number(bytes, index + 1, 4)?;
    if bytes.get(end).is_some_and(u8::is_ascii_alphanumeric) {
        return None;
    }
    Some((end, season))
}

/// Matches `Season 1`, `Season.1`, and `Season 1 Episode 2`.
fn match_season_word(bytes: &[u8], index: usize) -> Option<(usize, u32, Option<u32>)> {
    const WORD: &[u8] = b"season";
    if index > 0 && is_word_byte(bytes[index - 1]) {
        return None;
    }
    let end = index + WORD.len();
    if !bytes.get(index..end)?.eq_ignore_ascii_case(WORD) {
        return None;
    }
    if bytes.get(end).is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    let (season, after) = read_number(bytes, skip_separators(bytes, end), 3)?;
    let mut end = after;
    let mut episode = None;
    let next = skip_separators(bytes, after);
    if let Some((tail, number)) = match_episode_word(bytes, next) {
        episode = Some(number);
        end = tail;
    }
    Some((end, season, episode))
}

/// Matches `EP12`, `E12`, `Episode 12`, or `Ep.12`.
///
/// A spelled-out prefix may be separated from its number by anything, but the single letter `E`
/// may not be separated by a space: `E - 001` is far more likely to be a title that ends in the
/// letter than an episode marker, so that spelling falls through to the bare number.
fn match_episode_word(bytes: &[u8], index: usize) -> Option<(usize, u32)> {
    if index > 0 && is_word_byte(bytes[index - 1]) {
        return None;
    }
    let mut cursor = index;
    let spelled = if bytes
        .get(cursor..cursor + 7)
        .is_some_and(|word| word.eq_ignore_ascii_case(b"episode"))
    {
        cursor += 7;
        true
    } else if bytes
        .get(cursor..cursor + 2)
        .is_some_and(|word| word.eq_ignore_ascii_case(b"ep"))
    {
        cursor += 2;
        true
    } else if matches!(bytes.get(cursor), Some(b'e' | b'E')) {
        cursor += 1;
        false
    } else {
        return None;
    };
    let digits = if spelled {
        skip_separators(bytes, cursor)
    } else {
        skip_attached(bytes, cursor)
    };
    let (number, end) = read_number(bytes, digits, 4)?;
    if bytes.get(end).is_some_and(u8::is_ascii_alphanumeric) {
        return None;
    }
    Some((end, number))
}

/// Matches a year in brackets, which is the one placement that is never a title numeral.
fn match_bracketed_year(bytes: &[u8], index: usize) -> Option<(usize, u16)> {
    let close = match bytes.get(index) {
        Some(b'(') => b')',
        Some(b'[') => b']',
        _ => return None,
    };
    let (value, end) = read_number(bytes, index + 1, 4)?;
    if bytes.get(end) != Some(&close) || !(1900..=2099).contains(&value) {
        return None;
    }
    Some((end + 1, u16::try_from(value).ok()?))
}

/// Matches `<season>x<episode>`, refusing a resolution dimension.
fn match_cross_season(bytes: &[u8], index: usize) -> Option<(usize, u32, EpisodeSpan)> {
    let (season, after) = read_number(bytes, index, 2)?;
    if index > 0 && is_word_byte(bytes[index - 1]) {
        return None;
    }
    if !matches!(bytes.get(after), Some(b'x' | b'X')) {
        return None;
    }
    let (episode, end) = read_number(bytes, after + 1, 3)?;
    if bytes.get(end).is_some_and(|byte| is_word_byte(*byte)) {
        return None;
    }
    Some((end, season, EpisodeSpan::new(episode, episode)))
}

/// Matches a bare number, deciding whether it is a year, a running episode, or a title numeral.
fn match_number(bytes: &[u8], masked: &[u8], start: usize) -> Option<Marker> {
    if !bytes.get(start)?.is_ascii_digit() {
        return None;
    }
    let end = digit_end(bytes, start);
    if end - start > 4 || touches_word(bytes, start, end) {
        return None;
    }
    if follows_dimension(bytes, end)
        || follows_short_decimal(bytes, end)
        || follows_decimal_tail(bytes, start, end)
    {
        return None;
    }
    let value = std::str::from_utf8(&bytes[start..end]).ok()?.parse().ok()?;
    let whole = is_only_content(masked, start, end);
    if !whole && !has_content_before(bytes, start) {
        // A number with nothing before it is a title numeral (`1917`, `12 Monkeys`), never the
        // year of a name whose title is missing.
        return None;
    }
    if end - start == 4 && (1900..=2099).contains(&value) {
        return Some(Marker {
            start,
            end,
            kind: Kind::Year {
                value: u16::try_from(value).ok()?,
                definite: !is_trailing(masked, end),
            },
        });
    }
    let field = field_separated(bytes, start, end);
    let separated = field || dot_separated(bytes, start);
    if !whole && !field && follows_year(bytes, end) {
        // `Oceans.11.2001`: the year is the title's end, so the number before it belongs to it.
        return None;
    }
    if !whole && !separated && follows_title_word(masked, end) {
        // `Gundam 0083 Stardust Memory`: a number with title words after it is part of them.
        return None;
    }
    Some(Marker {
        start,
        end,
        kind: Kind::Bare(Bare {
            number: value,
            whole,
            separated,
        }),
    })
}

fn digit_end(bytes: &[u8], start: usize) -> usize {
    let mut end = start;
    while bytes.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
    }
    end
}

/// True when a word touches the number, so the number is part of that word.
fn touches_word(bytes: &[u8], start: usize, end: usize) -> bool {
    start
        .checked_sub(1)
        .and_then(|index| bytes.get(index))
        .is_some_and(|byte| is_word_byte(*byte))
        || bytes.get(end).is_some_and(|byte| is_word_byte(*byte))
}

/// True when the number is the width of a `1920x1080` pair.
fn follows_dimension(bytes: &[u8], end: usize) -> bool {
    matches!(bytes.get(end), Some(b'x' | b'X'))
        && bytes.get(end + 1).is_some_and(u8::is_ascii_digit)
}

/// True when the number is the first part of a channel layout such as `5.1` or `5.1.4`.
fn follows_short_decimal(bytes: &[u8], end: usize) -> bool {
    if bytes.get(end) != Some(&b'.') {
        return false;
    }
    let Some((_, after)) = read_number(bytes, end + 1, 2) else {
        return false;
    };
    bytes
        .get(after)
        .is_none_or(|byte| is_break(*byte) || *byte == b'-')
}

/// True when nothing but separators sits before the number, so it is not a year.
// The tag mask is deliberately not consulted: a title that is spelled like a tag (`1917` before
// `2019`) is still content a year may not claim the start of.
fn has_content_before(bytes: &[u8], start: usize) -> bool {
    bytes
        .get(..start)
        .unwrap_or_default()
        .iter()
        .any(|byte| !is_break(*byte) && *byte != b'-')
}

/// True when the number is all the component states.
fn is_only_content(masked: &[u8], start: usize, end: usize) -> bool {
    masked.iter().enumerate().all(|(index, byte)| {
        (start..end).contains(&index) || *byte == MASK || is_break(*byte) || *byte == b'-'
    })
}

/// True when only separators follow the number.
fn is_trailing(masked: &[u8], end: usize) -> bool {
    skip_separators(masked, end) >= masked.len()
}

/// True when a four-digit year follows, which puts the number inside the title.
fn follows_year(bytes: &[u8], end: usize) -> bool {
    let index = skip_separators(bytes, end);
    if !bytes.get(index).is_some_and(u8::is_ascii_digit) {
        return false;
    }
    let Some((value, after)) = read_number(bytes, index, 4) else {
        return false;
    };
    (1900..=2099).contains(&value)
        && after - index == 4
        && !bytes.get(after).is_some_and(|byte| is_word_byte(*byte))
}

/// True when a title word follows, so the number is part of the title.
fn follows_title_word(masked: &[u8], end: usize) -> bool {
    match masked.get(skip_separators(masked, end)) {
        None | Some(b'#') => false,
        Some(byte) if byte.is_ascii_digit() => false,
        Some(_) => true,
    }
}

/// True when a dash or an underscore separates the number from the title.
fn field_separated(bytes: &[u8], start: usize, end: usize) -> bool {
    let before = start.checked_sub(1).and_then(|index| bytes.get(index));
    let dotted = match before {
        Some(b'-' | b'_') => true,
        Some(b' ' | b'\t') => start
            .checked_sub(2)
            .and_then(|index| bytes.get(index))
            .is_some_and(|byte| matches!(byte, b'-' | b'_')),
        _ => false,
    };
    dotted || matches!(bytes.get(skip_whitespace(bytes, end)), Some(b'-' | b'_'))
}

/// True when a dot separates the number from the title.
fn dot_separated(bytes: &[u8], start: usize) -> bool {
    start
        .checked_sub(1)
        .is_some_and(|dot| bytes.get(dot) == Some(&b'.'))
}

/// True when the number is the tail of a decimal such as the `1` of `5.1`.
// Only one or two digits: `1917.2019` is a title numeral followed by a year, not a decimal.
fn follows_decimal_tail(bytes: &[u8], start: usize, end: usize) -> bool {
    if end - start > 2 {
        return false;
    }
    let Some(dot) = start.checked_sub(1) else {
        return false;
    };
    bytes.get(dot) == Some(&b'.')
        && dot
            .checked_sub(1)
            .and_then(|index| bytes.get(index))
            .is_some_and(u8::is_ascii_digit)
}

fn skip_separators(bytes: &[u8], mut index: usize) -> usize {
    while bytes
        .get(index)
        .is_some_and(|byte| is_break(*byte) || *byte == b'-')
    {
        index += 1;
    }
    index
}

/// Skips only the separators that attach one spelling to the next (`E.12`, `E-12`, `E_12`).
fn skip_attached(bytes: &[u8], mut index: usize) -> usize {
    while bytes
        .get(index)
        .is_some_and(|byte| matches!(byte, b'.' | b'-' | b'_'))
    {
        index += 1;
    }
    index
}

fn skip_whitespace(bytes: &[u8], mut index: usize) -> usize {
    while bytes
        .get(index)
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        index += 1;
    }
    index
}

/// Cleans a title of separators, collapsing runs and trimming decoration.
///
/// A hyphen is kept where it is part of a word (`Spider-Man`); it only disappears when the cut
/// left it dangling at the end of the title (`Show - `), which is what the trim does.
fn clean_title(prefix: &str) -> Option<String> {
    let mut cleaned = String::with_capacity(prefix.len());
    let mut at_separator = true;
    for character in prefix.chars() {
        if is_break_char(character) {
            at_separator = true;
            continue;
        }
        if at_separator && !cleaned.is_empty() {
            cleaned.push(' ');
        }
        at_separator = false;
        cleaned.push(character);
    }
    let trimmed = cleaned.trim_end_matches(|character: char| {
        character.is_whitespace() || matches!(character, '-' | ':' | ',' | '.' | '_' | '+')
    });
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// The last number of a season pair, order-independent and duplicate-free.
fn distinct_spans(spans: &[EpisodeSpan]) -> Vec<EpisodeSpan> {
    let mut distinct = Vec::new();
    for span in spans {
        if !distinct.contains(span) {
            distinct.push(*span);
        }
    }
    distinct
}

/// Keeps `slot`'s value unless the incoming one is preferred or the slot is empty.
fn take<T>(slot: &mut Option<T>, value: Option<T>, prefer: bool) {
    if prefer {
        if value.is_some() {
            *slot = value;
        }
    } else if slot.is_none() {
        *slot = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release::{
        AudioCodec, HdrFormat, ReleaseSource, RepackKind, Resolution, VideoCodec,
    };
    use proptest::prelude::*;

    /// What one corpus row expects. Every field is asserted for every row, so a row that does
    /// not state a field leaves it at [`Expected::NONE`].
    #[derive(Clone, Copy)]
    struct Expected {
        title: Option<&'static str>,
        year: Option<u16>,
        season: Option<u32>,
        episode: Option<(u32, u32)>,
        absolute: Option<u32>,
        numbering: Option<Numbering>,
        extension: Option<&'static str>,
    }

    impl Expected {
        const NONE: Self = Self {
            title: None,
            year: None,
            season: None,
            episode: None,
            absolute: None,
            numbering: None,
            extension: None,
        };
    }

    const MOVIES: &[(&str, Expected)] = &[
        (
            "The Matrix (1999) [1080p]",
            Expected {
                title: Some("The Matrix"),
                year: Some(1999),
                ..Expected::NONE
            },
        ),
        (
            "The.Matrix.1999.1080p.BluRay.x264-GROUP",
            Expected {
                title: Some("The Matrix"),
                year: Some(1999),
                ..Expected::NONE
            },
        ),
        (
            "The.Matrix.1999.1080p.mkv",
            Expected {
                title: Some("The Matrix"),
                year: Some(1999),
                extension: Some("mkv"),
                ..Expected::NONE
            },
        ),
        (
            "Blade Runner (1982) [4K]",
            Expected {
                title: Some("Blade Runner"),
                year: Some(1982),
                ..Expected::NONE
            },
        ),
        (
            "Dune.2021.2160p.WEB-DL.DDP5.1.H.265-GROUP",
            Expected {
                title: Some("Dune"),
                year: Some(2021),
                ..Expected::NONE
            },
        ),
        (
            "The Grand Budapest Hotel (2014) [1080p]",
            Expected {
                title: Some("The Grand Budapest Hotel"),
                year: Some(2014),
                ..Expected::NONE
            },
        ),
        (
            "Spider-Man.2002.1080p.BluRay.x264",
            Expected {
                title: Some("Spider-Man"),
                year: Some(2002),
                ..Expected::NONE
            },
        ),
        (
            "Amélie.2001.1080p.BluRay.x264.mkv",
            Expected {
                title: Some("Amélie"),
                year: Some(2001),
                extension: Some("mkv"),
                ..Expected::NONE
            },
        ),
        (
            "Матрица.1999.1080p.BluRay.x264-GROUP",
            Expected {
                title: Some("Матрица"),
                year: Some(1999),
                ..Expected::NONE
            },
        ),
        (
            "映画 (2018) [1080p]",
            Expected {
                title: Some("映画"),
                year: Some(2018),
                ..Expected::NONE
            },
        ),
        (
            "Le.Fabuleux.Destin.d'Amélie.Poulain.2001.1080p.BluRay.x264-GROUP",
            Expected {
                title: Some("Le Fabuleux Destin d'Amélie Poulain"),
                year: Some(2001),
                ..Expected::NONE
            },
        ),
        (
            "1917.2019.1080p.BluRay.x264-GROUP",
            Expected {
                title: Some("1917"),
                year: Some(2019),
                ..Expected::NONE
            },
        ),
        (
            "12.Monkeys.1995.1080p.BluRay.x264-GROUP",
            Expected {
                title: Some("12 Monkeys"),
                year: Some(1995),
                ..Expected::NONE
            },
        ),
        (
            "Oceans.11.2001.1080p.BluRay.x264-GROUP",
            Expected {
                title: Some("Oceans 11"),
                year: Some(2001),
                ..Expected::NONE
            },
        ),
        (
            "2001 A Space Odyssey",
            Expected {
                title: Some("2001 A Space Odyssey"),
                ..Expected::NONE
            },
        ),
        (
            "Some Random File Name",
            Expected {
                title: Some("Some Random File Name"),
                ..Expected::NONE
            },
        ),
        (
            "Show.mkv",
            Expected {
                title: Some("Show"),
                extension: Some("mkv"),
                ..Expected::NONE
            },
        ),
    ];

    const EPISODES: &[(&str, Expected)] = &[
        (
            "Show.Name.S01E02.1080p.WEB-DL.DDP5.1.H.264-NTb",
            Expected {
                title: Some("Show Name"),
                season: Some(1),
                episode: Some((2, 2)),
                numbering: Some(Numbering::SeasonEpisode),
                ..Expected::NONE
            },
        ),
        (
            "Show.Name.S01E01E02.1080p.WEB-DL.x264-GROUP",
            Expected {
                title: Some("Show Name"),
                season: Some(1),
                episode: Some((1, 2)),
                numbering: Some(Numbering::SeasonEpisode),
                ..Expected::NONE
            },
        ),
        (
            "Show.Name.S01E01-E03.1080p.WEB-DL.x264-GROUP",
            Expected {
                title: Some("Show Name"),
                season: Some(1),
                episode: Some((1, 3)),
                numbering: Some(Numbering::SeasonEpisode),
                ..Expected::NONE
            },
        ),
        (
            "Show.Name.S02E05E06E07.1080p.WEB-DL.x264-GROUP",
            Expected {
                title: Some("Show Name"),
                season: Some(2),
                episode: Some((5, 7)),
                numbering: Some(Numbering::SeasonEpisode),
                ..Expected::NONE
            },
        ),
        (
            "Show.Name.S10E20-E22.1080p.WEB-DL.x264-GROUP",
            Expected {
                title: Some("Show Name"),
                season: Some(10),
                episode: Some((20, 22)),
                numbering: Some(Numbering::SeasonEpisode),
                ..Expected::NONE
            },
        ),
        (
            "Show.Name.S01.1080p.BluRay.x264-GROUP",
            Expected {
                title: Some("Show Name"),
                season: Some(1),
                numbering: Some(Numbering::SeasonEpisode),
                ..Expected::NONE
            },
        ),
        (
            "Show.Name.Season.2.720p.HDTV.x264-GROUP",
            Expected {
                title: Some("Show Name"),
                season: Some(2),
                numbering: Some(Numbering::SeasonEpisode),
                ..Expected::NONE
            },
        ),
        (
            "Show Name - Season 1 Episode 2 - The Title",
            Expected {
                title: Some("Show Name"),
                season: Some(1),
                episode: Some((2, 2)),
                numbering: Some(Numbering::SeasonEpisode),
                ..Expected::NONE
            },
        ),
        (
            "Show.Name.s01e02.1080p",
            Expected {
                title: Some("Show Name"),
                season: Some(1),
                episode: Some((2, 2)),
                numbering: Some(Numbering::SeasonEpisode),
                ..Expected::NONE
            },
        ),
        (
            "Show.Name.S01E02.mkv",
            Expected {
                title: Some("Show Name"),
                season: Some(1),
                episode: Some((2, 2)),
                numbering: Some(Numbering::SeasonEpisode),
                extension: Some("mkv"),
                ..Expected::NONE
            },
        ),
        (
            "Show.Name.2020.S01E02.1080p",
            Expected {
                title: Some("Show Name"),
                year: Some(2020),
                season: Some(1),
                episode: Some((2, 2)),
                numbering: Some(Numbering::SeasonEpisode),
                ..Expected::NONE
            },
        ),
        (
            "Show/S01E02.mkv",
            Expected {
                title: Some("Show"),
                season: Some(1),
                episode: Some((2, 2)),
                numbering: Some(Numbering::SeasonEpisode),
                extension: Some("mkv"),
                ..Expected::NONE
            },
        ),
    ];

    const CROSSED: &[(&str, Expected)] = &[
        (
            "Show Name - 1x02 - Episode Title",
            Expected {
                title: Some("Show Name"),
                season: Some(1),
                episode: Some((2, 2)),
                numbering: Some(Numbering::CrossSeason),
                ..Expected::NONE
            },
        ),
        (
            "Show.1x02.1080p.WEB-DL",
            Expected {
                title: Some("Show"),
                season: Some(1),
                episode: Some((2, 2)),
                numbering: Some(Numbering::CrossSeason),
                ..Expected::NONE
            },
        ),
        (
            "Show 2x05",
            Expected {
                title: Some("Show"),
                season: Some(2),
                episode: Some((5, 5)),
                numbering: Some(Numbering::CrossSeason),
                ..Expected::NONE
            },
        ),
        (
            "Show - 10x20 - Episode Title",
            Expected {
                title: Some("Show"),
                season: Some(10),
                episode: Some((20, 20)),
                numbering: Some(Numbering::CrossSeason),
                ..Expected::NONE
            },
        ),
        (
            "Show_Name_3x07_1080p",
            Expected {
                title: Some("Show Name"),
                season: Some(3),
                episode: Some((7, 7)),
                numbering: Some(Numbering::CrossSeason),
                ..Expected::NONE
            },
        ),
        (
            "Show.1x01.mkv",
            Expected {
                title: Some("Show"),
                season: Some(1),
                episode: Some((1, 1)),
                numbering: Some(Numbering::CrossSeason),
                extension: Some("mkv"),
                ..Expected::NONE
            },
        ),
        (
            "Show 0x01",
            Expected {
                title: Some("Show"),
                season: Some(0),
                episode: Some((1, 1)),
                numbering: Some(Numbering::CrossSeason),
                ..Expected::NONE
            },
        ),
    ];

    const ABSOLUTE: &[(&str, Expected)] = &[
        (
            "Show Name - 012",
            Expected {
                title: Some("Show Name"),
                absolute: Some(12),
                numbering: Some(Numbering::BareAbsolute),
                ..Expected::NONE
            },
        ),
        (
            "Show.Name.012.1080p.BluRay.x264-GROUP",
            Expected {
                title: Some("Show Name"),
                absolute: Some(12),
                numbering: Some(Numbering::BareAbsolute),
                ..Expected::NONE
            },
        ),
        (
            "Show.Name - 012 [1080p]",
            Expected {
                title: Some("Show Name"),
                absolute: Some(12),
                numbering: Some(Numbering::BareAbsolute),
                ..Expected::NONE
            },
        ),
        (
            "Show_Name_012",
            Expected {
                title: Some("Show Name"),
                absolute: Some(12),
                numbering: Some(Numbering::BareAbsolute),
                ..Expected::NONE
            },
        ),
        (
            "Show-012",
            Expected {
                title: Some("Show"),
                absolute: Some(12),
                numbering: Some(Numbering::BareAbsolute),
                ..Expected::NONE
            },
        ),
        (
            "Show Name - 012 - Episode Title",
            Expected {
                title: Some("Show Name"),
                absolute: Some(12),
                numbering: Some(Numbering::BareAbsolute),
                ..Expected::NONE
            },
        ),
        (
            "Show.EP12.1080p",
            Expected {
                title: Some("Show"),
                absolute: Some(12),
                numbering: Some(Numbering::PrefixedAbsolute),
                ..Expected::NONE
            },
        ),
        (
            "Show.E012.1080p",
            Expected {
                title: Some("Show"),
                absolute: Some(12),
                numbering: Some(Numbering::PrefixedAbsolute),
                ..Expected::NONE
            },
        ),
        (
            "Show - Episode 12 - Episode Title",
            Expected {
                title: Some("Show"),
                absolute: Some(12),
                numbering: Some(Numbering::PrefixedAbsolute),
                ..Expected::NONE
            },
        ),
        (
            "Show/012.mkv",
            Expected {
                title: Some("Show"),
                absolute: Some(12),
                numbering: Some(Numbering::BareAbsolute),
                extension: Some("mkv"),
                ..Expected::NONE
            },
        ),
        (
            "Show/012",
            Expected {
                title: Some("Show"),
                absolute: Some(12),
                numbering: Some(Numbering::BareAbsolute),
                ..Expected::NONE
            },
        ),
        (
            "Show/Season 1/01.mkv",
            Expected {
                title: Some("Show"),
                season: Some(1),
                episode: Some((1, 1)),
                numbering: Some(Numbering::SeasonEpisode),
                extension: Some("mkv"),
                ..Expected::NONE
            },
        ),
        (
            "Show/Season 02/12.mkv",
            Expected {
                title: Some("Show"),
                season: Some(2),
                episode: Some((12, 12)),
                numbering: Some(Numbering::SeasonEpisode),
                extension: Some("mkv"),
                ..Expected::NONE
            },
        ),
    ];

    const FOLDERS: &[(&str, Expected)] = &[
        (
            "Show/Season 01/Show - S01E02.mkv",
            Expected {
                title: Some("Show"),
                season: Some(1),
                episode: Some((2, 2)),
                numbering: Some(Numbering::SeasonEpisode),
                extension: Some("mkv"),
                ..Expected::NONE
            },
        ),
        (
            "Show\\Season 01\\Show - S01E02.mkv",
            Expected {
                title: Some("Show"),
                season: Some(1),
                episode: Some((2, 2)),
                numbering: Some(Numbering::SeasonEpisode),
                extension: Some("mkv"),
                ..Expected::NONE
            },
        ),
        (
            "Movie Title (2005)/Movie Title.mkv",
            Expected {
                title: Some("Movie Title"),
                year: Some(2005),
                extension: Some("mkv"),
                ..Expected::NONE
            },
        ),
        (
            "Movie Title (2005)/Movie Title (2005) [1080p].mkv",
            Expected {
                title: Some("Movie Title"),
                year: Some(2005),
                extension: Some("mkv"),
                ..Expected::NONE
            },
        ),
        (
            "Show/Season 01",
            Expected {
                title: Some("Show"),
                season: Some(1),
                numbering: Some(Numbering::SeasonEpisode),
                ..Expected::NONE
            },
        ),
        (
            "Show/Season 01/08.mkv",
            Expected {
                title: Some("Show"),
                season: Some(1),
                episode: Some((8, 8)),
                numbering: Some(Numbering::SeasonEpisode),
                extension: Some("mkv"),
                ..Expected::NONE
            },
        ),
        (
            "Shows/The Office (2005)/Season 03/The Office S03E04 HDTV.mkv",
            Expected {
                title: Some("The Office"),
                year: Some(2005),
                season: Some(3),
                episode: Some((4, 4)),
                numbering: Some(Numbering::SeasonEpisode),
                extension: Some("mkv"),
                ..Expected::NONE
            },
        ),
        (
            "Shows/Breaking Bad (2008)/Season 05/Breaking Bad S05E14 Ozymandias.mkv",
            Expected {
                title: Some("Breaking Bad"),
                year: Some(2008),
                season: Some(5),
                episode: Some((14, 14)),
                numbering: Some(Numbering::SeasonEpisode),
                extension: Some("mkv"),
                ..Expected::NONE
            },
        ),
        (
            "The Matrix (1999)/",
            Expected {
                title: Some("The Matrix"),
                year: Some(1999),
                ..Expected::NONE
            },
        ),
    ];

    fn corpus(rows: &[(&str, Expected)]) {
        for &(name, expected) in rows {
            let parsed = match LibraryName::parse(name) {
                Ok(parsed) => parsed,
                Err(error) => panic!("`{name}` should resolve, but {error}"),
            };
            assert_eq!(parsed.title(), expected.title, "title of `{name}`");
            assert_eq!(parsed.year(), expected.year, "year of `{name}`");
            assert_eq!(parsed.season(), expected.season, "season of `{name}`");
            assert_eq!(
                parsed.episodes().map(|span| (span.first(), span.last())),
                expected.episode,
                "episode of `{name}`"
            );
            assert_eq!(parsed.absolute(), expected.absolute, "absolute of `{name}`");
            assert_eq!(
                parsed.numbering(),
                expected.numbering,
                "numbering of `{name}`"
            );
            assert_eq!(
                parsed.extension(),
                expected.extension,
                "extension of `{name}`"
            );
            assert_eq!(parsed.raw(), name, "raw text of `{name}`");
        }
    }

    #[test]
    fn parses_movie_names() {
        corpus(MOVIES);
    }

    #[test]
    fn parses_scene_episode_names() {
        corpus(EPISODES);
    }

    #[test]
    fn parses_the_crossed_season_form() {
        corpus(CROSSED);
    }

    #[test]
    fn parses_anime_absolute_names() {
        corpus(ABSOLUTE);
    }

    #[test]
    fn parses_folder_names() {
        corpus(FOLDERS);
    }

    #[test]
    fn reuses_the_release_parser_for_the_tail() {
        let raw = "The.Matrix.1999.2160p.UHD.BluRay.REMUX.HDR.HEVC.TrueHD.7.1.Atmos-GROUP";
        let parsed = LibraryName::parse(raw).expect("resolves");
        let release = parsed.release();
        assert_eq!(release.raw(), raw);
        assert_eq!(release.resolution(), Some(Resolution::UltraHd));
        assert_eq!(release.source(), Some(ReleaseSource::Remux));
        assert_eq!(release.video_codec(), Some(VideoCodec::H265));
        assert_eq!(release.hdr(), Some(HdrFormat::Hdr10));
        assert_eq!(release.audio(), [AudioCodec::TrueHd, AudioCodec::Atmos]);
        assert_eq!(release.group(), Some("GROUP"));
        assert_eq!(parsed.title(), Some("The Matrix"));
        assert_eq!(parsed.year(), Some(1999));
    }

    #[test]
    fn a_version_marker_is_reported_through_the_release_tail() {
        // No table has an edition column, so the marker is only visible through `ReleaseName`.
        let parsed = LibraryName::parse("Movie.Title.2019.1080p.BluRay.x264.REPACK-GROUP")
            .expect("resolves");
        assert_eq!(parsed.release().repack(), Some(RepackKind::Repack));
        assert_eq!(parsed.title(), Some("Movie Title"));
    }

    #[test]
    fn a_group_hyphen_does_not_change_the_title_or_the_number() {
        // `ReleaseName` loses the group after a hyphenated audio token (PG-E4-03). The library
        // parse never reads the group, so this name still cuts at its year.
        let parsed = LibraryName::parse("Movie.2024.2160p.BluRay.DTS-HD-GROUP").expect("resolves");
        assert_eq!(parsed.title(), Some("Movie"));
        assert_eq!(parsed.year(), Some(2024));
        assert_eq!(parsed.release().resolution(), Some(Resolution::UltraHd));
    }

    #[test]
    fn a_parsed_name_serializes_to_its_fields() {
        let parsed = LibraryName::parse("Show.Name.S01E02.1080p.WEB-DL.mkv").expect("resolves");
        let json = serde_json::to_value(&parsed).expect("serializes");
        assert_eq!(json["title"], "Show Name");
        assert_eq!(json["season"], 1);
        assert_eq!(json["numbering"], "season_episode");
        assert_eq!(json["extension"], "mkv");
    }

    #[test]
    fn a_dimension_is_not_the_crossed_season_form() {
        let parsed = LibraryName::parse("Show.1920x1080.1080p").expect("resolves");
        assert_eq!(parsed.title(), Some("Show"));
        assert_eq!(parsed.season(), None);
        assert_eq!(parsed.episodes(), None);
    }

    #[test]
    fn a_channel_layout_and_a_codec_are_not_episode_numbers() {
        let inputs = [
            "Show.Name.1080p.WEB-DL.DDP5.1.H.264",
            "Show.Name.5.1",
            "Show.Name.TrueHD.7.1.Atmos",
        ];
        for text in inputs {
            let parsed = LibraryName::parse(text).expect("resolves");
            assert_eq!(parsed.absolute(), None, "absolute of `{text}`");
            assert_eq!(parsed.numbering(), None, "numbering of `{text}`");
        }
    }

    #[test]
    fn an_extension_is_read_only_when_it_names_a_container() {
        assert_eq!(
            LibraryName::parse("Show - S01E02.MKV")
                .expect("resolves")
                .extension(),
            Some("mkv")
        );
        assert_eq!(
            LibraryName::parse("The.Matrix.1999.1080p.BluRay.x264-GROUP")
                .expect("resolves")
                .extension(),
            None
        );
        assert_eq!(
            LibraryName::parse("Show.Name")
                .expect("resolves")
                .extension(),
            None
        );
    }

    #[test]
    fn a_non_ascii_title_is_kept_whole() {
        let cases = [
            ("Матрица.1999.1080p.BluRay.x264-GROUP", "Матрица"),
            ("映画 (2018) [1080p]", "映画"),
            ("Amélie (2001) [1080p]", "Amélie"),
            ("L'homme.du.rail.2019.1080p", "L'homme du rail"),
        ];
        for (text, title) in cases {
            assert_eq!(
                LibraryName::parse(text).expect("resolves").title(),
                Some(title),
                "{text}"
            );
        }
    }

    #[test]
    fn a_direction_override_stays_inside_the_title() {
        let parsed = LibraryName::parse("Show.\u{202e}Name.S01E02.mkv").expect("resolves");
        assert_eq!(parsed.title(), Some("Show \u{202e}Name"));
        assert_eq!(parsed.episodes().map(EpisodeSpan::first), Some(2));
    }

    #[test]
    fn numbering_is_the_shape_evidence() {
        let cases = [
            ("Show.Name.S01E02.1080p", Numbering::SeasonEpisode),
            ("Show.Name.1x02.1080p", Numbering::CrossSeason),
            ("Show.Name.012.1080p.mkv", Numbering::BareAbsolute),
            ("Show.Name.EP12.1080p", Numbering::PrefixedAbsolute),
            ("Show.Name.S01.1080p", Numbering::SeasonEpisode),
        ];
        for (text, numbering) in cases {
            assert_eq!(
                LibraryName::parse(text).expect("resolves").numbering(),
                Some(numbering),
                "{text}"
            );
        }
    }

    #[test]
    fn a_year_never_decides_the_shape() {
        let film = LibraryName::parse("Movie Title (2005) [1080p]").expect("resolves");
        assert_eq!(film.title(), Some("Movie Title"));
        assert_eq!(film.year(), Some(2005));
        assert_eq!(film.numbering(), None, "a bracketed year is not numbering");

        let folder = LibraryName::parse("Show (2019)").expect("resolves");
        assert_eq!(folder.year(), Some(2019));
        assert_eq!(folder.numbering(), None, "a series folder looks the same");
    }

    #[test]
    fn a_year_is_read_when_it_is_bracketed_or_before_the_tags() {
        assert_eq!(
            LibraryName::parse("The Matrix (1999)")
                .expect("resolves")
                .year(),
            Some(1999)
        );
        assert_eq!(
            LibraryName::parse("The.Matrix.1999.1080p")
                .expect("resolves")
                .year(),
            Some(1999)
        );
    }

    #[test]
    fn a_number_inside_a_title_is_not_an_episode() {
        let cases = [
            "Mobile Suit Gundam 0083 Stardust Memory",
            "12 Monkeys",
            "2001 A Space Odyssey",
            "2 Fast 2 Furious",
        ];
        for text in cases {
            let parsed = LibraryName::parse(text).expect("resolves");
            assert_eq!(parsed.title(), Some(text), "title of `{text}`");
            assert_eq!(parsed.absolute(), None, "absolute of `{text}`");
            assert_eq!(parsed.numbering(), None, "numbering of `{text}`");
        }
    }

    #[test]
    fn a_space_separated_number_is_reported_rather_than_guessed() {
        let cases = [
            ("Show Name 012", "Show Name", 12),
            ("Ocean's 11", "Ocean's", 11),
            ("Mobile Suit Gundam 0083", "Mobile Suit Gundam", 83),
        ];
        for (text, title, number) in cases {
            let error = LibraryName::parse(text).expect_err("ambiguous");
            assert_eq!(
                error.reason,
                UnresolvedReason::AmbiguousAbsolute { number },
                "{text}"
            );
            assert_eq!(error.name.title(), Some(title), "{text}");
            assert_eq!(error.name.absolute(), None, "{text}");
        }
    }

    #[test]
    fn a_bare_trailing_year_is_reported_rather_than_guessed() {
        let cases = [
            ("Blade Runner 2049", "Blade Runner", 2049),
            ("The.Matrix.1999", "The Matrix", 1999),
            ("The Matrix 1999", "The Matrix", 1999),
        ];
        for (text, title, year) in cases {
            let error = LibraryName::parse(text).expect_err("ambiguous");
            assert_eq!(
                error.reason,
                UnresolvedReason::AmbiguousYear { year },
                "{text}"
            );
            assert_eq!(error.name.title(), Some(title), "{text}");
            assert_eq!(error.name.year(), None, "{text}");
        }
    }

    #[test]
    fn a_number_stated_twice_resolves_the_same_way() {
        let cases = [
            "Show.S01E02.S01E02.mkv",
            "Show/Season 01/Season 01/Show - S01E02.mkv",
        ];
        for text in cases {
            let parsed = LibraryName::parse(text).expect("resolves");
            assert_eq!(parsed.season(), Some(1), "{text}");
            assert_eq!(parsed.episodes().map(EpisodeSpan::first), Some(2), "{text}");
        }
    }

    #[test]
    fn two_candidate_seasons_are_rejected() {
        let cases = [
            "Show/Season 01/Season 02/Show - S01E02.mkv",
            "Show/Season 02/Show - S01E02.mkv",
            "Show.S02E03.1x04.mkv",
        ];
        for text in cases {
            let error = LibraryName::parse(text).expect_err("conflicting seasons");
            assert_eq!(
                error.reason,
                UnresolvedReason::ConflictingSeasons {
                    first: 2,
                    second: 1,
                },
                "{text}"
            );
            assert_eq!(error.name.title(), Some("Show"), "{text}");
        }
    }

    #[test]
    fn two_candidate_episodes_are_rejected() {
        let cases = ["Show.S01E02.S01E05.1080p.mkv", "Show.S01E02.1x05.mkv"];
        for text in cases {
            let error = LibraryName::parse(text).expect_err("conflicting episodes");
            assert_eq!(
                error.reason,
                UnresolvedReason::ConflictingEpisodes {
                    first: 2,
                    second: 5,
                },
                "{text}"
            );
        }
    }

    #[test]
    fn an_empty_name_is_unresolved() {
        for text in ["", "   ", "///", "\\\\", "\t\n"] {
            let error = LibraryName::parse(text).expect_err("empty");
            assert_eq!(error.reason, UnresolvedReason::Empty, "{text}");
            assert_eq!(error.name.title(), None, "{text}");
        }
    }

    #[test]
    fn a_name_with_no_title_is_unresolved() {
        let cases = [
            ("S01E02.mkv", Some(1), Some((2, 2))),
            (".mkv", None, None),
            ("1080p.BluRay.x264", None, None),
            ("Season 01/S01E02.mkv", Some(1), Some((2, 2))),
        ];
        for (text, season, episode) in cases {
            let error = LibraryName::parse(text).expect_err("no title");
            assert_eq!(error.reason, UnresolvedReason::NoTitle, "{text}");
            assert_eq!(error.name.title(), None, "{text}");
            assert_eq!(error.name.season(), season, "{text}");
            assert_eq!(
                error
                    .name
                    .episodes()
                    .map(|span| (span.first(), span.last())),
                episode,
                "{text}"
            );
        }
    }

    #[test]
    fn a_path_is_not_a_name() {
        let cases = [
            "..",
            "../../etc/passwd",
            "Show/../Movie.mkv",
            "Show/./Movie.mkv",
        ];
        for text in cases {
            let error = LibraryName::parse(text).expect_err("path text is refused");
            assert!(
                matches!(error.reason, UnresolvedReason::NotAName { .. }),
                "{text}: {error}"
            );
        }
    }

    #[test]
    fn an_absolute_path_parses_its_last_components() {
        let parsed = LibraryName::parse("/media/Movies/Movie Title (2005)/Movie Title.mkv")
            .expect("resolves");
        assert_eq!(parsed.title(), Some("Movie Title"));
        assert_eq!(parsed.year(), Some(2005));
        assert_eq!(parsed.extension(), Some("mkv"));
    }

    #[test]
    fn parse_never_panics_on_hostile_inputs() {
        let long = "a".repeat(200_000);
        let dots = ".".repeat(100_000);
        let markers = "S01E02-".repeat(10_000);
        let inputs = [
            "",
            ".",
            "..",
            "-",
            "....",
            "----",
            "[]()",
            "映画-🎬-S01E02-🎬",
            "\u{202e}",
            "\u{ff0f}\u{ff0e}",
            "1920x1080",
            "DDP5.1",
            "H.265",
            long.as_str(),
            dots.as_str(),
            markers.as_str(),
        ];
        for input in inputs {
            let _ = LibraryName::parse(input);
        }
    }

    proptest! {
        #[test]
        fn parse_never_panics_on_arbitrary_input(input in any::<String>()) {
            let _ = LibraryName::parse(&input);
        }

        #[test]
        fn parse_never_panics_on_separator_soup(input in "[._\\-\\[\\]() /\\\\]{0,512}") {
            let _ = LibraryName::parse(&input);
        }

        #[test]
        fn parse_is_deterministic(input in any::<String>()) {
            match (LibraryName::parse(&input), LibraryName::parse(&input)) {
                (Ok(first), Ok(second)) => prop_assert_eq!(first, second),
                (Err(first), Err(second)) => prop_assert_eq!(first, second),
                _ => prop_assert!(false, "the same input parsed two ways"),
            }
        }

        #[test]
        fn a_parsed_name_keeps_its_raw_text(input in any::<String>()) {
            match LibraryName::parse(&input) {
                Ok(name) => prop_assert_eq!(name.raw(), input.as_str()),
                Err(error) => prop_assert_eq!(error.name.raw(), input.as_str()),
            }
        }

        #[test]
        fn a_resolved_name_states_a_title(input in "[ -~]{0,64}") {
            if let Ok(name) = LibraryName::parse(&input) {
                prop_assert!(name.title().is_some_and(|title| !title.trim().is_empty()));
            }
        }

        #[test]
        fn a_generated_season_episode_name_parses(
            words in prop::collection::vec("[A-Za-z]{1,8}", 1..4),
            season in 1u32..40,
            episode in 1u32..200,
        ) {
            let title = words.join(" ");
            let text = format!("{title}.S{season:02}E{episode:02}.1080p.WEB-DL.x264-GROUP");
            let parsed = LibraryName::parse(&text).expect("a well-formed name resolves");
            prop_assert_eq!(parsed.title(), Some(title.as_str()));
            prop_assert_eq!(parsed.season(), Some(season));
            prop_assert_eq!(parsed.episodes().map(EpisodeSpan::first), Some(episode));
            prop_assert_eq!(parsed.numbering(), Some(Numbering::SeasonEpisode));
        }

        #[test]
        fn a_generated_crossed_name_parses(
            words in prop::collection::vec("[A-Za-z]{1,8}", 1..4),
            season in 1u32..30,
            episode in 1u32..100,
        ) {
            let title = words.join(" ");
            let text = format!("{title} - {season}x{episode:02} - An Episode");
            let parsed = LibraryName::parse(&text).expect("a well-formed name resolves");
            prop_assert_eq!(parsed.title(), Some(title.as_str()));
            prop_assert_eq!(parsed.season(), Some(season));
            prop_assert_eq!(parsed.episodes().map(EpisodeSpan::first), Some(episode));
            prop_assert_eq!(parsed.numbering(), Some(Numbering::CrossSeason));
        }

        #[test]
        fn a_generated_absolute_name_parses(
            words in prop::collection::vec("[A-Za-z]{1,8}", 1..4),
            episode in 1u32..1000,
        ) {
            let title = words.join(" ");
            let text = format!("{title} - {episode:03}");
            let parsed = LibraryName::parse(&text).expect("a well-formed name resolves");
            prop_assert_eq!(parsed.title(), Some(title.as_str()));
            prop_assert_eq!(parsed.absolute(), Some(episode));
            prop_assert_eq!(parsed.numbering(), Some(Numbering::BareAbsolute));
        }
    }
}
