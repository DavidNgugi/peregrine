//! The subtitle document model: cue timings, inline styling, and the three text formats.
//!
//! The model carries text, not bytes. Encoding detection and conversion to UTF-8 is
//! `PG-E4-17-06`'s job, so [`SubtitleDocument::parse`] takes a `&str` that a caller has
//! already decoded, and [`SubtitleDocument::render`] returns a `String` for a caller to
//! encode. Nothing here decides a charset, and nothing here reads a file.
//!
//! The model is lossless for text that is *cued*, and deliberately dumb about it: an inline
//! override — `{\i1}`, `\pos(640,360)`, `\an8` — is part of the cue's text and travels
//! through parse and render unchanged rather than being parsed into a structure and
//! rebuilt. Rebuilding it would mean re-encoding decisions the source already made, and the
//! ASS acceptance test is exactly that a styling-bearing cue comes back byte-for-byte.
//!
//! A document's *non-cue* lines — an ASS `[Script Info]` header, a `WebVTT` `NOTE` or `STYLE`
//! block, a leader a parser did not recognise — are kept verbatim in
//! [`SubtitleDocument::preamble`] and [`SubtitleDocument::suffix`], which is what makes "the
//! writer did not silently drop the script header" a test rather than a review.

use std::fmt::Write as _;

use super::timing::Millis;
use crate::library::SubtitleFormat;

/// The number of digits an ASS timestamp writes after the decimal point.
///
/// ASS files use two — the format is defined in centiseconds — but three exist in the wild
/// because `SubStation` tools have accepted them for years and ffmpeg writes them when
/// extracting. Both are preserved, because a cue's millisecond value does not always
/// survive being written with two digits and the writer has to be able to say so.
const ASS_DEFAULT_FRACTION_DIGITS: u8 = 2;
/// The most digits an ASS timestamp may carry: three is milliseconds, the resolution the
/// rest of this module works in.
const ASS_MAX_FRACTION_DIGITS: u8 = 3;

/// The timing of one subtitle cue.
///
/// Both ends are [`Millis`], and no relationship between them is enforced: a cue that ends
/// before it starts, or has zero duration, is a value this type holds rather than refuses. A
/// file in the wild contains both, and the honest thing to do with one is to keep it and let
/// the caller decide, not to lose a cue while parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubtitleTiming {
    start: Millis,
    end: Millis,
}

impl SubtitleTiming {
    /// Builds a timing from a start and an end.
    #[must_use]
    pub const fn new(start: Millis, end: Millis) -> Self {
        Self { start, end }
    }

    /// When the cue appears.
    #[must_use]
    pub const fn start(self) -> Millis {
        self.start
    }

    /// When the cue disappears.
    #[must_use]
    pub const fn end(self) -> Millis {
        self.end
    }

    /// How long the cue is shown; zero for a cue that ends before it starts.
    #[must_use]
    pub const fn duration(self) -> Millis {
        self.end.advanced_by(self.start)
    }

    /// Moves the whole cue later by `offset`, keeping its length.
    #[must_use]
    pub const fn delayed_by(self, offset: Millis) -> Self {
        Self {
            start: self.start.delayed_by(offset),
            end: self.end.delayed_by(offset),
        }
    }

    /// Moves the whole cue earlier by `offset`, keeping its length.
    #[must_use]
    pub const fn advanced_by(self, offset: Millis) -> Self {
        Self {
            start: self.start.advanced_by(offset),
            end: self.end.advanced_by(offset),
        }
    }
}

/// One subtitle cue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtitleCue {
    /// The cue's identifier: an SRT index, a `WebVTT` cue id, or `None` when the source had
    /// none, which an ASS dialogue line never does.
    pub identifier: Option<String>,
    /// When the cue is shown.
    pub timing: SubtitleTiming,
    /// `WebVTT` cue settings — `align:center position:50%` — kept as the source wrote them,
    /// so a render does not have to know which settings this build understands.
    pub settings: Option<String>,
    /// The ASS dialogue fields that carry meaning across a round trip.
    pub ass_fields: Option<AssFields>,
    /// The cue's text, one element per line.
    ///
    /// A line never carries its terminating newline; the writers supply those, which is what
    /// keeps a cue's text out of the business of knowing its format.
    pub text: Vec<String>,
}

impl SubtitleCue {
    /// Builds a cue with no identifier, settings, or ASS fields.
    #[must_use]
    pub fn new(timing: SubtitleTiming, text: Vec<String>) -> Self {
        Self {
            identifier: None,
            timing,
            settings: None,
            ass_fields: None,
            text,
        }
    }

    /// Builds a cue whose text is one line.
    #[must_use]
    pub fn line(timing: SubtitleTiming, text: impl Into<String>) -> Self {
        Self::new(timing, vec![text.into()])
    }

    /// The cue's text as one string, lines joined by newlines.
    #[must_use]
    pub fn text(&self) -> String {
        self.text.join("\n")
    }
}

/// The ASS dialogue fields a round trip must keep besides the timing and the text.
///
/// The styling *inside* the text is not here. It is in [`SubtitleCue::text`], where it
/// travels through untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssFields {
    /// `Layer`, the compositing order. `None` when the format carried no such field, which
    /// is every SSA v4 file.
    pub layer: Option<i64>,
    /// `Style`, the name of a style in the script's styles section.
    pub style: Option<String>,
    /// `Name`, the speaker. A comment rather than a directive — nothing renders it — but a
    /// round trip that dropped it would lose what a translator wrote.
    pub name: Option<String>,
    /// `MarginL`, `MarginR`, and `MarginV`, kept as the source's digits: `0040` is not the
    /// same field text as `40`, and a tool that compares them is comparing text.
    pub margins: [Option<String>; 3],
    /// `Effect`, the karaoke or scroll directive.
    pub effect: Option<String>,
    /// How many digits the source wrote after the decimal point of a timestamp.
    pub millis_digits: u8,
}

/// A parsed subtitle document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtitleDocument {
    /// The text before the first cue, one entry per line.
    pub preamble: Vec<String>,
    /// The cues, in the order the source had them.
    pub cues: Vec<SubtitleCue>,
    /// The text after the last cue, one entry per line.
    pub suffix: Vec<String>,
}

impl SubtitleDocument {
    /// Builds a document from cues, with no header or footer text.
    #[must_use]
    pub const fn new(cues: Vec<SubtitleCue>) -> Self {
        Self {
            preamble: Vec::new(),
            cues,
            suffix: Vec::new(),
        }
    }

    /// Parses a document a caller has already decoded to UTF-8.
    ///
    /// Parsing is forgiving, block by block: a block this build cannot read is kept verbatim
    /// before the first cue or after it, so a file with one unreadable line still yields
    /// every cue it does have. Malformed input therefore never loses a well-formed cue, and
    /// never panics — the hostile-input property test asserts both.
    ///
    /// # Errors
    ///
    /// [`FormatError::EmptyInput`] for input with no lines at all, [`FormatError::NotText`]
    /// for a bitmap format, and [`FormatError::NotWebVtt`] when a `WebVTT` file does not begin
    /// with its required header. Everything else is absorbed into the document.
    pub fn parse(format: SubtitleFormat, input: &str) -> Result<Self, FormatError> {
        if input.is_empty() {
            return Err(FormatError::EmptyInput);
        }
        // Normalising the terminator keeps a Windows-authored file and a Unix one the same
        // document. Neither terminator can occur inside a cue's text, so nothing is lost.
        let normalized = input.replace("\r\n", "\n").replace('\r', "\n");
        match format {
            SubtitleFormat::Vtt => parse_vtt(&normalized),
            SubtitleFormat::Srt | SubtitleFormat::Sub => parse_srt(&normalized),
            SubtitleFormat::Ass | SubtitleFormat::Ssa => Ok(parse_ass(&normalized)),
            SubtitleFormat::Pgs | SubtitleFormat::Vobsub => Err(FormatError::NotText { format }),
        }
    }

    /// Renders the document in a text format.
    ///
    /// ASS writes only from ASS and SSA cues, because an ASS dialogue line has fields — the
    /// style, the margins — that SRT and `WebVTT` do not carry, and converting *into* ASS
    /// would have to invent them. Converting *out* of ASS is lossless for the text, which is
    /// where the styling lives, and that is the direction a user actually needs: an ASS
    /// track's cues in an SRT player still carry their `{\i1}` overrides as text, which is
    /// why the ASS-to-SRT round trip in `docs/FLOWS.md` F9 asserts italics survive it.
    ///
    /// # Errors
    ///
    /// [`FormatError::NotText`] for a bitmap format, and [`FormatError::CueNotAss`] when a
    /// cue with no ASS fields is rendered as ASS.
    pub fn render(&self, format: SubtitleFormat) -> Result<String, FormatError> {
        match format {
            SubtitleFormat::Vtt => Ok(render_vtt(self)),
            SubtitleFormat::Srt | SubtitleFormat::Sub => Ok(render_srt(self)),
            SubtitleFormat::Ass | SubtitleFormat::Ssa => render_ass(format, self),
            SubtitleFormat::Pgs | SubtitleFormat::Vobsub => Err(FormatError::NotText { format }),
        }
    }
}

/// Why a subtitle format could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FormatError {
    /// The input has no lines at all.
    #[error("the subtitle input is empty")]
    EmptyInput,
    /// The format is a bitmap and has no text to parse or write.
    #[error("{format:?} is a bitmap subtitle format and carries no text")]
    NotText {
        /// The refused format.
        format: SubtitleFormat,
    },
    /// A `WebVTT` file did not begin with the header the specification requires.
    #[error("a WebVTT file must begin with a WEBVTT line")]
    NotWebVtt,
    /// A cue with no ASS dialogue fields was rendered as ASS.
    #[error("cue {index} has no ASS fields, so it cannot be written as an ASS dialogue line")]
    CueNotAss {
        /// The cue's position in the document.
        index: usize,
    },
    /// Text that was not a cue sat between two cues.
    ///
    /// The document model has a preamble and a suffix, so it cannot hold text in the middle
    /// of a cue list without moving it; refusing keeps a rewrite from reordering a file.
    #[error(
        "the subtitle text between two cues cannot be represented, so the document is refused rather than reordered"
    )]
    InterleavedText,
}

/// The line that must open a `WebVTT` file.
const WEBVTT_HEADER: &str = "WEBVTT";

/// Renders one timestamp as `HH:MM:SS,mmm` (SRT) or `HH:MM:SS.mmm` (`WebVTT`).
///
/// The two formats differ by exactly that separator, and by nothing else about the number:
/// both require the hours field, at least two digits of it, and exactly three decimal
/// digits. The comma is not a locale artifact — it is what `SubRip` parsers look for — so the
/// separator is a parameter the writer always passes explicitly rather than an environment
/// this function reads.
/// Renders one timestamp as `HH:MM:SS,mmm` (SRT) or `HH:MM:SS.mmm` (`WebVTT`).
///
/// The two formats differ by exactly that separator, and by nothing else about the number:
/// both require the hours field, at least two digits of it, and exactly three decimal
/// digits. The comma is not a locale artifact — it is what `SubRip` parsers look for — so the
/// separator is a parameter the writer always passes explicitly rather than an environment
/// this function reads.
fn timestamp(millis: Millis, separator: char) -> String {
    let total = millis.as_millis();
    let sign = if total < 0 { "-" } else { "" };
    // `unsigned_abs` rather than `abs`, because `i64::MIN.abs()` panics and a value read
    // from an untrusted file can be exactly that.
    let magnitude = total.unsigned_abs();
    let hours = magnitude / 3_600_000;
    let minutes = magnitude % 3_600_000 / 60_000;
    let seconds = magnitude % 60_000 / 1_000;
    let thousandths = magnitude % 1_000;
    format!("{sign}{hours:02}:{minutes:02}:{seconds:02}{separator}{thousandths:03}")
}

/// Parses a timestamp written as `[-]HH:MM:SS{separator}mmm`.
///
/// The hours field is capped at three digits, which is where [`super::timing::MAX_MILLIS`]
/// stops; a longer one is refused rather than wrapped.
fn parse_timestamp(token: &str, separator: char) -> Option<Millis> {
    let (negative, rest) = match token.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, token),
    };
    let (clock, fraction) = rest.split_once(separator)?;
    if rest.matches(separator).count() != 1 {
        return None;
    }
    let mut parts = clock.split(':');
    let hours: i64 = parts.next()?.parse().ok()?;
    let minutes: i64 = parts.next()?.parse().ok()?;
    let seconds: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || hours > 999 || minutes > 59 || seconds > 59 {
        return None;
    }
    if fraction.len() != 3 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let thousandths: i64 = fraction.parse().ok()?;
    let total = hours * 3_600_000 + minutes * 60_000 + seconds * 1_000 + thousandths;
    Some(Millis::from_millis(if negative { -total } else { total }))
}

/// A block of non-empty lines, and the blank lines that separated it from the one before.
#[derive(Debug, Clone, Copy)]
struct Block<'a> {
    /// How many blank lines preceded this block.
    blanks_before: usize,
    /// The block's own lines, none of which is empty.
    lines: &'a [&'a str],
}

/// Splits lines into blocks separated by runs of blank lines, keeping the run lengths.
fn blocks<'a>(lines: &'a [&'a str]) -> Vec<Block<'a>> {
    let mut blocks = Vec::new();
    let mut start: Option<usize> = None;
    let mut blanks = 0;
    for (index, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            if let Some(from) = start.take() {
                blocks.push(Block {
                    blanks_before: blanks,
                    lines: &lines[from..index],
                });
                blanks = 0;
            }
            blanks += 1;
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(from) = start {
        blocks.push(Block {
            blanks_before: blanks,
            lines: &lines[from..],
        });
    }
    blocks
}

/// Drops the empty line a trailing terminator leaves, so a document that ends with a newline
/// does not gain a blank line every time it is written and read.
fn trim_trailing_blank(lines: &mut Vec<String>) {
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
}

/// Leaves `lines` ending with exactly one blank line, or with none when it is empty.
///
/// This is the document model's one spacing rule. A block's last line is always terminated,
/// so a blank line between two blocks is an *empty line entry*, and normalising it here is
/// what makes a writer's output equal to the document it wrote instead of gaining a blank
/// line on every pass.
fn end_with_one_blank(lines: &mut Vec<String>) {
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    if !lines.is_empty() {
        lines.push(String::new());
    }
}

/// Normalises the text that follows the last cue.
///
/// The blank line between the last cue and that text is structural — the writer supplies it —
/// so the suffix is stored without it, and a file that ended with a blank line does not gain
/// another one on the way out.
fn normalise_suffix(suffix: &mut Vec<String>) {
    let leading = suffix.iter().take_while(|line| line.is_empty()).count();
    suffix.drain(..leading);
    end_with_one_blank(suffix);
}

/// The lines a writer puts between the last cue and the suffix: nothing when there is no
/// suffix, and one blank line when there is.
fn suffix_spacer(cues: usize, suffix: &[String]) -> usize {
    usize::from(cues > 0 && !suffix.is_empty())
}

/// Drops the blank line a document with no cues would otherwise gain at its end.
///
/// Such a document is all preamble, and the separator normalisation gave its text the blank
/// line that would have separated it from cues it does not have; with none, the file ends at
/// its last line.
fn collapse_trailing_blanks(cues: usize, preamble: &mut Vec<String>) {
    if cues == 0 {
        trim_trailing_blank(preamble);
    }
}

/// The blank lines a separator between two blocks needs: the source's own, or one.
fn separator_blanks(blanks_before: usize) -> usize {
    blanks_before.max(1)
}

/// Whether a line looks like `00:00:01,000 --> 00:00:04,000`.
fn is_timing_line(line: &str) -> bool {
    line.contains("-->")
}

/// Which end of a `SubRip` or `WebVTT` document an unreadable block belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextEnd {
    /// It comes before the first cue.
    Preamble,
    /// It comes after the last cue.
    Suffix,
}

/// Files a block this build could not read as a cue at the end the parser put it at.
fn keep_unreadable(document: &mut SubtitleDocument, end: TextEnd, block: Block<'_>) {
    let lines = block.lines.iter().map(|line| (*line).to_owned());
    match end {
        TextEnd::Preamble => {
            // The first block of a document has nothing before it; a later one is separated
            // from the text already there.
            let blanks = if document.preamble.is_empty() {
                0
            } else {
                separator_blanks(block.blanks_before)
            };
            for _ in 0..blanks {
                document.preamble.push(String::new());
            }
            document.preamble.extend(lines);
        }
        TextEnd::Suffix => {
            // The blank line that separates the suffix from the last cue is the writer's, so
            // only the blank lines *inside* the suffix are stored here.
            let blanks =
                usize::from(!document.suffix.is_empty()) * separator_blanks(block.blanks_before);
            for _ in 0..blanks {
                document.suffix.push(String::new());
            }
            document.suffix.extend(lines);
        }
    }
}

/// Refuses a document whose unreadable blocks sit between cues.
///
/// The model has a preamble before the first cue and a suffix after the last one, so text
/// that comes after every cue is suffix text however many blocks it takes, and text before
/// the first cue is preamble. Text with a cue after it can only be preamble or interleaved,
/// and once a cue has been seen the only place it could go is the end — which is what this
/// refuses rather than silently reordering a file.
fn check_text_is_at_one_end(
    blocks: &[Block<'_>],
    separator: char,
) -> Result<Vec<TextEnd>, FormatError> {
    let is_cue: Vec<bool> = blocks
        .iter()
        .map(|block| block_timing(block, separator).is_some())
        .collect();
    let Some(first) = is_cue.iter().position(|is_cue| *is_cue) else {
        // A document with no cues at all is all preamble, wherever its text is.
        return Ok(vec![TextEnd::Preamble; blocks.len()]);
    };
    let mut ends = Vec::with_capacity(blocks.len());
    for (index, cue_here) in is_cue.iter().enumerate() {
        if *cue_here || index < first {
            ends.push(if *cue_here {
                TextEnd::Suffix
            } else {
                TextEnd::Preamble
            });
        } else if is_cue[index + 1..].iter().any(|is_cue| *is_cue) {
            // A cue still follows this text, so putting it in the suffix would move it past
            // that cue.
            return Err(FormatError::InterleavedText);
        } else {
            // Nothing follows this text, so the suffix is where it stays.
            ends.push(TextEnd::Suffix);
        }
    }
    Ok(ends)
}

/// Reads a cue out of one block, or `None` when the block is not a cue.
fn block_timing(block: &Block<'_>, separator: char) -> Option<(usize, SubtitleTiming)> {
    let index = block.lines.iter().position(|line| is_timing_line(line))?;
    let timing = match separator {
        '.' => parse_vtt_timing_line(block.lines[index]).map(|(timing, _)| timing),
        _ => parse_timing_line(block.lines[index]),
    }?;
    Some((index, timing))
}

/// The identifier a cue block's leading lines carry, if any.
fn leading_identifier(leading: &[&str]) -> Option<String> {
    leading
        .last()
        .filter(|line| !line.trim().is_empty())
        .map(|line| (*line).to_owned())
}

/// Parses one `SubRip` block, or `None` when it is not a cue.
fn parse_srt_block(block: &Block<'_>) -> Option<SubtitleCue> {
    let (index, timing) = block_timing(block, ',')?;
    Some(SubtitleCue {
        identifier: leading_identifier(&block.lines[..index]),
        timing,
        settings: None,
        ass_fields: None,
        text: block.lines[index + 1..]
            .iter()
            .map(|line| (*line).to_owned())
            .collect(),
    })
}

/// Parses a `SubRip` document.
///
/// A block is the lines up to the next blank line. Inside one, an optional index line comes
/// first and the `-->` line after it; nothing else is required, because a file with a
/// missing index is still a file with cues in it.
fn parse_srt(input: &str) -> Result<SubtitleDocument, FormatError> {
    let lines: Vec<&str> = input.split('\n').collect();
    let blocks = blocks(&lines);
    let ends = check_text_is_at_one_end(&blocks, ',')?;
    let mut document = SubtitleDocument::new(Vec::new());
    for (block, end) in blocks.iter().zip(ends) {
        match parse_srt_block(block) {
            Some(cue) => document.cues.push(cue),
            None => keep_unreadable(&mut document, end, *block),
        }
    }
    end_with_one_blank(&mut document.preamble);
    normalise_suffix(&mut document.suffix);
    collapse_trailing_blanks(document.cues.len(), &mut document.preamble);
    Ok(document)
}

/// Parses one `WebVTT` block, or `None` when it is not a cue.
fn parse_vtt_block(block: &Block<'_>) -> Option<SubtitleCue> {
    let index = block.lines.iter().position(|line| is_timing_line(line))?;
    let (timing, settings) = parse_vtt_timing_line(block.lines[index])?;
    Some(SubtitleCue {
        identifier: leading_identifier(&block.lines[..index]),
        timing,
        settings,
        ass_fields: None,
        text: block.lines[index + 1..]
            .iter()
            .map(|line| (*line).to_owned())
            .collect(),
    })
}

/// Parses a `WebVTT` document.
///
/// `NOTE` and `STYLE` blocks are not cues and not modelled: they are kept verbatim, before
/// the first cue or after the last one, so a document with them round-trips without the model
/// having to know what either means.
fn parse_vtt(input: &str) -> Result<SubtitleDocument, FormatError> {
    let lines: Vec<&str> = input.split('\n').collect();
    let header = lines
        .iter()
        .position(|line| !line.trim().is_empty())
        .ok_or(FormatError::EmptyInput)?;
    let first = lines[header].trim_end();
    if first != WEBVTT_HEADER && !first.starts_with(&format!("{WEBVTT_HEADER} ")) {
        return Err(FormatError::NotWebVtt);
    }
    // The header line is preamble, description and all: `WEBVTT - a comment` is a legal
    // header, and dropping the comment would be a round trip that lost text. The blank lines
    // after it are not copied, because the first block records them as `blanks_before` and the
    // normalisation at the end gives the header its one separator.
    let mut document = SubtitleDocument::new(Vec::new());
    document.preamble.push(lines[header].to_owned());
    let blocks = blocks(&lines[header + 1..]);
    let ends = check_text_is_at_one_end(&blocks, '.')?;
    for (block, end) in blocks.iter().zip(ends) {
        match parse_vtt_block(block) {
            Some(cue) => document.cues.push(cue),
            None => keep_unreadable(&mut document, end, *block),
        }
    }
    end_with_one_blank(&mut document.preamble);
    normalise_suffix(&mut document.suffix);
    collapse_trailing_blanks(document.cues.len(), &mut document.preamble);
    Ok(document)
}

/// Parses `00:00:01.000 --> 00:00:04.000 align:center` into a timing and its settings.
fn parse_vtt_timing_line(line: &str) -> Option<(SubtitleTiming, Option<String>)> {
    let (start, rest) = line.split_once("-->")?;
    let mut tokens = rest.trim().splitn(2, char::is_whitespace);
    let end = tokens.next()?.trim();
    let settings = tokens
        .next()
        .map(str::trim)
        .filter(|settings| !settings.is_empty())
        .map(str::to_owned);
    Some((
        SubtitleTiming::new(
            parse_timestamp(start.trim(), '.')?,
            parse_timestamp(end, '.')?,
        ),
        settings,
    ))
}

/// Parses a `SubRip` `00:00:01,000 --> 00:00:04,000` line.
fn parse_timing_line(line: &str) -> Option<SubtitleTiming> {
    let (start, end) = line.split_once("-->")?;
    Some(SubtitleTiming::new(
        parse_timestamp(start.trim(), ',')?,
        parse_timestamp(end.split_whitespace().next()?, ',')?,
    ))
}

/// Parses an ASS or SSA document.
///
/// Every line before the first dialogue line is header text and every line after the last one
/// is footer text. A dialogue line that cannot be split against the `Format:` line its
/// `[Events]` section declared goes into that text verbatim rather than being dropped.
fn parse_ass(input: &str) -> SubtitleDocument {
    let mut document = SubtitleDocument::new(Vec::new());
    let mut pending: Vec<String> = Vec::new();
    let mut in_events = false;
    let mut field_order: Option<Vec<String>> = None;
    for line in input.split('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_events = trimmed.eq_ignore_ascii_case("[Events]");
            pending.push(line.to_owned());
        } else if in_events && let Some(rest) = event_value(trimmed, "Format") {
            let fields: Vec<String> = rest
                .split(',')
                .map(|field| field.trim().to_ascii_lowercase())
                .collect();
            let usable = fields
                .iter()
                .position(|field| field == "text")
                .is_some_and(|index| index + 1 == fields.len());
            field_order = usable.then_some(fields);
            pending.push(line.to_owned());
        } else if in_events && let Some(rest) = event_value(trimmed, "Dialogue") {
            match field_order
                .as_deref()
                .and_then(|order| parse_dialogue(rest, order))
            {
                Some(cue) => {
                    if document.cues.is_empty() {
                        document.preamble.append(&mut pending);
                    }
                    document.cues.push(cue);
                }
                // The dialogue line stays in the text, where it is written back unchanged.
                None => pending.push(line.to_owned()),
            }
        } else {
            pending.push(line.to_owned());
        }
    }
    // `split('\n')` turns a trailing newline into one empty element, which is the file's
    // terminator rather than a blank line of the document. A trailing blank line is the one
    // the writer puts after the last line, so exactly one is kept: the shape of a rendered
    // document does not depend on whether the source ended with a newline.
    trim_trailing_blank(&mut pending);
    end_with_one_blank(&mut pending);
    if document.cues.is_empty() {
        document.preamble.append(&mut pending);
    } else {
        document.suffix.append(&mut pending);
        normalise_suffix(&mut document.suffix);
        trim_trailing_blank(&mut document.preamble);
    }
    document
}

/// Strips `Key:` from an event line and returns the rest, when the key matches.
fn event_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let (found, rest) = line.split_once(':')?;
    found.trim().eq_ignore_ascii_case(key).then(|| rest.trim())
}

/// Parses one ASS dialogue line against the field order its `Format:` line declared.
fn parse_dialogue(rest: &str, order: &[String]) -> Option<SubtitleCue> {
    if order.last().map(String::as_str) != Some("text") {
        // `Text` is last in every ASS and SSA file the specifications allow, and a line whose
        // Text is not last cannot be split without a full CSV reader for a field order that
        // does not exist. Refusing the line keeps it verbatim instead of guessing where the
        // commas belong.
        return None;
    }
    // `Text` may itself contain commas, so the line is split into one field per `Format`
    // column and only the first `order.len() - 1` commas are separators; the last column's
    // value is what remains. A line with fewer commas than its format needs has a field that
    // swallowed its successor, and is refused rather than read with the wrong columns.
    if rest.matches(',').count() < order.len() - 1 {
        return None;
    }
    let fields: Vec<&str> = rest.splitn(order.len(), ',').collect();
    let mut layer = None;
    let mut style = None;
    let mut name = None;
    let mut margins = [None, None, None];
    let mut effect = None;
    let mut start = None;
    let mut end = None;
    let mut text = String::new();
    let mut precision = ASS_DEFAULT_FRACTION_DIGITS;
    for (field, value) in order.iter().zip(fields) {
        match field.as_str() {
            "layer" | "marked" => layer = value.trim().parse().ok(),
            "start" => {
                let (parsed, digits) = parse_ass_timestamp(value.trim())?;
                start = Some(parsed);
                precision = precision.max(digits);
            }
            "end" => {
                let (parsed, digits) = parse_ass_timestamp(value.trim())?;
                end = Some(parsed);
                // Both timestamps are written with the same number of digits, so the cue
                // keeps the wider of the two.
                precision = precision.max(digits);
            }
            "style" => style = non_empty(value),
            "name" => name = non_empty(value),
            "marginl" => margins[0] = non_empty(value),
            "marginr" => margins[1] = non_empty(value),
            "marginv" => margins[2] = non_empty(value),
            "effect" => effect = non_empty(value),
            "text" => value.clone_into(&mut text),
            // The remaining SSA codes — `Actor`, `Picture`, `Highlight` — carry nothing this
            // model writes back.
            _ => {}
        }
    }
    let timing = SubtitleTiming::new(start?, end?);
    // ASS text is one line whose breaks are `\N`; splitting it here and joining it on render
    // is what keeps `\N` out of the model.
    let text = text.split("\\N").map(str::to_owned).collect();
    Some(SubtitleCue {
        identifier: None,
        timing,
        settings: None,
        ass_fields: Some(AssFields {
            layer,
            style,
            name,
            margins,
            effect,
            millis_digits: precision,
        }),
        text,
    })
}

/// Parses an ASS timestamp, returning it and the number of fraction digits it carried.
fn parse_ass_timestamp(token: &str) -> Option<(Millis, u8)> {
    let (negative, rest) = match token.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, token),
    };
    let (clock, fraction) = rest.split_once('.')?;
    let mut parts = clock.split(':');
    let hours: u64 = parts.next()?.parse().ok()?;
    let minutes: u64 = parts.next()?.parse().ok()?;
    let seconds: u64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || minutes > 59 || seconds > 59 {
        return None;
    }
    let digits = u8::try_from(fraction.len()).ok()?;
    if digits == 0 || digits > ASS_MAX_FRACTION_DIGITS {
        return None;
    }
    if !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let fraction_value: u64 = fraction.parse().ok()?;
    // One digit is tenths and two are centiseconds; scaling to milliseconds loses nothing the
    // source's own digits had not already lost.
    let fraction_millis = match digits {
        1 => fraction_value * 100,
        2 => fraction_value * 10,
        _ => fraction_value,
    };
    let total = hours
        .checked_mul(3_600_000)?
        .checked_add(minutes * 60_000)?
        .checked_add(seconds * 1_000)?
        .checked_add(fraction_millis)?;
    let total = i64::try_from(total).ok()?;
    Some((
        Millis::from_millis(if negative { -total } else { total }),
        digits,
    ))
}

/// Returns the field text when it is not empty, so an absent `Style` is `None` rather than an
/// empty string the writer would emit as a real field.
fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// The text a document keeps when it is written in another format.
///
/// A preamble is format-specific: an ASS `[Script Info]` block means nothing in a `SubRip` file,
/// and a `WebVTT` `NOTE` means nothing in a dialogue line's header area. The rule is therefore
/// that a preamble or suffix travels only to a format that can read it — ASS text to ASS and
/// SSA, `WebVTT` text to `WebVTT`, and a `SubRip` document's leader to both, because `SubRip` has no
/// header of its own and a leading comment is a cue-less line either way.
///
/// This is a deliberate loss, and it is the only one the writers make: converting *out* of a
/// format drops the format's own furniture and keeps the cues, which is what a user converting
/// a track actually wants.
fn carried_text(lines: &[String], format: SubtitleFormat) -> &[String] {
    let is_ass = matches!(format, SubtitleFormat::Ass | SubtitleFormat::Ssa);
    if is_ass {
        return lines;
    }
    if lines.iter().any(|line| line.trim_start().starts_with('[')) {
        return &[];
    }
    lines
}

/// Writes the SRT text for a document.
///
/// A cue is written as its index, its timing line, its text, and a blank line; the blank line
/// after the last one is the separator before the suffix, which the suffix may not have had,
/// so the writer supplies it. Everything else comes from the document.
fn render_srt(document: &SubtitleDocument) -> String {
    let mut out = String::new();
    write_text(
        &mut out,
        carried_text(&document.preamble, SubtitleFormat::Srt),
    );
    for (index, cue) in document.cues.iter().enumerate() {
        // The index is always written, in order, even when the source's own numbering was
        // missing or went another way: a SubRip index is a position, not content, and a file
        // whose numbers skip is a file most muxers reject.
        let _ = writeln!(out, "{}", index + 1);
        let _ = writeln!(
            out,
            "{} --> {}",
            timestamp(cue.timing.start(), ','),
            timestamp(cue.timing.end(), ',')
        );
        write_text(&mut out, &cue.text);
        out.push('\n');
    }
    let suffix = carried_text(&document.suffix, SubtitleFormat::Srt);
    // A blank line separates two blocks in SubRip: the one after the last cue is written here
    // and nowhere else, so a document with no suffix does not end in two blank lines.
    append_spacers(&mut out, suffix_spacer(document.cues.len(), suffix));
    write_text(&mut out, suffix);
    out
}

/// Writes the `WebVTT` text for a document.
///
/// The header is not treated specially: a parsed document's preamble already contains `WEBVTT`
/// where the source had it, and a document built in memory may have a preamble that is only
/// the header. The one case the writer has to supply it for is a document converted from a
/// format that has no header, because a `WebVTT` file without one is invalid.
fn render_vtt(document: &SubtitleDocument) -> String {
    let preamble = carried_text(&document.preamble, SubtitleFormat::Vtt);
    let mut out = String::new();
    if preamble.is_empty() {
        // A document converted from SRT has no header to carry over, and a WebVTT file
        // without one is invalid, so the writer supplies it and its separator.
        out.push_str(WEBVTT_HEADER);
        out.push('\n');
        append_spacers(&mut out, 1);
    } else {
        write_text(&mut out, preamble);
    }
    for cue in &document.cues {
        if let Some(identifier) = &cue.identifier {
            out.push_str(identifier);
            out.push('\n');
        }
        let _ = write!(
            out,
            "{} --> {}",
            timestamp(cue.timing.start(), '.'),
            timestamp(cue.timing.end(), '.')
        );
        if let Some(settings) = &cue.settings {
            let _ = write!(out, " {settings}");
        }
        out.push('\n');
        write_text(&mut out, &cue.text);
        out.push('\n');
    }
    let suffix = carried_text(&document.suffix, SubtitleFormat::Vtt);
    append_spacers(&mut out, suffix_spacer(document.cues.len(), suffix));
    write_text(&mut out, suffix);
    out
}

/// Writes the ASS text for a document.
///
/// ASS has no blank line between its header and the events, so the preamble is written with its
/// trailing blank removed, which is the one place the document's spacing rule does not apply.
fn render_ass(format: SubtitleFormat, document: &SubtitleDocument) -> Result<String, FormatError> {
    let mut preamble = carried_text(&document.preamble, format).to_vec();
    trim_trailing_blank(&mut preamble);
    let mut out = String::new();
    write_text(&mut out, &preamble);
    for (index, cue) in document.cues.iter().enumerate() {
        let Some(fields) = &cue.ass_fields else {
            return Err(FormatError::CueNotAss { index });
        };
        let digits = fraction_digits(fields.millis_digits, cue.timing);
        // The ten dialogue columns are exactly the `Format:` line the format defines for them,
        // collected in one place here so a compile error rather than a shifted field says when
        // one is missed.
        let columns = [
            layer_field(format, fields),
            ass_timestamp(cue.timing.start(), digits),
            ass_timestamp(cue.timing.end(), digits),
            fields.style.clone().unwrap_or_default(),
            fields.name.clone().unwrap_or_default(),
            margin_field(fields.margins[0].as_deref()),
            margin_field(fields.margins[1].as_deref()),
            margin_field(fields.margins[2].as_deref()),
            fields.effect.clone().unwrap_or_default(),
            cue.text.join("\\N"),
        ];
        let _ = writeln!(out, "Dialogue: {}", columns.join(","));
    }
    // The blank line between the last dialogue line and the suffix is written here, as it is
    // in SubRip and WebVTT, so the suffix itself carries only the blanks inside it.
    append_spacers(
        &mut out,
        suffix_spacer(document.cues.len(), &document.suffix),
    );
    write_text(&mut out, &document.suffix);
    Ok(out)
}

/// The first dialogue column: ASS's `Layer` number, or SSA's `Marked=0` flag, which is the
/// same field under the older format's spelling. Writing a bare number into an SSA file
/// produces a line its own parser reads back as a missing field.
fn layer_field(format: SubtitleFormat, fields: &AssFields) -> String {
    let value = fields
        .layer
        .map_or_else(|| "0".to_owned(), |layer| layer.to_string());
    match format {
        SubtitleFormat::Ssa => format!("Marked={value}"),
        _ => value,
    }
}

/// Writes lines, each terminated. An empty line in the input is an empty line in the output,
/// which is what preserves the blank lines a document's own text contains.
fn write_text(out: &mut String, lines: &[String]) {
    for line in lines {
        out.push_str(line);
        out.push('\n');
    }
}

/// Appends `count` blank lines, which is how every one of these formats separates one block
/// from the next.
fn append_spacers(out: &mut String, count: usize) {
    for _ in 0..count {
        out.push('\n');
    }
}

/// The fraction digits a cue is written with.
///
/// The source's own digits are used when they can express the cue exactly; otherwise three
/// are, which is the resolution the model holds. Writing two digits for a cue at 1 501 ms
/// would round it by a millisecond on the way out, which is precisely the drift the timing
/// module exists to avoid.
fn fraction_digits(preferred: u8, timing: SubtitleTiming) -> u8 {
    let digits = preferred.clamp(1, ASS_MAX_FRACTION_DIGITS);
    let step = 10_i64.pow(u32::from(ASS_MAX_FRACTION_DIGITS - digits));
    if timing.start().as_millis() % step == 0 && timing.end().as_millis() % step == 0 {
        digits
    } else {
        ASS_MAX_FRACTION_DIGITS
    }
}

/// Renders an ASS timestamp, which writes the hours without a fixed width and the fraction
/// with `digits` digits.
fn ass_timestamp(millis: Millis, digits: u8) -> String {
    let total = millis.as_millis();
    let sign = if total < 0 { "-" } else { "" };
    let magnitude = total.unsigned_abs();
    let hours = magnitude / 3_600_000;
    let minutes = magnitude % 3_600_000 / 60_000;
    let seconds = magnitude % 60_000 / 1_000;
    let fraction = magnitude % 1_000;
    match digits {
        1 => format!("{sign}{hours}:{minutes:02}:{seconds:02}.{}", fraction / 100),
        2 => format!(
            "{sign}{hours}:{minutes:02}:{seconds:02}.{:02}",
            fraction / 10
        ),
        _ => format!("{sign}{hours}:{minutes:02}:{seconds:02}.{fraction:03}"),
    }
}

/// One margin field, defaulting to the `0000` the format uses for "unset".
fn margin_field(margin: Option<&str>) -> String {
    margin.unwrap_or("0000").to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn ms(value: i64) -> Millis {
        Millis::from_millis(value)
    }

    fn timing(start: i64, end: i64) -> SubtitleTiming {
        SubtitleTiming::new(ms(start), ms(end))
    }

    /// The fixture the ASS acceptance test rests on: a header, a styles section, and two
    /// dialogue lines whose text carries an italic override, a position, and an alignment.
    const STYLED_ASS: &str = "\
[Script Info]
; Script generated by Aegisub
Title: Blade Runner 2049
ScriptType: v4.00+
PlayResX: 1920
PlayResY: 1080

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Default,DejaVu Sans,48,&H00FFFFFF,&H000000FF,&H00000000,&H64000000,0,0,0,0,100,100,0,0,1,2,1,2,10,10,20,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:01.00,0:00:03.50,Default,Bob,0040,0040,0020,,{\\i1}It was a mistake.
Dialogue: 1,0:00:04.00,0:00:06.25,Default,,0000,0000,0000,fx,{\\an8}{\\pos(960,80)}The city, sprawled out below.
";

    /// The fixture the ASS acceptance test rests on, as the writer produces it: one
    /// terminator per line and no line after the last one.
    fn styled_source() -> String {
        STYLED_ASS.to_owned()
    }

    fn styled_fixture() -> SubtitleDocument {
        SubtitleDocument::parse(SubtitleFormat::Ass, &styled_source()).expect("the fixture parses")
    }

    #[test]
    fn the_ass_acceptance_fixture_round_trips_byte_for_byte() {
        let document = styled_fixture();
        assert_eq!(document.cues.len(), 2);
        assert_eq!(
            document.render(SubtitleFormat::Ass).expect("renders"),
            styled_source(),
            "a styling-bearing ASS round trip preserves italics and positioning"
        );
    }

    #[test]
    fn the_ass_round_trip_keeps_the_inline_overrides_as_text() {
        let document = styled_fixture();
        assert!(
            document.cues[0].text[0].starts_with("{\\i1}"),
            "the override is cue text, not a parsed style: {}",
            document.cues[0].text[0]
        );
        assert!(document.cues[1].text[0].contains("\\pos(960,80)"));
        assert!(document.cues[1].text[0].contains("\\an8"));

        let fields = document.cues[1].ass_fields.as_ref().expect("ASS fields");
        assert_eq!(fields.layer, Some(1));
        assert_eq!(fields.style.as_deref(), Some("Default"));
        assert_eq!(fields.name, None);
        assert_eq!(
            fields.margins,
            [
                Some("0000".to_owned()),
                Some("0000".to_owned()),
                Some("0000".to_owned())
            ]
        );
        assert_eq!(fields.effect.as_deref(), Some("fx"));
        assert_eq!(document.cues[0].timing, timing(1_000, 3_500));
    }

    #[test]
    fn an_ass_header_block_survives_a_round_trip() {
        let rendered = styled_fixture()
            .render(SubtitleFormat::Ass)
            .expect("renders");
        for line in [
            "[Script Info]",
            "Title: Blade Runner 2049",
            "PlayResX: 1920",
            "[V4+ Styles]",
            "Style: Default,DejaVu Sans,48,",
            "Dialogue: 0,0:00:01.00",
        ] {
            assert!(
                rendered.contains(line),
                "`{line}` was dropped by the round trip"
            );
        }
    }

    #[test]
    fn a_document_with_no_cues_keeps_its_text() {
        let header = "[Script Info]\nTitle: Nothing\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n";
        let document = SubtitleDocument::parse(SubtitleFormat::Ass, header).expect("parses");
        assert_eq!(document.cues.len(), 0, "the document has no cues");
        assert_eq!(
            document.render(SubtitleFormat::Ass).expect("renders"),
            header,
            "a header with no dialogue lines is still a file"
        );
        // A file terminator is not a line of the document: a source the writer itself
        // produced has the same shape with or without one.
        assert_eq!(
            document.render(SubtitleFormat::Ass).expect("renders"),
            SubtitleDocument::parse(SubtitleFormat::Ass, header)
                .expect("parses")
                .render(SubtitleFormat::Ass)
                .expect("renders")
        );
    }

    #[test]
    fn an_srt_document_round_trips_byte_for_byte() {
        let source = "1\n00:00:01,000 --> 00:00:04,000\nHello there.\n\n2\n00:00:05,500 --> 00:00:07,000\n{\\i1}Styled{\\i0} text.\n\n";
        let document = SubtitleDocument::parse(SubtitleFormat::Srt, source).expect("parses");
        assert_eq!(document.cues.len(), 2);
        assert_eq!(document.cues[0].identifier.as_deref(), Some("1"));
        assert_eq!(document.cues[0].timing, timing(1_000, 4_000));
        assert_eq!(
            document.render(SubtitleFormat::Srt).expect("renders"),
            source
        );
    }

    #[test]
    fn the_golden_srt_fixture_is_written_byte_for_byte() {
        let document = SubtitleDocument::new(vec![
            SubtitleCue::line(timing(1_000, 4_000), "Hello there."),
            SubtitleCue::line(timing(3_661_000, 3_723_500), "An hour and a minute in."),
        ]);
        let expected = "1\n00:00:01,000 --> 00:00:04,000\nHello there.\n\n2\n01:01:01,000 --> 01:02:03,500\nAn hour and a minute in.\n\n";
        assert_eq!(
            document.render(SubtitleFormat::Srt).expect("renders"),
            expected
        );
    }

    #[test]
    fn the_golden_vtt_fixture_is_written_byte_for_byte() {
        let mut first = SubtitleCue::line(timing(1_000, 4_000), "Hello there.");
        first.identifier = Some("intro".to_owned());
        first.settings = Some("align:center".to_owned());
        let second = SubtitleCue::line(timing(3_661_000, 3_723_500), "An hour and a minute in.");
        let mut document = SubtitleDocument::new(vec![first, second]);
        document.preamble = vec!["WEBVTT".to_owned(), String::new()];
        let expected = "WEBVTT\n\nintro\n00:00:01.000 --> 00:00:04.000 align:center\nHello there.\n\n01:01:01.000 --> 01:02:03.500\nAn hour and a minute in.\n\n";
        assert_eq!(
            document.render(SubtitleFormat::Vtt).expect("renders"),
            expected
        );
    }

    #[test]
    fn the_two_simple_formats_differ_by_separator_header_and_index() {
        let document = SubtitleDocument::new(vec![SubtitleCue::line(timing(1_000, 2_000), "Hi.")]);
        let srt = document.render(SubtitleFormat::Srt).expect("renders");
        let vtt = document.render(SubtitleFormat::Vtt).expect("renders");

        assert_eq!(srt, "1\n00:00:01,000 --> 00:00:02,000\nHi.\n\n");
        assert_eq!(vtt, "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nHi.\n\n");
        assert!(
            !vtt.contains("1\n00:00:01"),
            "WebVTT has no required cue index"
        );
    }

    #[test]
    fn an_hour_field_past_a_day_is_still_written_in_hours() {
        let document = SubtitleDocument::new(vec![SubtitleCue::line(
            timing(90_061_500, 90_062_000),
            "Past a day.",
        )]);
        let srt = document.render(SubtitleFormat::Srt).expect("renders");
        assert!(
            srt.contains("25:01:01,500 --> 25:01:02,000"),
            "an hour field is not wrapped at 24: {srt}"
        );
        assert_eq!(
            SubtitleDocument::parse(SubtitleFormat::Srt, &srt)
                .expect("parses")
                .cues[0]
                .timing,
            timing(90_061_500, 90_062_000)
        );
    }

    #[test]
    fn a_timestamp_at_the_end_of_the_range_round_trips() {
        let far = super::super::timing::MAX_MILLIS;
        let document =
            SubtitleDocument::new(vec![SubtitleCue::line(timing(far - 1, far), "The end.")]);
        let srt = document.render(SubtitleFormat::Srt).expect("renders");
        assert!(srt.contains("998:59:59,999 --> 999:00:00,000"), "{srt}");
        assert_eq!(
            SubtitleDocument::parse(SubtitleFormat::Srt, &srt)
                .expect("parses")
                .cues[0]
                .timing,
            timing(far - 1, far)
        );
    }

    #[test]
    fn an_over_long_hour_field_is_refused_as_a_cue_and_kept_as_text() {
        let source = "1\n1000:00:01,000 --> 00:00:02,000\nNot a timestamp.\n\n";
        let document = SubtitleDocument::parse(SubtitleFormat::Srt, source).expect("parses");
        assert_eq!(document.cues.len(), 0, "the document has no cues");
        assert!(
            document
                .preamble
                .iter()
                .any(|line| line == "1000:00:01,000 --> 00:00:02,000"),
            "the unreadable block is kept verbatim: {:?}",
            document.preamble
        );
    }

    #[test]
    fn zero_duration_a_backwards_cue_and_overlaps_are_values() {
        let document = SubtitleDocument::new(vec![
            SubtitleCue::line(timing(1_000, 1_000), "Zero duration."),
            SubtitleCue::line(timing(5_000, 4_000), "Ends before it starts."),
            SubtitleCue::line(timing(4_500, 6_000), "Overlaps the one before it."),
        ]);
        let srt = document.render(SubtitleFormat::Srt).expect("renders");
        let parsed = SubtitleDocument::parse(SubtitleFormat::Srt, &srt).expect("parses");
        assert_eq!(parsed.cues.len(), 3, "no cue is dropped");
        assert_eq!(parsed.cues[0].timing.duration(), Millis::ZERO);
        assert_eq!(parsed.cues[1].timing, timing(5_000, 4_000));
        assert!(
            parsed.cues[1].timing.end() < parsed.cues[1].timing.start(),
            "a cue that ends before it starts is kept as it is"
        );
        assert!(
            parsed.cues[2].timing.start() < parsed.cues[1].timing.start(),
            "an overlapping cue starts before the one before it"
        );
        assert_eq!(
            parsed
                .cues
                .iter()
                .map(SubtitleCue::text)
                .collect::<Vec<_>>(),
            document
                .cues
                .iter()
                .map(SubtitleCue::text)
                .collect::<Vec<_>>(),
            "every cue's text survived"
        );
    }

    #[test]
    fn a_cue_advanced_before_zero_keeps_its_text_and_comes_back() {
        let mut document =
            SubtitleDocument::new(vec![SubtitleCue::line(timing(500, 1_500), "Early.")]);
        for cue in &mut document.cues {
            cue.timing = cue.timing.advanced_by(ms(2_000));
        }
        assert_eq!(document.cues[0].timing, timing(-1_500, -500));

        let srt = document.render(SubtitleFormat::Srt).expect("renders");
        assert!(srt.contains("-00:00:01,500 --> -00:00:00,500"), "{srt}");
        let parsed = SubtitleDocument::parse(SubtitleFormat::Srt, &srt).expect("parses");
        assert_eq!(parsed.cues[0].timing, document.cues[0].timing);
        assert_eq!(parsed.cues[0].text(), document.cues[0].text());
    }

    #[test]
    fn ass_converts_to_the_simple_formats_with_its_styling_intact() {
        let srt = styled_fixture()
            .render(SubtitleFormat::Srt)
            .expect("renders");
        assert!(
            srt.contains("{\\i1}It was a mistake."),
            "the ASS-to-SRT direction keeps italics as text: {srt}"
        );
        assert!(srt.contains("{\\pos(960,80)}"));
        assert!(
            !srt.contains("[Script Info]"),
            "the script header is ASS-specific and has no place in a SubRip file"
        );
    }

    #[test]
    fn writing_ass_from_a_cue_without_ass_fields_is_refused() {
        let document =
            SubtitleDocument::new(vec![SubtitleCue::line(timing(0, 1_000), "No style.")]);
        assert_eq!(
            document.render(SubtitleFormat::Ass),
            Err(FormatError::CueNotAss { index: 0 })
        );
    }

    #[test]
    fn a_bitmap_format_has_no_text_to_read_or_write() {
        let document = SubtitleDocument::new(Vec::new());
        assert_eq!(
            document.render(SubtitleFormat::Pgs),
            Err(FormatError::NotText {
                format: SubtitleFormat::Pgs
            })
        );
        assert_eq!(
            SubtitleDocument::parse(SubtitleFormat::Vobsub, "anything"),
            Err(FormatError::NotText {
                format: SubtitleFormat::Vobsub
            })
        );
    }

    #[test]
    fn an_empty_input_is_refused_and_a_vtt_without_its_header_is_too() {
        assert_eq!(
            SubtitleDocument::parse(SubtitleFormat::Srt, ""),
            Err(FormatError::EmptyInput)
        );
        assert_eq!(
            SubtitleDocument::parse(SubtitleFormat::Vtt, "1\n00:00:01.000 --> 00:00:02.000\nX\n"),
            Err(FormatError::NotWebVtt)
        );
        assert!(
            SubtitleDocument::parse(
                SubtitleFormat::Vtt,
                "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nX\n"
            )
            .is_ok()
        );
    }

    #[test]
    fn a_non_ascii_and_right_to_left_cue_survives_the_round_trip() {
        let document = SubtitleDocument::new(vec![
            SubtitleCue::line(timing(0, 1_000), "Grüße, señor — 日本語のテキスト"),
            SubtitleCue::line(timing(1_000, 2_000), "مرحبا بالعالم"),
        ]);
        // A SubRip index is the cue's position, so a document built without one gains one on
        // its first write; what a locale can lose is the text, and that is what is asserted.
        for format in [SubtitleFormat::Srt, SubtitleFormat::Vtt] {
            let text = document.render(format).expect("renders");
            let parsed = SubtitleDocument::parse(format, &text).expect("parses");
            assert_eq!(parsed.cues.len(), document.cues.len());
            for (parsed, original) in parsed.cues.iter().zip(&document.cues) {
                assert_eq!(parsed.text, original.text, "{format:?} lost text");
                assert_eq!(parsed.timing, original.timing, "{format:?} lost timing");
            }
        }
    }

    #[test]
    fn a_carriage_return_file_is_the_same_document_as_an_lf_one() {
        let unix = "1\n00:00:01,000 --> 00:00:02,000\nHi.\n\n";
        let windows = "1\r\n00:00:01,000 --> 00:00:02,000\r\nHi.\r\n\r\n";
        assert_eq!(
            SubtitleDocument::parse(SubtitleFormat::Srt, unix).expect("parses"),
            SubtitleDocument::parse(SubtitleFormat::Srt, windows).expect("parses")
        );
    }

    #[test]
    fn a_malformed_block_at_either_end_is_kept_and_the_cues_are_not() {
        let source = "garbage at the top\n\n1\n00:00:01,000 --> 00:00:02,000\nGood.\n\n2\n00:00:09,000 --> 00:00:10,000\nAlso good.\n\nnot a timing\n\ntrailing garbage\n";
        let document = SubtitleDocument::parse(SubtitleFormat::Srt, source).expect("parses");
        assert_eq!(document.cues.len(), 2, "the readable cues are all there");
        assert_eq!(document.cues[0].text, vec!["Good.".to_owned()]);
        assert_eq!(document.cues[1].text, vec!["Also good.".to_owned()]);
        assert!(
            document
                .preamble
                .iter()
                .any(|line| line == "garbage at the top"),
            "an unreadable block before the first cue is preamble: {:?}",
            document.preamble
        );
        assert!(
            document
                .suffix
                .iter()
                .any(|line| line == "trailing garbage"),
            "an unreadable block after the last cue is suffix: {:?}",
            document.suffix
        );
        let rendered = document.render(SubtitleFormat::Srt).expect("renders");
        let order: Vec<usize> = [
            "garbage at the top",
            "Good.",
            "Also good.",
            "not a timing",
            "trailing garbage",
        ]
        .iter()
        .map(|needle| rendered.find(needle).expect("every line is written back"))
        .collect();
        assert!(
            order.windows(2).all(|pair| pair[0] < pair[1]),
            "text at either end is written back where it was: {rendered}"
        );
    }

    #[test]
    fn text_between_two_cues_is_refused_rather_than_reordered() {
        let source = "1\n00:00:01,000 --> 00:00:02,000\nGood.\n\nnot a cue\n\n2\n00:00:09,000 --> 00:00:10,000\nAlso good.\n\n";
        assert_eq!(
            SubtitleDocument::parse(SubtitleFormat::Srt, source),
            Err(FormatError::InterleavedText),
            "moving the text to the end would silently reorder the file"
        );
        assert_eq!(
            SubtitleDocument::parse(
                SubtitleFormat::Vtt,
                "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nGood.\n\nNOTE a note\n\n00:00:09.000 --> 00:00:10.000\nAlso good.\n\n"
            ),
            Err(FormatError::InterleavedText)
        );
    }

    #[test]
    fn a_document_that_is_only_text_has_no_cues_and_keeps_it() {
        let source = "NOTE nothing here is a cue\n\nnot a timing line either\n";
        let document = SubtitleDocument::parse(SubtitleFormat::Srt, source).expect("parses");
        assert_eq!(document.cues.len(), 0, "the document has no cues");
        assert_eq!(
            document.render(SubtitleFormat::Srt).expect("renders"),
            "NOTE nothing here is a cue\n\nnot a timing line either\n",
            "both blocks are written back, in order"
        );
    }

    #[test]
    fn a_leader_before_the_first_cue_is_preamble_not_a_cue() {
        let source = "NOTE this is not a cue\n\n1\n00:00:01,000 --> 00:00:02,000\nGood.\n\n";
        let document = SubtitleDocument::parse(SubtitleFormat::Srt, source).expect("parses");
        assert_eq!(document.cues.len(), 1);
        assert!(
            document
                .preamble
                .iter()
                .any(|line| line == "NOTE this is not a cue"),
            "a leader before the first cue is preamble: {:?}",
            document.preamble
        );
        assert_eq!(
            document.render(SubtitleFormat::Srt).expect("renders"),
            source
        );
    }

    #[test]
    fn a_webvtt_note_and_style_block_is_kept_out_of_the_cues() {
        let source = "WEBVTT - with a comment\n\nNOTE\nA note that is not a cue.\n\nSTYLE\n::cue { color: lime }\n\nintro\n00:00:01.000 --> 00:00:02.000 align:start\nHello.\n\n";
        let document = SubtitleDocument::parse(SubtitleFormat::Vtt, source).expect("parses");
        assert_eq!(document.cues.len(), 1);
        assert_eq!(document.cues[0].identifier.as_deref(), Some("intro"));
        assert_eq!(document.cues[0].settings.as_deref(), Some("align:start"));
        assert!(
            document.suffix.is_empty() && !document.preamble.is_empty(),
            "the NOTE and the STYLE are preamble, not cues"
        );
        assert_eq!(
            document.render(SubtitleFormat::Vtt).expect("renders"),
            source,
            "a NOTE and a STYLE block round-trip as text"
        );
    }

    #[test]
    fn an_srt_index_is_not_read_as_cue_text() {
        let source = "1\n00:00:01,000 --> 00:00:02,000\n1984\n\n2\n00:00:03,000 --> 00:00:04,000\nChannel 5\n\n";
        let document = SubtitleDocument::parse(SubtitleFormat::Srt, source).expect("parses");
        assert_eq!(document.cues[0].text, vec!["1984".to_owned()]);
        assert_eq!(document.cues[1].text, vec!["Channel 5".to_owned()]);
    }

    #[test]
    fn an_ass_timestamp_with_three_fraction_digits_is_read_and_written_as_three() {
        let source = "[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:01.001,0:00:03.500,Default,,0000,0000,0000,,Precise.\n";
        let document = SubtitleDocument::parse(SubtitleFormat::Ass, source).expect("parses");
        assert_eq!(document.cues[0].timing, timing(1_001, 3_500));
        assert_eq!(
            document.cues[0]
                .ass_fields
                .as_ref()
                .expect("ASS fields")
                .millis_digits,
            3
        );
        assert_eq!(
            document.render(SubtitleFormat::Ass).expect("renders"),
            source,
            "three digits are preserved rather than rewritten as centiseconds"
        );
    }

    #[test]
    fn an_ssa_marked_field_is_read_as_a_layer() {
        let source = "[Script Info]\nScriptType: v4.00\n\n[V4 Styles]\nFormat: Name, Fontname, Fontsize\nStyle: Default,Arial,20\n\n[Events]\nFormat: Marked, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: Marked=0,0:00:01.00,0:00:02.00,Default,,0000,0000,0000,,SSA line.\n";
        let document = SubtitleDocument::parse(SubtitleFormat::Ssa, source).expect("parses");
        assert_eq!(document.cues.len(), 1, "the SSA field order is read");
        assert_eq!(document.cues[0].text, vec!["SSA line.".to_owned()]);
        assert_eq!(
            document.render(SubtitleFormat::Ssa).expect("renders"),
            source
        );
    }

    #[test]
    fn ass_text_line_breaks_are_split_and_joined() {
        let source = "[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:01.00,0:00:02.00,Default,,0000,0000,0000,,Two\\Nlines.\n";
        let document = SubtitleDocument::parse(SubtitleFormat::Ass, source).expect("parses");
        assert_eq!(
            document.cues[0].text,
            vec!["Two".to_owned(), "lines.".to_owned()]
        );
        assert_eq!(
            document.render(SubtitleFormat::Ass).expect("renders"),
            source
        );
    }

    #[test]
    fn an_ass_text_field_may_contain_commas() {
        let source = "[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:01.00,0:00:02.00,Default,,0000,0000,0000,,The city, sprawled out, below.\n";
        let document = SubtitleDocument::parse(SubtitleFormat::Ass, source).expect("parses");
        assert_eq!(
            document.cues[0].text,
            vec!["The city, sprawled out, below.".to_owned()]
        );
        assert_eq!(
            document.render(SubtitleFormat::Ass).expect("renders"),
            source
        );
    }

    #[test]
    fn a_dialogue_line_missing_a_required_field_is_kept_as_text() {
        let source = "[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:01.00\n";
        let document = SubtitleDocument::parse(SubtitleFormat::Ass, source).expect("parses");
        assert_eq!(document.cues.len(), 0, "the document has no cues");
        assert_eq!(
            document.render(SubtitleFormat::Ass).expect("renders"),
            source
        );
    }

    #[test]
    fn ass_text_after_the_last_dialogue_line_round_trips() {
        let source = "[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:01.00,0:00:02.00,Default,,0000,0000,0000,,Spoken.\n\n[Fonts]\nfontname: DejaVu Sans\n\n";
        let document = SubtitleDocument::parse(SubtitleFormat::Ass, source).expect("parses");
        assert_eq!(document.cues.len(), 1);
        assert!(
            document
                .suffix
                .iter()
                .any(|line| line == "fontname: DejaVu Sans"),
            "the text after the last dialogue line is the suffix: {:?}",
            document.suffix
        );
        assert_eq!(
            document.render(SubtitleFormat::Ass).expect("renders"),
            source
        );
    }

    #[test]
    fn a_cue_with_a_timestamp_the_model_cannot_express_in_two_digits_is_written_with_three() {
        let mut cue = styled_fixture().cues[0].clone();
        cue.timing = timing(1_000, 1_501);
        let rendered = SubtitleDocument::new(vec![cue])
            .render(SubtitleFormat::Ass)
            .expect("renders");
        assert!(
            rendered.contains("0:00:01.000") && rendered.contains("0:00:01.501"),
            "a half-millisecond cue is not rounded away: {rendered}"
        );
    }

    #[test]
    fn a_cue_with_no_text_is_dropped_rather_than_read_as_the_next_cue() {
        // The two formats use a blank line for both an empty cue and the gap between two
        // blocks, so this document cannot be written back as itself: the writer emits a
        // separator, an empty cue carries no line to distinguish it, and re-reading gives a
        // document with two cues of which one has lost its timing. The reader's job is to
        // lose nothing further, which is what the second assertion holds it to.
        let source =
            "1\n00:00:01,000 --> 00:00:02,000\n\n2\n00:00:03,000 --> 00:00:04,000\nAfter.\n\n";
        let document = SubtitleDocument::parse(SubtitleFormat::Srt, source).expect("parses");
        assert_eq!(document.cues.len(), 2);
        assert_eq!(
            document.cues[0].text.len(),
            0,
            "the cue carries no text line"
        );

        let rewritten = document.render(SubtitleFormat::Srt).expect("renders");
        let reparsed = SubtitleDocument::parse(SubtitleFormat::Srt, &rewritten).expect("parses");
        assert_eq!(reparsed.cues.len(), 2, "no cue is dropped");
        assert_eq!(reparsed.cues[1].timing, timing(3_000, 4_000));
        assert_eq!(reparsed.cues[1].text, vec!["After.".to_owned()]);
    }

    #[test]
    fn the_single_cue_case_is_not_a_special_one() {
        let source = "1\n00:00:01,000 --> 00:00:02,000\nOnly one.\n\n";
        let mut document = SubtitleDocument::parse(SubtitleFormat::Srt, source).expect("parses");
        assert_eq!(document.cues.len(), 1);
        assert_eq!(
            document.render(SubtitleFormat::Srt).expect("renders"),
            source
        );
        document.cues[0].identifier = None;
        assert_eq!(
            document.render(SubtitleFormat::Vtt).expect("renders"),
            "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nOnly one.\n\n"
        );
    }

    proptest! {
        // Malformed input of any shape never panics, and a cue that parsed is never lost on
        // the way out and back in.
        #[test]
        fn parsing_arbitrary_text_never_panics(
            input in any::<String>(),
            format in prop_oneof![
                Just(SubtitleFormat::Srt),
                Just(SubtitleFormat::Vtt),
                Just(SubtitleFormat::Ass),
                Just(SubtitleFormat::Ssa),
                Just(SubtitleFormat::Sub),
            ],
        ) {
            let Ok(document) = SubtitleDocument::parse(format, &input) else {
                return Ok(());
            };
            let Ok(rendered) = document.render(format) else {
                return Ok(());
            };
            if rendered.is_empty() {
                // A document that was nothing but an empty line writes nothing, and parsing
                // nothing is an error rather than a document with no cues.
                return Ok(());
            }
            let reparsed =
                SubtitleDocument::parse(format, &rendered).expect("the writer's own output parses");
            // SubRip numbers cues by position, so a document that came from anywhere but this
            // writer is not expected to keep its own numbers through a rewrite.
            prop_assert_eq!(
                reparsed.cues.len(),
                document.cues.len(),
                "a cue was lost between the writer and the parser"
            );
        }

        // The round trip a generated document has to survive is exact: same count, same
        // timing, same text, same settings. Its cues carry no identifier, because a SubRip
        // index is the position the writer assigns and so is not part of the comparison.
        #[test]
        fn a_generated_document_round_trips_through_srt_and_vtt(
            cues in prop::collection::vec(
                (0i64..7_200_000, 0i64..7_200_000, "[!-~\\p{Greek}][ -~\\p{Greek}]{0,39}", "[a-z0-9:%.]{1,20}"),
                1..12,
            ),
        ) {
            // A SubRip index is the position the writer assigns, so it is not part of the
            // document this property compares.
            let document = SubtitleDocument::new(
                cues.into_iter()
                    .map(|(start, length, text, settings)| {
                        let mut cue = SubtitleCue::line(timing(start, start + length), text);
                        cue.identifier = None;
                        cue.settings = (!settings.is_empty()).then_some(settings);
                        cue
                    })
                    .collect(),
            );
            let srt = SubtitleDocument::parse(
                SubtitleFormat::Srt,
                &document.render(SubtitleFormat::Srt).expect("renders"),
            )
            .expect("parses");
            prop_assert_eq!(srt.cues.len(), document.cues.len(), "SubRip lost a cue");
            for (reparsed, original) in srt.cues.iter().zip(&document.cues) {
                prop_assert_eq!(&reparsed.text, &original.text);
                prop_assert_eq!(reparsed.timing, original.timing);
                // SubRip has no cue settings, so a WebVTT setting is not expected to survive
                // the SubRip half of the round trip.
                prop_assert!(reparsed.settings.is_none());
            }
            let vtt = SubtitleDocument::parse(
                SubtitleFormat::Vtt,
                &document.render(SubtitleFormat::Vtt).expect("renders"),
            )
            .expect("parses");
            prop_assert_eq!(vtt.cues.len(), document.cues.len(), "WebVTT lost a cue");
            for (reparsed, original) in vtt.cues.iter().zip(&document.cues) {
                prop_assert_eq!(&reparsed.text, &original.text);
                prop_assert_eq!(reparsed.timing, original.timing);
                prop_assert_eq!(&reparsed.settings, &original.settings);
            }
        }

        // A cue an offset has pushed before zero survives the writer unchanged.
        #[test]
        fn a_cue_written_before_zero_round_trips(
            start in -7_200_000i64..7_200_000,
            length in 0i64..7_200_000,
        ) {
            let document = SubtitleDocument::new(vec![SubtitleCue::line(
                timing(start, start + length),
                "shifted",
            )]);
            let rendered = document.render(SubtitleFormat::Srt).expect("renders");
            let reparsed = SubtitleDocument::parse(SubtitleFormat::Srt, &rendered).expect("parses");
            prop_assert_eq!(reparsed.cues[0].timing, document.cues[0].timing);
        }
    }
}
