//! The structured error every command returns.
//!
//! The shape is fixed by ARCHITECTURE §7 rule 3: `{ code, message, detail?,
//! retryable, cause? }`, where `code` is a stable enum shared with the UI. Two
//! consequences drive the design here:
//!
//! * `retryable` is taken from the code's metadata rather than passed in by the
//!   caller, so a retryability claim can never disagree with the code it
//!   describes.
//! * The serialized form of a code is part of the contract. The UI branches on
//!   it and the message catalogue is keyed by it, so `serialized_forms_are_stable`
//!   pins the strings and a rename becomes a visible, deliberate change.

use serde::{Deserialize, Serialize};

/// A stable, machine-readable failure code.
///
/// Each variant records the documented condition that produces it, so the
/// taxonomy stays tied to observed behaviour instead of growing into a list of
/// hypothetical situations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// The caller's input failed validation before any work started.
    InvalidInput,
    /// The requested entity does not exist, or is not visible in this scope.
    NotFound,
    /// The request conflicts with existing state, such as a duplicate job.
    Conflict,
    /// The database is locked or busy; the same call can succeed later.
    StorageBusy,
    /// `PRAGMA integrity_check` failed; recovery is a rebuild, not a retry.
    StorageCorrupt,
    /// The OS denied the operation and only the user can grant it.
    PermissionDenied,
    /// There is not enough free space; freeing space requires the user.
    DiskFull,
    /// The network is unavailable or a connection failed.
    Network,
    /// A provider returned `429`, or an exhausted quota with a known reset.
    ProviderRateLimited,
    /// A provider returned `5xx` or was unreachable.
    ProviderUnavailable,
    /// A provider's payload no longer matches the shape we decode.
    ProviderSchemaDrift,
    /// Playback failed; retrying from the last known position is offered.
    PlaybackFailed,
    /// A cast renderer went away; another device or local playback can resume.
    DeviceUnavailable,
    /// The user cancelled, so the failure is expected and not worth reporting.
    Cancelled,
    /// The operation is not supported on this platform or configuration.
    Unsupported,
    /// An invariant was violated. This is a bug, and the code exists so the UI
    /// can say so plainly instead of showing a raw message.
    Internal,
}

/// What the UI needs to know about a code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeMetadata {
    /// The code this metadata describes. Redundant with the enum, but it makes a
    /// metadata lookup self-describing at the call site.
    pub code: ErrorCode,
    /// Catalog key for the user-facing message. The pattern is enforced by test.
    pub message_key: &'static str,
    /// English fallback, used before the catalogue is loaded and in logs.
    pub message: &'static str,
    /// Whether retrying the same operation, unchanged, could succeed.
    pub retryable: bool,
}

impl ErrorCode {
    /// Every code, for exhaustive checks. Kept in the same order as the enum.
    pub const ALL: &[ErrorCode] = &[
        ErrorCode::InvalidInput,
        ErrorCode::NotFound,
        ErrorCode::Conflict,
        ErrorCode::StorageBusy,
        ErrorCode::StorageCorrupt,
        ErrorCode::PermissionDenied,
        ErrorCode::DiskFull,
        ErrorCode::Network,
        ErrorCode::ProviderRateLimited,
        ErrorCode::ProviderUnavailable,
        ErrorCode::ProviderSchemaDrift,
        ErrorCode::PlaybackFailed,
        ErrorCode::DeviceUnavailable,
        ErrorCode::Cancelled,
        ErrorCode::Unsupported,
        ErrorCode::Internal,
    ];

    /// The code's UI metadata.
    ///
    /// The `match` is exhaustive on purpose: a new code cannot be added without
    /// deciding its message key and retryability, because the crate will not
    /// compile until it does.
    #[must_use]
    pub const fn metadata(self) -> CodeMetadata {
        let (message_key, message, retryable) = match self {
            ErrorCode::InvalidInput => ("error.invalid_input", "That input is not valid.", false),
            ErrorCode::NotFound => ("error.not_found", "That item no longer exists.", false),
            ErrorCode::Conflict => (
                "error.conflict",
                "That conflicts with something already in progress.",
                false,
            ),
            ErrorCode::StorageBusy => (
                "error.storage_busy",
                "The library database is busy. Try again in a moment.",
                true,
            ),
            ErrorCode::StorageCorrupt => (
                "error.storage_corrupt",
                "The library database is damaged and has to be rebuilt.",
                false,
            ),
            ErrorCode::PermissionDenied => (
                "error.permission_denied",
                "Peregrine does not have permission to do that.",
                false,
            ),
            ErrorCode::DiskFull => (
                "error.disk_full",
                "There is not enough free space. Free some space and resume.",
                false,
            ),
            ErrorCode::Network => (
                "error.network",
                "The network is unavailable. Cached content still works.",
                true,
            ),
            ErrorCode::ProviderRateLimited => (
                "error.provider_rate_limited",
                "The provider is rate limiting us. This will retry automatically.",
                true,
            ),
            ErrorCode::ProviderUnavailable => (
                "error.provider_unavailable",
                "The provider is unavailable. This will retry automatically.",
                true,
            ),
            ErrorCode::ProviderSchemaDrift => (
                "error.provider_schema_drift",
                "The provider changed its response format; showing cached data.",
                false,
            ),
            ErrorCode::PlaybackFailed => (
                "error.playback_failed",
                "Playback stopped unexpectedly.",
                true,
            ),
            ErrorCode::DeviceUnavailable => (
                "error.device_unavailable",
                "The device is no longer reachable.",
                true,
            ),
            ErrorCode::Cancelled => ("error.cancelled", "Cancelled.", false),
            ErrorCode::Unsupported => (
                "error.unsupported",
                "That is not supported on this setup.",
                false,
            ),
            ErrorCode::Internal => (
                "error.internal",
                "Something went wrong inside Peregrine.",
                false,
            ),
        };
        CodeMetadata {
            code: self,
            message_key,
            message,
            retryable,
        }
    }
}

/// The error payload for every command, and the only shape the UI has to handle.
///
/// `detail` and `cause` are constrained by ARCHITECTURE §7 rule 3: neither may
/// contain a secret or an absolute home path. Callers pass values that are
/// already safe to show, because deciding what is safe is a property of the
/// source error, not of this type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppError {
    /// The stable machine-readable code.
    pub code: ErrorCode,
    /// English fallback text, from the code's metadata.
    pub message: String,
    /// Extra context for the user, already redacted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Whether retrying the same operation, unchanged, could succeed.
    pub retryable: bool,
    /// Where it came from, already redacted. For logs and diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
}

impl AppError {
    /// Builds an error from a code, taking the message and retryability from the
    /// code's metadata.
    #[must_use]
    pub fn new(code: ErrorCode) -> Self {
        let meta = code.metadata();
        Self {
            code,
            message: meta.message.to_owned(),
            detail: None,
            retryable: meta.retryable,
            cause: None,
        }
    }

    /// Attaches user-facing context. `detail` must already be safe to display.
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Attaches the originating condition, for logs and diagnostics.
    #[must_use]
    pub fn with_cause(mut self, cause: impl Into<String>) -> Self {
        self.cause = Some(cause.into());
        self
    }

    /// The catalogue key for this error's message.
    #[must_use]
    pub const fn message_key(&self) -> &'static str {
        self.code.metadata().message_key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical string form is read by TypeScript and keys the message
    /// catalogue, so these values are a contract, not an implementation detail.
    #[test]
    fn serialized_forms_are_stable() {
        let expected = [
            (ErrorCode::InvalidInput, "\"INVALID_INPUT\""),
            (ErrorCode::NotFound, "\"NOT_FOUND\""),
            (ErrorCode::StorageBusy, "\"STORAGE_BUSY\""),
            (ErrorCode::StorageCorrupt, "\"STORAGE_CORRUPT\""),
            (ErrorCode::ProviderRateLimited, "\"PROVIDER_RATE_LIMITED\""),
            (ErrorCode::ProviderSchemaDrift, "\"PROVIDER_SCHEMA_DRIFT\""),
            (ErrorCode::DeviceUnavailable, "\"DEVICE_UNAVAILABLE\""),
            (ErrorCode::Internal, "\"INTERNAL\""),
        ];
        for (code, json) in expected {
            assert_eq!(
                serde_json::to_string(&code).unwrap(),
                json,
                "code {code:?} renamed"
            );
        }
    }

    /// The acceptance criterion for PG-E2-02: no code may exist without both a
    /// catalogue key and a retryability decision.
    #[test]
    fn every_code_has_a_message_key_and_a_retryability_flag() {
        assert!(!ErrorCode::ALL.is_empty());
        for code in ErrorCode::ALL {
            let meta = code.metadata();
            assert_eq!(meta.code, *code, "metadata returned the wrong code");
            assert!(!meta.message.is_empty(), "{code:?} has no fallback message");
            assert!(
                meta.message_key.starts_with("error."),
                "{code:?} key {:?} is outside the `error.` namespace",
                meta.message_key
            );
            assert!(
                meta.message_key
                    .trim_start_matches("error.")
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_'),
                "{code:?} key {:?} is not snake_case",
                meta.message_key
            );
        }
    }

    #[test]
    fn message_keys_are_unique() {
        let mut keys: Vec<&str> = ErrorCode::ALL
            .iter()
            .map(|code| code.metadata().message_key)
            .collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(before, keys.len(), "two codes share a message key");
    }

    #[test]
    fn all_lists_every_code_exactly_once() {
        let mut seen = ErrorCode::ALL.to_vec();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(before, seen.len(), "ErrorCode::ALL contains a duplicate");
        // `ALL` is hand-maintained, so this checks it is not self-contradictory
        // and that every entry survives the wire format the UI reads. The
        // guarantee that a *new* variant cannot be added without a message key
        // and a retryability flag comes from the exhaustive `match` in
        // `metadata()`, which will not compile until both are supplied. A variant
        // left out of `ALL` would not be caught here.
        for code in ErrorCode::ALL {
            let json = serde_json::to_string(code).unwrap();
            let back: ErrorCode = serde_json::from_str(&json).unwrap();
            assert_eq!(back, *code);
            assert!(ErrorCode::ALL.contains(&back));
        }
    }

    /// Retryability is policy, so it is pinned per code rather than left to the
    /// reader. A change here is a change in what the UI promises the user.
    #[test]
    fn retryability_follows_the_documented_condition() {
        let retryable = [
            ErrorCode::StorageBusy,
            ErrorCode::Network,
            ErrorCode::ProviderRateLimited,
            ErrorCode::ProviderUnavailable,
            ErrorCode::PlaybackFailed,
            ErrorCode::DeviceUnavailable,
        ];
        let not_retryable = [
            ErrorCode::InvalidInput,
            ErrorCode::NotFound,
            ErrorCode::Conflict,
            ErrorCode::StorageCorrupt,
            ErrorCode::PermissionDenied,
            ErrorCode::DiskFull,
            ErrorCode::ProviderSchemaDrift,
            ErrorCode::Cancelled,
            ErrorCode::Unsupported,
            ErrorCode::Internal,
        ];
        for code in retryable {
            assert!(code.metadata().retryable, "{code:?} should be retryable");
        }
        for code in not_retryable {
            assert!(
                !code.metadata().retryable,
                "{code:?} should not be retryable"
            );
        }
        assert_eq!(
            retryable.len() + not_retryable.len(),
            ErrorCode::ALL.len(),
            "a code is missing from the retryability policy test"
        );
    }

    #[test]
    fn new_takes_message_and_retryability_from_the_code() {
        let error = AppError::new(ErrorCode::Network);
        assert_eq!(error.message, ErrorCode::Network.metadata().message);
        assert!(error.retryable);
        assert_eq!(error.message_key(), "error.network");
        assert_eq!(error.detail, None);
        assert_eq!(error.cause, None);
    }

    /// The UI opts into retry from this flag alone, so it must be impossible for
    /// a caller to set it inconsistently with the code.
    #[test]
    fn constructing_with_a_code_cannot_contradict_retryability() {
        for code in ErrorCode::ALL {
            assert_eq!(AppError::new(*code).retryable, code.metadata().retryable);
        }
    }

    #[test]
    fn absent_optionals_are_omitted_from_the_wire_format() {
        let json = serde_json::to_string(&AppError::new(ErrorCode::Cancelled)).unwrap();
        assert!(!json.contains("detail"), "{json}");
        assert!(!json.contains("cause"), "{json}");
        assert!(json.contains("\"retryable\":false"), "{json}");
    }

    #[test]
    fn round_trip_preserves_detail_and_cause() {
        let original = AppError::new(ErrorCode::DiskFull)
            .with_detail("need 4.2 GB more")
            .with_cause("download job 01J8");
        let json = serde_json::to_string(&original).unwrap();
        let back: AppError = serde_json::from_str(&json).unwrap();
        assert_eq!(back, original);
        assert_eq!(back.detail.as_deref(), Some("need 4.2 GB more"));
        assert_eq!(back.cause.as_deref(), Some("download job 01J8"));
    }

    #[test]
    fn wire_shape_matches_the_documented_contract() {
        let json = serde_json::to_value(AppError::new(ErrorCode::Network)).unwrap();
        let object = json.as_object().unwrap();
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["code", "message", "retryable"]);
    }
}
