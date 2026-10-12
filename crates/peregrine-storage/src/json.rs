//! The JSON text columns a row mapper has to encode and decode itself.
//!
//! Exactly one column in the tables this crate writes holds JSON whose *shape* the mapper
//! owns rather than carrying opaquely: `media_file.audio_languages`, documented in
//! `DATA_MODEL` §2.4 as "JSON array of BCP-47". It is an array of short strings, and this
//! module reads and writes that shape and nothing else — a general JSON parser would be a
//! dependency whose behaviour this crate does not need, and the columns that *are* general
//! JSON (`probe_json`, `engine_state_json`, `rules_json`) travel as bytes or strings with no
//! interpretation (AGENTS R2).
//!
//! An escape this build did not write is refused rather than guessed at, the same way the
//! stored enums refuse an unknown spelling: the value came from another version, and
//! reporting the column is what makes that diagnosable.

use std::fmt::Write as _;

use crate::error::StorageError;

/// Encodes a list of strings as a JSON array.
///
/// The output is what [`decode_string_array`] reads back, and what any other JSON reader
/// would read as an array of strings. There is no whitespace between elements, so two equal
/// arrays are byte-equal.
pub(crate) fn encode_string_array(values: &[String]) -> String {
    let mut encoded = String::from("[");
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            encoded.push(',');
        }
        encoded.push('"');
        for character in value.chars() {
            match character {
                '"' => encoded.push_str("\\\""),
                '\\' => encoded.push_str("\\\\"),
                '\n' => encoded.push_str("\\n"),
                '\r' => encoded.push_str("\\r"),
                '\t' => encoded.push_str("\\t"),
                // JSON forbids a raw control character inside a string, so the rest are
                // written as `\u00XX`.
                control if control < ' ' => {
                    // Writing into the string cannot fail, which is what the ignored result
                    // says; `format!` here would allocate a second string per escape.
                    let _ = write!(encoded, "\\u{:04x}", u32::from(control));
                }
                other => encoded.push(other),
            }
        }
        encoded.push('"');
    }
    encoded.push(']');
    encoded
}

/// Decodes a JSON array of strings, or reports the column it could not read.
///
/// Whitespace around and between elements is accepted, because a hand-edited column is a
/// thing that happens and its content is still unambiguous.
///
/// # Errors
///
/// [`StorageError::InvalidJsonField`] for anything that is not an array of strings with the
/// escapes [`encode_string_array`] writes.
pub(crate) fn decode_string_array(
    column: &'static str,
    text: &str,
) -> Result<Vec<String>, StorageError> {
    let invalid = |detail: &str| StorageError::InvalidJsonField {
        column,
        detail: detail.to_owned(),
    };

    let mut characters = text.chars().peekable();
    skip_whitespace(&mut characters);
    if characters.next() != Some('[') {
        return Err(invalid("it does not start with `[`"));
    }
    skip_whitespace(&mut characters);

    let mut values = Vec::new();
    if characters.peek() == Some(&']') {
        characters.next();
        return trailing(characters, &invalid).map(|()| values);
    }

    loop {
        match characters.next() {
            Some('"') => values.push(read_string(&mut characters, &invalid)?),
            Some(found) => {
                return Err(invalid(&format!(
                    "it holds `{found}` where a value was due"
                )));
            }
            None => return Err(invalid("it ends before the array does")),
        }
        skip_whitespace(&mut characters);
        match characters.next() {
            Some(',') => skip_whitespace(&mut characters),
            Some(']') => return trailing(characters, &invalid).map(|()| values),
            Some(found) => {
                return Err(invalid(&format!(
                    "it holds `{found}` where a comma or `]` was due"
                )));
            }
            None => return Err(invalid("it ends before the array does")),
        }
    }
}

/// Accepts only the end of the text after the array's closing bracket.
fn trailing(
    mut remaining: std::iter::Peekable<std::str::Chars<'_>>,
    invalid: &impl Fn(&str) -> StorageError,
) -> Result<(), StorageError> {
    skip_whitespace(&mut remaining);
    if remaining.next().is_some() {
        return Err(invalid("it holds more than the array"));
    }
    Ok(())
}

fn skip_whitespace(characters: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while characters
        .peek()
        .is_some_and(|character| character.is_whitespace())
    {
        characters.next();
    }
}

/// Reads one quoted string, which the caller has already seen the opening quote of.
fn read_string(
    characters: &mut std::iter::Peekable<std::str::Chars<'_>>,
    invalid: &impl Fn(&str) -> StorageError,
) -> Result<String, StorageError> {
    let mut value = String::new();
    loop {
        match characters.next() {
            Some('"') => return Ok(value),
            Some('\\') => match characters.next() {
                Some('"') => value.push('"'),
                Some('\\') => value.push('\\'),
                Some('n') => value.push('\n'),
                Some('r') => value.push('\r'),
                Some('t') => value.push('\t'),
                Some('u') => {
                    let digits: String = characters.by_ref().take(4).collect();
                    if digits.len() != 4 {
                        return Err(invalid("a `\\u` escape is shorter than four digits"));
                    }
                    let code = u32::from_str_radix(&digits, 16)
                        .map_err(|_| invalid("a `\\u` escape is not hexadecimal"))?;
                    value.push(
                        char::from_u32(code)
                            .ok_or_else(|| invalid("a `\\u` escape is not a character"))?,
                    );
                }
                Some(found) => {
                    return Err(invalid(&format!("it holds an unknown `\\{found}` escape")));
                }
                None => return Err(invalid("an escape ends the text")),
            },
            Some(found) => value.push(found),
            None => return Err(invalid("a string is not closed")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn a_string_array_is_written_as_json() {
        assert_eq!(encode_string_array(&[]), "[]");
        assert_eq!(encode_string_array(&strings(&["en"])), "[\"en\"]");
        assert_eq!(
            encode_string_array(&strings(&["en", "fr"])),
            "[\"en\",\"fr\"]",
            "no space after the comma, which is what keeps two equal arrays byte-equal"
        );
    }

    #[test]
    fn the_characters_json_reserves_are_escaped() {
        assert_eq!(
            encode_string_array(&strings(&["a\"b", "c\\d", "e\nf", "g\th"])),
            "[\"a\\\"b\",\"c\\\\d\",\"e\\nf\",\"g\\th\"]"
        );
        assert_eq!(encode_string_array(&strings(&["\u{1}"])), "[\"\\u0001\"]");
    }

    #[test]
    fn an_encoded_array_is_read_back() {
        for values in [
            strings(&[]),
            strings(&["en"]),
            strings(&["en", "fr", "ja"]),
            strings(&["a\"b", "c\\d", "e\nf"]),
            strings(&["\u{1}", "ja"]),
        ] {
            let encoded = encode_string_array(&values);
            assert_eq!(
                decode_string_array("media_file.audio_languages", &encoded).unwrap(),
                values,
                "{encoded}"
            );
        }
    }

    /// A reader that accepted anything would turn a corrupt column into a plausible-looking
    /// list of languages, which is worse than refusing the row.
    #[test]
    fn text_that_is_not_a_string_array_is_refused() {
        for refused in [
            "",
            "en",
            "{",
            "[",
            "[\"en\"",
            "[\"en\",]",
            "[\"en\"] trailing",
            "[en]",
            "[1]",
            "[\"a\" \"b\"]",
            "[\"a\\qb\"]",
            "[\"a\\u00zz\"]",
            "[\"a\\u00\"]",
        ] {
            let error = decode_string_array("media_file.audio_languages", refused)
                .expect_err("this is not an array of strings");
            assert!(
                matches!(
                    error,
                    StorageError::InvalidJsonField {
                        column: "media_file.audio_languages",
                        ..
                    }
                ),
                "{refused}: {error:?}"
            );
        }
    }

    #[test]
    fn whitespace_around_and_between_elements_is_accepted() {
        assert_eq!(
            decode_string_array("media_file.audio_languages", " [ \"en\" , \"fr\" ] ").unwrap(),
            strings(&["en", "fr"])
        );
        assert_eq!(
            decode_string_array("media_file.audio_languages", "  [  ]  ").unwrap(),
            Vec::<String>::new()
        );
    }

    proptest! {
        /// The codec's whole contract: every list of strings survives the round trip, which
        /// is what makes the column safe to write and read for any BCP-47 tag a probe
        /// reports.
        #[test]
        fn any_string_array_round_trips(
            values in proptest::collection::vec(".{0,16}", 0..8)
        ) {
            let encoded = encode_string_array(&values);
            prop_assert_eq!(
                decode_string_array("media_file.audio_languages", &encoded)
                    .expect("the codec reads what it wrote"),
                values
            );
        }
    }
}
