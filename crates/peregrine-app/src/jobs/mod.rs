//! The job-scoped work the runtime owns: the payload shapes its events carry, and the
//! jobs themselves.
//!
//! The payload builders live here rather than inside a job because two crates have to
//! agree on them: the job publishes the value, and `peregrine-ipc` turns it into the DTO
//! the UI reads. `peregrine-ipc`'s test deserializes values built here into its own types,
//! so a change to one that is not made to the other fails the build rather than the UI.

pub mod hello;

use peregrine_storage::JobPhase;
use serde_json::{Value, json};
use ulid::Ulid;

/// The stable error code a cancelled job records.
///
/// It is the string form of `peregrine-ipc`'s `ErrorCode::Cancelled`; that crate's test
/// asserts the two agree, so the journal and the UI cannot name the same outcome twice.
pub const CANCELLED_CODE: &str = "CANCELLED";

/// The stable error code a job records when it could not be started at all.
///
/// The string form of `peregrine-ipc`'s `ErrorCode::Internal`, checked by that crate's
/// test in the same way as [`CANCELLED_CODE`].
pub const INTERNAL_CODE: &str = "INTERNAL";

/// The body of a `job://state` event.
#[must_use]
pub fn state(job: Ulid, journal_id: i64, kind: &str, phase: JobPhase, attempt: u32) -> Value {
    json!({
        "jobId": job.to_string(),
        "journalId": journal_id,
        "kind": kind,
        "phase": phase.as_str(),
        "attempt": attempt,
    })
}

/// The body of a `job://progress` event.
#[must_use]
pub fn progress(job: Ulid, journal_id: i64, permille: u16, attempt: u32) -> Value {
    json!({
        "jobId": job.to_string(),
        "journalId": journal_id,
        "progressPermille": permille,
        "attempt": attempt,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The keys are the UI's contract, so a rename is a deliberate edit here.
    #[test]
    fn payload_keys_are_pinned() {
        let job = Ulid::from_parts(1_700_000_000_000, 7);
        let state = state(job, 3, "hello", JobPhase::Running, 1);
        let mut keys: Vec<&str> = state
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["attempt", "jobId", "journalId", "kind", "phase"],
            "the `job://state` shape changed"
        );
        assert_eq!(state["phase"], "running");
        assert_eq!(state["jobId"], job.to_string());

        let progress = progress(job, 3, 640, 1);
        let mut keys: Vec<&str> = progress
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["attempt", "jobId", "journalId", "progressPermille"],
            "the `job://progress` shape changed"
        );
        assert_eq!(progress["progressPermille"], 640);
    }

    /// Every phase must serialize to the spelling the schema and the UI share.
    #[test]
    fn every_phase_has_a_wire_spelling() {
        let job = Ulid::from_parts(1_700_000_000_000, 7);
        for phase in [
            JobPhase::Queued,
            JobPhase::Running,
            JobPhase::Deferred,
            JobPhase::Succeeded,
            JobPhase::Failed,
            JobPhase::Canceled,
        ] {
            assert_eq!(state(job, 1, "hello", phase, 0)["phase"], phase.as_str());
        }
    }
}
