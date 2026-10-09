//! The scheduler's typed errors.
//!
//! No variant carries the engine's internals: a caller can name a cancellation, a
//! retryable failure, a permanent failure, and a stopped scheduler, and nothing else.

use std::error::Error as StdError;

/// Why a job did not succeed.
///
/// Every variant carries a message that is safe to log; job code is responsible for
/// keeping secrets out of it (ARCHITECTURE §12).
#[derive(Debug, Clone, thiserror::Error)]
pub enum JobError {
    /// The job observed its cancellation token and stopped cooperatively.
    #[error("the job was cancelled")]
    Cancelled,

    /// The job failed in a way that is worth retrying, so it is rescheduled with backoff.
    #[error("{message}")]
    Retryable {
        /// A redacted description of the failure.
        message: String,
        /// A caller-chosen delay, used instead of the configured backoff curve.
        ///
        /// A provider that answers `429` with `Retry-After` names the delay; the
        /// scheduler otherwise computes one from the attempt count.
        retry_after: Option<std::time::Duration>,
    },

    /// The job failed permanently and has no useful retry.
    #[error("{message}")]
    Failed {
        /// A redacted description of the failure.
        message: String,
    },
}

impl JobError {
    /// Builds a retryable failure with the configured backoff.
    pub fn retryable(message: impl Into<String>) -> Self {
        Self::Retryable {
            message: message.into(),
            retry_after: None,
        }
    }

    /// Builds a retryable failure that overrides the backoff with `retry_after`.
    pub fn retry_after(message: impl Into<String>, retry_after: std::time::Duration) -> Self {
        Self::Retryable {
            message: message.into(),
            retry_after: Some(retry_after),
        }
    }

    /// Builds a permanent failure.
    pub fn failed(message: impl Into<String>) -> Self {
        Self::Failed {
            message: message.into(),
        }
    }
}

/// Why the scheduler refused to accept a job.
#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    /// The scheduler's own task is no longer running.
    #[error("the scheduler has stopped; build a new one before submitting more jobs")]
    Stopped,

    /// The engine could not run a submitted job.
    ///
    /// The payload is boxed so that a caller's notification type stays out of this
    /// crate's error surface, exactly as the storage crate boxes its driver errors.
    #[error("{message}")]
    Engine {
        /// A redacted description of the failure.
        message: String,
        /// The failure the engine reported.
        source: Box<dyn StdError + Send + Sync>,
    },
}
