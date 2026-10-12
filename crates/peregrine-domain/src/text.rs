//! Required text.
//!
//! `NOT NULL` in the schema does not exclude `''`, and `SQLite` will store one without
//! complaint. Every column this type guards is a name, an identifier, or a path that the
//! application acts on, and an empty one is not a value: a `library_root` whose `path` is
//! empty is not a root, and a genre without a name can be neither shown nor matched. Making
//! emptiness unrepresentable means no constructor, no row mapper, and no deserializer can
//! produce one.
//!
//! Paths are deliberately *not* checked for containment here. `PathGuard` owns that
//! (AGENTS R21) and it needs the configured root, which a value rebuilt from a stored row
//! does not have; the scan boundary is where a path meets the guard.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Text a row must have: not empty after trimming, stored exactly as given.
///
/// The surrounding whitespace of a non-empty value is preserved rather than trimmed, because
/// trimming a title is a normalization decision that belongs to the provider adapter, not to
/// the type that stores the result.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct NonEmptyText(String);

impl NonEmptyText {
    /// Wraps text, refusing empty and whitespace-only input.
    ///
    /// # Errors
    ///
    /// [`TextError::Empty`].
    pub fn new(value: &str) -> Result<Self, TextError> {
        if value.trim().is_empty() {
            return Err(TextError::Empty);
        }
        Ok(Self(value.to_owned()))
    }

    /// The stored text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NonEmptyText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for NonEmptyText {
    type Error = TextError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.trim().is_empty() {
            return Err(TextError::Empty);
        }
        Ok(Self(value))
    }
}

impl From<NonEmptyText> for String {
    fn from(value: NonEmptyText) -> Self {
        value.0
    }
}

/// Why required text was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TextError {
    /// The value was empty, or only whitespace.
    #[error("required text is empty")]
    Empty,
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Every character `char::is_whitespace` reports, which is the set `str::trim` uses. The
    /// property test generates from the list rather than from `any::<char>()` so that the
    /// Unicode members are actually exercised, not just the ASCII ones.
    const WHITESPACE: [char; 25] = [
        '\u{9}', '\u{a}', '\u{b}', '\u{c}', '\u{d}', '\u{20}', '\u{85}', '\u{a0}', '\u{1680}',
        '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}', '\u{2006}',
        '\u{2007}', '\u{2008}', '\u{2009}', '\u{200a}', '\u{2028}', '\u{2029}', '\u{202f}',
        '\u{205f}', '\u{3000}',
    ];

    #[test]
    fn required_text_is_non_empty() {
        assert_eq!(
            NonEmptyText::new("The Matrix").unwrap().as_str(),
            "The Matrix"
        );
        assert_eq!(NonEmptyText::new("0").unwrap().as_str(), "0");
        assert_eq!(NonEmptyText::new("  \\  ").unwrap().as_str(), "  \\  ");
    }

    #[test]
    fn required_text_refuses_empty_and_whitespace_only_input() {
        for refused in ["", " ", "\t", "\n", " \t\n "] {
            assert_eq!(NonEmptyText::new(refused), Err(TextError::Empty));
            assert_eq!(
                NonEmptyText::try_from(refused.to_owned()),
                Err(TextError::Empty)
            );
        }
    }

    #[test]
    fn required_text_keeps_its_own_whitespace() {
        let padded = NonEmptyText::new("  The Matrix  ").unwrap();
        assert_eq!(padded.as_str(), "  The Matrix  ");
        assert_eq!(String::from(padded.clone()), "  The Matrix  ");
        assert_eq!(padded.to_string(), "  The Matrix  ");
    }

    #[test]
    fn required_text_round_trips_through_json() {
        let text = NonEmptyText::new("Blade Runner 2049").unwrap();
        let json = serde_json::to_string(&text).unwrap();
        assert_eq!(json, "\"Blade Runner 2049\"");
        assert_eq!(serde_json::from_str::<NonEmptyText>(&json).unwrap(), text);
    }

    #[test]
    fn required_text_json_rejects_an_empty_string() {
        assert!(serde_json::from_str::<NonEmptyText>("\"\"").is_err());
        assert!(serde_json::from_str::<NonEmptyText>("\"   \"").is_err());
    }

    proptest! {
        #[test]
        fn required_text_round_trips_any_value_it_accepts(value in ".{1,32}") {
            // Whether a string is empty is decided by `trim`, which knows every Unicode
            // whitespace character; the strategy cannot enumerate them, so it asks instead.
            prop_assume!(!value.trim().is_empty());
            let text = NonEmptyText::new(&value).expect("the value is not whitespace");
            prop_assert_eq!(text.as_str(), value.as_str());
            let json = serde_json::to_string(&text).expect("serializes");
            prop_assert_eq!(serde_json::from_str::<NonEmptyText>(&json).expect("deserializes"), text);
        }

        #[test]
        fn required_text_refuses_whitespace_of_any_length(
            blank in proptest::collection::vec(prop::sample::select(WHITESPACE.to_vec()), 0..16)
        ) {
            let blank: String = blank.into_iter().collect();
            prop_assert_eq!(NonEmptyText::new(&blank), Err(TextError::Empty));
        }
    }
}
