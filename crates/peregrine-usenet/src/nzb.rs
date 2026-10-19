//! A bounded NZB reader and the subject-to-name sanitizer.
//!
//! An NZB is XML written by a stranger. The reader therefore applies its caps before it allocates
//! a collection, never resolves an external entity, and treats every attribute and every text
//! node as untrusted. A file's subject is not a path: it is reduced to a single safe basename,
//! and the engine writes only that name inside the job directory.

use peregrine_domain::{EntryKind, NonEmptyText, PayloadDescription, PayloadEntry};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use secrecy::SecretString;

/// The most bytes an NZB may be before it is refused unread.
pub const MAX_NZB_BYTES: usize = 10 * 1024 * 1024;
/// The most files one post may list.
pub const MAX_FILES: usize = 2_000;
/// The most segments one file may carry.
pub const MAX_SEGMENTS_PER_FILE: usize = 50_000;
/// The most segments one post may carry.
pub const MAX_SEGMENTS: usize = 200_000;
/// The longest safe basename this reader will produce, in bytes.
pub const MAX_NAME_BYTES: usize = 200;

/// Why an NZB could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NzbError {
    /// The body was larger than MAX_NZB_BYTES.
    #[error("the NZB is larger than this build will read")]
    TooLarge,
    /// The XML was not well formed.
    #[error("the NZB is not well-formed XML")]
    Malformed,
    /// The post listed more files than MAX_FILES.
    #[error("the NZB lists more files than this build will read")]
    TooManyFiles,
    /// The post listed more segments than the caps allow.
    #[error("the NZB lists more segments than this build will read")]
    TooManySegments,
    /// The post listed no usable file.
    #[error("the NZB lists no file")]
    Empty,
}

/// One article of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NzbSegment {
    /// The article's size as the NZB states it.
    pub bytes: u64,
    /// Its position within the file.
    pub number: u32,
    /// The message-id the NNTP server knows it by.
    pub message_id: String,
}

/// One file of a post.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NzbFile {
    /// The subject the poster wrote.
    pub subject: String,
    /// The poster.
    pub poster: String,
    /// The posting date, in Unix seconds.
    pub date: i64,
    /// The groups the article was posted to.
    pub groups: Vec<String>,
    /// The articles that make up the file, in order.
    pub segments: Vec<NzbSegment>,
    /// The safe basename the engine writes.
    pub name: NonEmptyText,
}

impl NzbFile {
    /// The encoded size the NZB states, summed over the segments.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.segments
            .iter()
            .fold(0u64, |total, segment| total.saturating_add(segment.bytes))
    }
}

/// One parsed post.
#[derive(Debug, Clone)]
pub struct NzbDocument {
    files: Vec<NzbFile>,
    password: Option<SecretString>,
}

impl NzbDocument {
    /// Parses a body no larger than MAX_NZB_BYTES.
    ///
    /// # Errors
    ///
    /// NzbError when the body is too large, malformed, over a cap, or lists no file.
    #[allow(clippy::too_many_lines)] // One linear pull-parser loop; splitting hides the state machine.
    pub fn parse(bytes: &[u8]) -> Result<Self, NzbError> {
        if bytes.len() > MAX_NZB_BYTES {
            return Err(NzbError::TooLarge);
        }
        let mut reader = Reader::from_reader(bytes);
        reader.config_mut().trim_text(true);
        let mut files = Vec::new();
        let mut file: Option<FileBuilder> = None;
        let mut segment: Option<SegmentBuilder> = None;
        let mut group: Option<String> = None;
        let mut password: Option<String> = None;
        let mut text = TextTarget::None;
        let mut total_segments = 0usize;
        let mut root_seen = false;
        let mut root_closed = false;

        loop {
            match reader.read_event() {
                Ok(Event::Start(event)) => match event.name().as_ref() {
                    b"file" => {
                        if files.len() >= MAX_FILES {
                            return Err(NzbError::TooManyFiles);
                        }
                        file = Some(FileBuilder::from_event(&event)?);
                    }
                    b"segment" => {
                        if file.is_none() {
                            return Err(NzbError::Malformed);
                        }
                        segment = Some(SegmentBuilder::from_event(&event)?);
                        text = TextTarget::Segment;
                    }
                    b"group" => {
                        if file.is_none() {
                            return Err(NzbError::Malformed);
                        }
                        group = Some(String::new());
                        text = TextTarget::Group;
                    }
                    b"meta" if attribute(&event, b"type")?.as_deref() == Some("password") => {
                        password = Some(String::new());
                        text = TextTarget::Password;
                    }
                    b"nzb" => root_seen = true,
                    _ => {}
                },
                Ok(Event::Empty(event)) => {
                    if event.name().as_ref() == b"segment" {
                        return Err(NzbError::Malformed);
                    }
                }
                Ok(Event::Text(event)) => {
                    let value = event.xml10_content().map_err(|_| NzbError::Malformed)?;
                    let value = value.trim();
                    if value.is_empty() {
                        continue;
                    }
                    match text {
                        TextTarget::Group => {
                            if let Some(group) = group.as_mut() {
                                group.push_str(value);
                            }
                        }
                        TextTarget::Segment => {
                            if let Some(segment) = segment.as_mut() {
                                segment.message_id.push_str(value);
                            }
                        }
                        TextTarget::Password => {
                            if let Some(password) = password.as_mut() {
                                password.push_str(value);
                            }
                        }
                        TextTarget::None => {}
                    }
                }
                Ok(Event::End(event)) => match event.name().as_ref() {
                    b"segment" => {
                        let Some(segment) = segment.take() else {
                            return Err(NzbError::Malformed);
                        };
                        if segment.message_id.is_empty() {
                            return Err(NzbError::Malformed);
                        }
                        let Some(file) = file.as_mut() else {
                            return Err(NzbError::Malformed);
                        };
                        if file.segments.len() >= MAX_SEGMENTS_PER_FILE {
                            return Err(NzbError::TooManySegments);
                        }
                        total_segments += 1;
                        if total_segments > MAX_SEGMENTS {
                            return Err(NzbError::TooManySegments);
                        }
                        file.segments.push(segment.finish());
                        text = TextTarget::None;
                    }
                    b"group" => {
                        if let (Some(group), Some(file)) = (group.take(), file.as_mut())
                            && !group.is_empty()
                        {
                            file.groups.push(group);
                        }
                        text = TextTarget::None;
                    }
                    b"meta" => {
                        text = TextTarget::None;
                    }
                    b"nzb" => root_closed = true,
                    b"file" => {
                        let Some(file) = file.take() else {
                            return Err(NzbError::Malformed);
                        };
                        if file.segments.is_empty() {
                            return Err(NzbError::Malformed);
                        }
                        files.push(file.finish(files.len()));
                        text = TextTarget::None;
                    }
                    _ => {}
                },
                Ok(Event::Eof) => break,
                Ok(_) => {}
                Err(_) => return Err(NzbError::Malformed),
            }
        }
        if root_seen && !root_closed {
            return Err(NzbError::Malformed);
        }
        if files.is_empty() {
            return Err(NzbError::Empty);
        }
        Ok(Self {
            files,
            password: password
                .filter(|value| !value.is_empty())
                .map(SecretString::from),
        })
    }

    /// The files, in the order the NZB listed them.
    #[must_use]
    pub fn files(&self) -> &[NzbFile] {
        &self.files
    }

    /// The password a post carries, when it carries one.
    #[must_use]
    pub fn password(&self) -> Option<&str> {
        self.password
            .as_ref()
            .map(secrecy::ExposeSecret::expose_secret)
    }

    /// The pre-download description the safety gate judges.
    #[must_use]
    pub fn description(&self) -> PayloadDescription {
        PayloadDescription::new(
            self.files
                .iter()
                .map(|file| PayloadEntry::new(file.name.as_str(), file.bytes(), EntryKind::File))
                .collect(),
        )
    }
}

#[derive(Clone, Copy)]
enum TextTarget {
    None,
    Group,
    Segment,
    Password,
}

struct FileBuilder {
    subject: String,
    poster: String,
    date: i64,
    groups: Vec<String>,
    segments: Vec<NzbSegment>,
}

impl FileBuilder {
    fn from_event(event: &BytesStart<'_>) -> Result<Self, NzbError> {
        Ok(Self {
            subject: attribute(event, b"subject")?.unwrap_or_default(),
            poster: attribute(event, b"poster")?.unwrap_or_default(),
            date: attribute(event, b"date")?
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(0),
            groups: Vec::new(),
            segments: Vec::new(),
        })
    }

    fn finish(self, index: usize) -> NzbFile {
        NzbFile {
            name: safe_file_name(&self.subject, index),
            subject: self.subject,
            poster: self.poster,
            date: self.date,
            groups: self.groups,
            segments: self.segments,
        }
    }
}

struct SegmentBuilder {
    bytes: u64,
    number: u32,
    message_id: String,
}

impl SegmentBuilder {
    fn from_event(event: &BytesStart<'_>) -> Result<Self, NzbError> {
        Ok(Self {
            bytes: attribute(event, b"bytes")?
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0),
            number: attribute(event, b"number")?
                .and_then(|value| value.parse::<u32>().ok())
                .ok_or(NzbError::Malformed)?,
            message_id: String::new(),
        })
    }

    fn finish(self) -> NzbSegment {
        // The reader may hand back the raw entity text or the decoded angle brackets; both are
        // one message-id, and NNTP wants the bracketed form.
        let id = self
            .message_id
            .trim()
            .trim_start_matches("&lt;")
            .trim_end_matches("&gt;")
            .trim_matches(['<', '>'])
            .trim();
        NzbSegment {
            bytes: self.bytes,
            number: self.number,
            message_id: format!("<{id}>"),
        }
    }
}

fn attribute(event: &BytesStart<'_>, name: &[u8]) -> Result<Option<String>, NzbError> {
    for attribute in event.attributes().with_checks(false) {
        let attribute = attribute.map_err(|_| NzbError::Malformed)?;
        if attribute.key.as_ref() == name {
            return attribute
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map(|value| Some(value.into_owned()))
                .map_err(|_| NzbError::Malformed);
        }
    }
    Ok(None)
}

/// Reduces an NZB subject to one safe basename.
///
/// The subject is the only name a post offers, and it is written by the poster. The result has no
/// separator, no control character, no bidirection-override character, and no Windows device name;
/// an empty or unusable subject becomes file-<index>.
#[must_use]
pub fn safe_file_name(subject: &str, index: usize) -> NonEmptyText {
    let mut candidate = quoted_name(subject).unwrap_or_else(|| subject.to_owned());
    if let Some(separator) = candidate.rfind(['/', '\\']) {
        candidate = candidate[separator + 1..].to_owned();
    }
    let mut cleaned = String::with_capacity(candidate.len());
    for character in candidate.chars() {
        if character.is_control() || is_bidi_control(character) {
            continue;
        }
        cleaned.push(match character {
            '/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            other => other,
        });
    }
    let cleaned = cleaned.trim().trim_end_matches(['.', ' ']).trim();
    let mut name = if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        format!("file-{}", index + 1)
    } else {
        cleaned.to_owned()
    };
    if is_reserved(&name) {
        name.insert(0, '_');
    }
    let mut end = name.len().min(MAX_NAME_BYTES);
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    name.truncate(end);
    NonEmptyText::new(&name).unwrap_or_else(|_| {
        NonEmptyText::new(&format!("file-{}", index + 1)).expect("a fallback name is not empty")
    })
}

fn quoted_name(subject: &str) -> Option<String> {
    let start = subject.find('"')?;
    let rest = &subject[start + 1..];
    let end = rest.find('"')?;
    let name = rest[..end].trim();
    (!name.is_empty()).then(|| name.to_owned())
}

fn is_bidi_control(character: char) -> bool {
    matches!(
        character,
        '\u{202A}'
            | '\u{202B}'
            | '\u{202C}'
            | '\u{202D}'
            | '\u{202E}'
            | '\u{2066}'
            | '\u{2067}'
            | '\u{2068}'
            | '\u{2069}'
    )
}

fn is_reserved(name: &str) -> bool {
    let stem = name
        .split_once('.')
        .map_or(name, |(stem, _)| stem)
        .trim()
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<nzb xmlns="http://www.newzbin.com/DTD/2003/nzb">
  <head><meta type="password">secret</meta></head>
  <file poster="poster &lt;a@b&gt;" date="1700000000" subject="[1/2] - &quot;Movie.2024.1080p.mkv&quot; yEnc (1/2) 100">
    <groups><group>alt.binaries.test</group></groups>
    <segments>
      <segment bytes="50" number="1">&lt;one@test&gt;</segment>
      <segment bytes="50" number="2">&lt;two@test&gt;</segment>
    </segments>
  </file>
</nzb>"#;

    #[test]
    fn a_post_parses_into_files_segments_and_a_description() {
        let document = NzbDocument::parse(SAMPLE.as_bytes()).expect("the sample parses");
        assert_eq!(document.files().len(), 1);
        let file = &document.files()[0];
        assert_eq!(file.name.as_str(), "Movie.2024.1080p.mkv");
        assert_eq!(file.bytes(), 100);
        assert_eq!(file.segments.len(), 2);
        assert_eq!(file.segments[0].message_id, "<one@test>");
        assert_eq!(file.groups, ["alt.binaries.test"]);
        assert_eq!(document.password(), Some("secret"));
        let description = document.description();
        assert_eq!(description.file_count(), 1);
        assert_eq!(description.entries()[0].bytes(), 100);
    }

    #[test]
    fn malformed_and_empty_posts_are_refused() {
        assert!(matches!(NzbDocument::parse(b""), Err(NzbError::Empty)));
        assert_eq!(
            NzbDocument::parse(b"<nzb><file subject=\"x\"></file></nzb>").map(|_| ()),
            Err(NzbError::Malformed)
        );
        assert_eq!(
            NzbDocument::parse(b"<nzb>").map(|_| ()),
            Err(NzbError::Malformed)
        );
    }

    #[test]
    fn a_subject_is_reduced_to_one_safe_basename() {
        assert_eq!(safe_file_name("../escape.mkv", 0).as_str(), "escape.mkv");
        assert_eq!(
            safe_file_name(r"C:\Windows\evil.exe", 0).as_str(),
            "evil.exe"
        );
        assert_eq!(safe_file_name("\"CON.mkv\"", 0).as_str(), "_CON.mkv");
        assert_eq!(safe_file_name("..", 0).as_str(), "file-1");
        assert_eq!(safe_file_name("", 2).as_str(), "file-3");
        assert_eq!(
            safe_file_name("safe\u{202E}gnp.exe", 0).as_str(),
            "safegnp.exe"
        );
        let long = safe_file_name(&"a".repeat(500), 0);
        assert!(long.as_str().len() <= MAX_NAME_BYTES);
    }

    proptest! {
        #[test]
        fn parse_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
            let _ = NzbDocument::parse(&bytes);
        }
    }
}
