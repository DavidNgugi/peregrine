//! The in-memory wrapper for a credential.
//!
//! The wrapper exists so that holding a secret is safe by construction rather
//! than by remembering a rule. It has no `Display` and no `Serialize`, and its
//! `Debug` is a fixed marker, so `#[derive(Debug)]` on a struct that contains
//! one cannot print the value, an error message cannot interpolate it, and a
//! config export cannot encode it. The copy in memory is zeroized on drop by
//! `secrecy`.

use secrecy::{ExposeSecret as _, SecretString};

/// A credential in memory.
///
/// The value is reachable only through [`Secret::expose_secret`]. Creating one
/// also registers the value with the redaction filter, so a value that is
/// exposed into a format string is still removed from the log line.
///
/// ```
/// use peregrine_secrets::Secret;
///
/// let secret = Secret::new("hunter2");
/// assert_eq!(format!("{secret:?}"), "Secret([REDACTED])");
/// assert_eq!(secret.expose_secret(), "hunter2");
/// ```
///
/// There is no `Display`, so interpolating a secret into a message — or into an
/// error's message — does not compile:
///
/// ```compile_fail
/// let secret = peregrine_secrets::Secret::new("hunter2");
/// let rendered = format!("{secret}");
/// ```
///
/// There is no `Serialize` either, so a secret cannot be written to the database
/// or the config export. The first block below shows that `serde_json` is linked
/// into this doctest, which is what makes the second block fail on the missing
/// impl rather than on a missing import:
///
/// ```
/// let public = serde_json::json!({ "name": "peregrine" });
/// assert_eq!(public["name"], "peregrine");
/// ```
///
/// ```compile_fail
/// let secret = peregrine_secrets::Secret::new("hunter2");
/// let exported = serde_json::to_string(&secret);
/// ```
pub struct Secret(SecretString);

impl Secret {
    /// Wraps `value` and registers it for log redaction.
    pub fn new(value: impl Into<String>) -> Self {
        let value: String = value.into();
        let secret = SecretString::from(value);
        crate::redact::register(secret.expose_secret());
        Self(secret)
    }

    /// The value, for the one caller that needs it.
    ///
    /// The name matches `secrecy`'s trait so that a call site reads as a
    /// deliberate exposure in review.
    pub fn expose_secret(&self) -> &str {
        self.0.expose_secret()
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Secret([REDACTED])")
    }
}
