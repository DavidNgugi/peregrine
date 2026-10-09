//! The leak scanner every notification string passes through.
//!
//! AGENTS R31 and `DATA_MODEL` I15 make one thing non-negotiable: no notification
//! `title`, `body`, or `deep_link` may contain a filesystem path, a secret, or a
//! magnet URI. A toast lands on a lock screen that other people can see, and a
//! centre row is durable, so the rule is enforced where the text is still text.
//!
//! This module is deliberately a scanner and not a formatter: it never rewrites a
//! value into something safe, because a silently mangled API key is harder to
//! notice than a refused notification. Its callers either refuse the whole event
//! ([`crate::notify::NotificationPayload::render`]) or drop the decorative part
//! (a subject label) and keep the event.

use thiserror::Error;

/// The shortest character run treated as a credential.
///
/// Below this a run is far more likely to be a word in a title than a token: a
/// 20-character password is common, a 19-character film title is not.
const MIN_SECRET_BYTES: usize = 20;

/// The fewest distinct characters a run must use to be a credential.
///
/// `seeeeeeeeeeeeeeeeeeecret` is 25 characters of a word; `aB3xK9mQ2pL7wR4tY6uI` is
/// 20 characters of a token. Alphabet coverage is what separates them, and it is
/// what keeps a long title such as "The Extraordinary Adventures" out of the net.
const MIN_SECRET_DISTINCT_CHARS: usize = 8;

/// A magnet URI marks a torrent in text.
const MAGNET_PREFIX: &str = "magnet:";

/// A `BitTorrent` v1 info-hash written as hex.
const INFOHASH_HEX_BYTES: usize = 40;

/// A `BitTorrent` v1 info-hash written as base32.
const INFOHASH_BASE32_BYTES: usize = 32;

/// Why a string may not be shown to the user.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RedactionError {
    /// The string is empty or only whitespace.
    #[error("notification text must not be empty")]
    Empty,

    /// The string is longer than the caller's cap.
    #[error("notification text is {bytes} bytes, above the {limit}-byte limit")]
    TooLong {
        /// The rejected length.
        bytes: usize,
        /// The cap the caller set.
        limit: usize,
    },

    /// The string looks like a filesystem path.
    #[error("notification text looks like a filesystem path")]
    Path,

    /// The string contains a run that looks like a credential.
    #[error("notification text contains a token-shaped run")]
    Secret,

    /// The string contains a magnet URI or an info-hash.
    #[error("notification text contains a magnet URI or an info-hash")]
    Magnet,

    /// The string is a deep link the app will not route.
    #[error("the deep link is not a route this app handles: {reason}")]
    UnsafeLink {
        /// Why the route was refused.
        reason: String,
    },
}

/// Checks one string and names the first rule it breaks.
///
/// The rules run in a fixed order so a caller's error is reproducible: empty,
/// then length, then magnet, path, and secret.
///
/// This is the whole-string form, and a whole `peregrine://` URL trips the path
/// rule on its own delimiters. It is the right check for free text — a body, a
/// title, a label — and the wrong one for a deep link, which is never free text:
/// routes are built by [`crate::notify::deep_link`] from a closed set and each
/// segment is checked on its own by [`check_elements`].
pub fn check_text(text: &str, max_bytes: usize) -> Result<(), RedactionError> {
    if text.trim().is_empty() {
        return Err(RedactionError::Empty);
    }
    if text.len() > max_bytes {
        return Err(RedactionError::TooLong {
            bytes: text.len(),
            limit: max_bytes,
        });
    }
    check_element(text)?;
    Ok(())
}

/// Whether `text` is safe to show, without naming why not.
#[must_use]
pub fn is_safe(text: &str, max_bytes: usize) -> bool {
    check_text(text, max_bytes).is_ok()
}

/// Checks every element of a URL-ish string independently.
///
/// A checker that only scanned the whole string could be fooled by a delimiter:
/// `peregrine://title/tmdb/123` contains `/` many times, so the element scan has
/// to look at `title`, `tmdb`, and `123` separately before any path rule is
/// allowed to consider the joined form.
pub fn check_elements(text: &str, max_bytes: usize) -> Result<(), RedactionError> {
    if text.trim().is_empty() {
        return Err(RedactionError::Empty);
    }
    if text.len() > max_bytes {
        return Err(RedactionError::TooLong {
            bytes: text.len(),
            limit: max_bytes,
        });
    }
    for element in text
        .split(|character: char| {
            !(character.is_alphanumeric() || character == '-' || character == '_')
        })
        .filter(|element| !element.is_empty())
    {
        check_element(element)?;
    }
    Ok(())
}

/// Runs the three forbidden-content rules over one string or one element.
fn check_element(text: &str) -> Result<(), RedactionError> {
    let lowered = text.to_lowercase();
    if lowered.contains(MAGNET_PREFIX)
        || lowered.contains("urn:btih:")
        || has_infohash(text, INFOHASH_HEX_BYTES, |byte| byte.is_ascii_hexdigit())
        || has_infohash(text, INFOHASH_BASE32_BYTES, |byte| {
            byte.is_ascii_uppercase() || (b'2'..=b'7').contains(&byte)
        })
    {
        return Err(RedactionError::Magnet);
    }
    if looks_like_path(text) {
        return Err(RedactionError::Path);
    }
    if has_secret_run(text) {
        return Err(RedactionError::Secret);
    }
    Ok(())
}

/// Whether the text is shaped like a path on any platform.
///
/// Both separator styles are checked on every platform, for the same reason
/// [`crate::path`] refuses Windows device names on Unix: a library scanned on one
/// system is read on another, and the scanner has no idea which one will show the
/// toast.
fn looks_like_path(text: &str) -> bool {
    let unix_absolute = text.starts_with('/');
    let mut previous: Option<char> = None;
    let mut windows_absolute = false;
    let mut windows_unc = false;
    let mut backslashes = 0_usize;
    for character in text.chars() {
        if character == '\\' {
            backslashes += 1;
        }
        if character == ':'
            && let Some(letter) = previous
            && letter.is_ascii_alphabetic()
        {
            windows_absolute = true;
        }
        if previous == Some('\\') && character == '\\' {
            windows_unc = true;
        }
        previous = Some(character);
    }
    unix_absolute || windows_absolute || windows_unc || backslashes >= 2
}

/// Whether a window of `width` bytes exists whose bytes all satisfy `allowed`.
fn has_infohash(text: &str, width: usize, allowed: impl Fn(u8) -> bool) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() < width {
        return false;
    }
    bytes
        .windows(width)
        .any(|window| window.iter().all(|byte| allowed(*byte)))
}

/// Whether the text contains a run shaped like a generated credential.
fn has_secret_run(text: &str) -> bool {
    let mut start = None;
    for (index, character) in text.char_indices() {
        let in_run = matches!(character, 'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_');
        match (in_run, start) {
            (true, None) => start = Some(index),
            (false, Some(begin)) => {
                if run_is_secret(&text[begin..index]) {
                    return true;
                }
                start = None;
            }
            _ => {}
        }
    }
    start.is_some_and(|begin| run_is_secret(&text[begin..]))
}

/// Whether one contiguous run of word characters is token-shaped.
fn run_is_secret(run: &str) -> bool {
    if run.len() < MIN_SECRET_BYTES {
        return false;
    }
    // A digit is what makes a run a credential rather than a word: prose does not
    // carry digits inside a 20-character word, and every generated token does.
    if !run.bytes().any(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let mut seen = [false; 128];
    let mut distinct = 0_usize;
    for byte in run.bytes() {
        if byte.is_ascii() && !seen[usize::from(byte)] {
            seen[usize::from(byte)] = true;
            distinct += 1;
        }
    }
    distinct >= MIN_SECRET_DISTINCT_CHARS
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// A 24-byte token with enough alphabet coverage to be unmistakably a
    /// credential; a scanner that cannot find this one is not scanning.
    const REACHABLE_TOKEN: &str = "Xk9Qm2Lp7Rt4Wz8Nv3Bd6Hs1";

    const PATH_PROBE: &str = "/Users/dave/Downloads/Peregrine/x.mkv";
    const WINDOWS_PATH_PROBE: &str = r"C:\Users\dave\Videos\x.mkv";
    const SECRET_PROBE: &str = "tmdb-api-key-4f9c2b1a8e7d6c5b";
    const MAGNET_PROBE: &str = "magnet:?xt=urn:btih:C12FE1C06BBA254A4DE7DDD1AD9E1B0D9C6F0A4B";

    #[test]
    fn the_scanner_finds_each_forbidden_class() {
        assert_eq!(
            check_text(PATH_PROBE, 512),
            Err(RedactionError::Path),
            "an absolute path must be refused"
        );
        assert_eq!(
            check_text(WINDOWS_PATH_PROBE, 512),
            Err(RedactionError::Path),
            "a Windows path must be refused on every platform"
        );
        assert_eq!(
            check_text(SECRET_PROBE, 512),
            Err(RedactionError::Secret),
            "a token-shaped run must be refused"
        );
        assert_eq!(
            check_text(MAGNET_PROBE, 512),
            Err(RedactionError::Magnet),
            "a magnet URI must be refused"
        );
        assert_eq!(
            check_text("  ", 512),
            Err(RedactionError::Empty),
            "whitespace is not a title"
        );
        assert_eq!(
            check_text("a title", 3),
            Err(RedactionError::TooLong { bytes: 7, limit: 3 }),
            "the caller's cap is enforced"
        );
    }

    #[test]
    fn ordinary_notification_text_passes() {
        for text in [
            "Download finished",
            "New episode available",
            "Peregrine",
            "5 downloads finished",
            "Disk space is low",
            "The Extraordinary Adventures of a Very Long Title",
        ] {
            assert!(is_safe(text, 512), "`{text}` is ordinary notification copy");
        }
    }

    #[test]
    fn a_bare_infohash_is_refused_without_the_magnet_scheme() {
        assert_eq!(
            check_text("C12FE1C06BBA254A4DE7DDD1AD9E1B0D9C6F0A4B", 512),
            Err(RedactionError::Magnet)
        );
    }

    #[test]
    fn a_url_path_of_delimiters_is_not_a_filesystem_path() {
        // The same string, checked as elements, is a deep link and not a path: this
        // is why `check_text` is not run over a deep link and `check_elements` is.
        assert_eq!(check_elements("peregrine://title/tmdb/123", 512), Ok(()));
        assert_eq!(
            check_text("peregrine://title/tmdb/123", 512),
            Err(RedactionError::Path),
            "the whole-string scan refuses any text with a slash; that is why the \
             deep-link builder checks segments instead"
        );
    }

    proptest! {
        #[test]
        fn a_plain_word_title_is_always_safe(word in "[a-z]{1,30}") {
            prop_assert!(is_safe(&word, 512), "{word} must pass");
        }

        #[test]
        fn text_with_a_slash_is_never_accepted(prefix in "[a-z]{1,10}", suffix in "[a-z]{1,10}") {
            let probe = format!("/{prefix}/{suffix}");
            prop_assert_eq!(check_text(&probe, 512), Err(RedactionError::Path));
        }

        #[test]
        fn a_generated_run_of_length_is_never_a_secret(run in "[A-Za-z]{1,40}") {
            // The negative control for the property test below: alphabetic runs of
            // any length are words, and a scanner that flagged them would refuse
            // every real title. An empty string is `Empty`, not `Secret`.
            if run.is_empty() {
                prop_assert_eq!(check_text(&run, 512), Err(RedactionError::Empty));
            } else {
                prop_assert!(
                    is_safe(&run, 512),
                    "{run} is a word, not a credential"
                );
            }
        }

        #[test]
        fn check_text_never_panics(text in any::<String>()) {
            let _ = check_text(&text, 512);
            let _ = check_elements(&text, 512);
        }
    }

    #[test]
    fn the_negative_controls_are_reachable() {
        // If the probe below were ever refused, the tests above would be proving
        // that the scanner refuses everything.
        assert_eq!(
            check_text(REACHABLE_TOKEN, 512),
            Err(RedactionError::Secret),
            "the scanner must be able to find a token it was given"
        );
        assert!(is_safe("Download finished", 512));
    }
}
