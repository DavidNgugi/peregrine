//! A yEnc decoder for one article.
//!
//! yEnc is not a standard, but every binary Usenet post uses it. The decoder is deliberately
//! narrow: it reads the header lines it needs, decodes one article, checks the part and whole CRC
//! when the poster supplied them, and refuses an article larger than the cap or one with a broken
//! escape. It never allocates from a size the header claims.

use crc32fast::Hasher;

/// The most decoded bytes one article may produce.
pub const MAX_ARTICLE_BYTES: usize = 16 * 1024 * 1024;

/// Why an article could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum YencError {
    /// The article carried no ybegin line.
    #[error("the article is not yEnc")]
    NoHeader,
    /// A line ended in a bare escape.
    #[error("the yEnc article has a broken escape")]
    BadEscape,
    /// A header line was malformed.
    #[error("the yEnc header is malformed")]
    BadHeader,
    /// The decoded data did not match the CRC the header stated.
    #[error("the yEnc article did not match its checksum")]
    CrcMismatch,
    /// The decoded data exceeded MAX_ARTICLE_BYTES.
    #[error("the yEnc article is larger than this build will decode")]
    TooLarge,
    /// The article carried no data between the header and the end marker.
    #[error("the yEnc article carried no data")]
    Empty,
}

/// One decoded article.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedArticle {
    /// The decoded bytes.
    pub data: Vec<u8>,
    /// The 1-based first byte of this part in the file, when ypart named it.
    pub begin: Option<u64>,
    /// The 1-based last byte of this part, inclusive, when ypart named it.
    pub end: Option<u64>,
    /// The whole file's size, when ybegin named it.
    pub size: Option<u64>,
    /// The filename ybegin carried, when it carried one.
    pub name: Option<String>,
    /// The part number, when ybegin or ypart named it.
    pub part: Option<u32>,
    /// The number of parts, when ybegin named it.
    pub total: Option<u32>,
}

/// Decodes one article.
///
/// # Errors
///
/// YencError when the article is not yEnc, malformed, over the cap, empty, or fails a CRC.
pub fn decode(bytes: &[u8]) -> Result<DecodedArticle, YencError> {
    let mut header: Option<Header> = None;
    let mut part: Option<Part> = None;
    let mut end: Option<End> = None;
    let mut data = Vec::new();
    let mut in_data = false;

    for line in bytes.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.starts_with(b"=ybegin ") {
            header = Some(parse_header(line, b"=ybegin ")?);
            in_data = true;
            continue;
        }
        if line.starts_with(b"=ypart ") {
            part = Some(parse_part(line)?);
            continue;
        }
        if line.starts_with(b"=yend ") {
            end = Some(parse_end(line)?);
            in_data = false;
            continue;
        }
        if in_data && !line.is_empty() {
            decode_line(line, &mut data)?;
            if data.len() > MAX_ARTICLE_BYTES {
                return Err(YencError::TooLarge);
            }
        }
    }

    let header = header.ok_or(YencError::NoHeader)?;
    if data.is_empty() {
        return Err(YencError::Empty);
    }
    let digest = crc32(&data);
    if let Some(expected) = part.as_ref().and_then(|part| part.pcrc32)
        && expected != digest
    {
        return Err(YencError::CrcMismatch);
    }
    if let Some(expected) = end.as_ref().and_then(|end| end.crc32)
        && expected != digest
    {
        return Err(YencError::CrcMismatch);
    }
    Ok(DecodedArticle {
        data,
        begin: part.as_ref().and_then(|part| part.begin),
        end: part.and_then(|part| part.end),
        size: header.size,
        name: header.name,
        part: header.part,
        total: header.total,
    })
}

struct Header {
    size: Option<u64>,
    name: Option<String>,
    part: Option<u32>,
    total: Option<u32>,
}

struct Part {
    begin: Option<u64>,
    end: Option<u64>,
    pcrc32: Option<u32>,
}

struct End {
    crc32: Option<u32>,
}

fn parse_header(line: &[u8], prefix: &[u8]) -> Result<Header, YencError> {
    let text = std::str::from_utf8(&line[prefix.len()..]).map_err(|_| YencError::BadHeader)?;
    let mut header = Header {
        size: None,
        name: None,
        part: None,
        total: None,
    };
    for field in text.split(' ') {
        if let Some((key, value)) = field.split_once('=') {
            match key {
                "size" => header.size = value.parse().ok(),
                "name" => header.name = Some(value.to_owned()),
                "part" => header.part = value.parse().ok(),
                "total" => header.total = value.parse().ok(),
                _ => {}
            }
        }
    }
    Ok(header)
}

fn parse_part(line: &[u8]) -> Result<Part, YencError> {
    let text = std::str::from_utf8(&line[6..]).map_err(|_| YencError::BadHeader)?;
    let mut part = Part {
        begin: None,
        end: None,
        pcrc32: None,
    };
    for field in text.split(' ') {
        if let Some((key, value)) = field.split_once('=') {
            match key {
                "begin" => part.begin = value.parse().ok(),
                "end" => part.end = value.parse().ok(),
                "pcrc32" => part.pcrc32 = parse_crc(value),
                _ => {}
            }
        }
    }
    Ok(part)
}

fn parse_end(line: &[u8]) -> Result<End, YencError> {
    let text = std::str::from_utf8(&line[6..]).map_err(|_| YencError::BadHeader)?;
    let mut end = End { crc32: None };
    for field in text.split(' ') {
        if let Some((key, value)) = field.split_once('=')
            && key == "crc32"
        {
            end.crc32 = parse_crc(value);
        }
    }
    Ok(end)
}

fn parse_crc(value: &str) -> Option<u32> {
    u32::from_str_radix(value, 16).ok()
}

fn crc32(data: &[u8]) -> u32 {
    let mut hasher = Hasher::new();
    hasher.update(data);
    hasher.finalize()
}

fn decode_line(line: &[u8], data: &mut Vec<u8>) -> Result<(), YencError> {
    let mut index = 0;
    while index < line.len() {
        let byte = line[index];
        let value = if byte == b'=' {
            index += 1;
            let Some(escaped) = line.get(index) else {
                return Err(YencError::BadEscape);
            };
            escaped.wrapping_sub(64)
        } else {
            byte
        };
        data.push(value.wrapping_sub(42));
        index += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn crc(data: &[u8]) -> u32 {
        let mut hasher = Hasher::new();
        hasher.update(data);
        hasher.finalize()
    }

    fn article(data: &[u8], crc: u32) -> Vec<u8> {
        let mut out =
            format!("=ybegin line=128 size={} name=file.bin\r\n", data.len()).into_bytes();
        for byte in data {
            let encoded = byte.wrapping_add(42);
            if matches!(encoded, 0 | 10 | 13 | 61) {
                out.push(b'=');
                out.push(encoded.wrapping_add(64));
            } else {
                out.push(encoded);
            }
        }
        out.extend_from_slice(
            format!("\r\n=yend size={} crc32={crc:08x}\r\n", data.len()).as_bytes(),
        );
        out
    }

    #[test]
    fn a_single_part_article_decodes_and_checks_its_crc() {
        let body = article(b"hello", crc(b"hello"));
        let decoded = decode(&body).expect("the article decodes");
        assert_eq!(decoded.data, b"hello");
        assert_eq!(decoded.size, Some(5));
        assert_eq!(decoded.name.as_deref(), Some("file.bin"));
    }

    #[test]
    fn a_bad_crc_is_refused() {
        let body = article(b"hello", 0);
        assert_eq!(decode(&body), Err(YencError::CrcMismatch));
    }

    #[test]
    fn an_escaped_byte_decodes() {
        let body = article(&[0x13], crc(&[0x13]));
        assert!(body.windows(2).any(|pair| pair == b"=}"));
        assert_eq!(decode(&body).unwrap().data, [0x13]);
    }

    #[test]
    fn a_broken_escape_is_refused() {
        let article = b"=ybegin line=128 size=1 name=x.bin\r\n=\r\n=yend size=1\r\n";
        assert_eq!(decode(article), Err(YencError::BadEscape));
    }

    proptest! {
        #[test]
        fn decode_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
            let _ = decode(&bytes);
        }
    }
}
