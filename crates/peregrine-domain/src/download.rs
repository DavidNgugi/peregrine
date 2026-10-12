//! The download job state machine: the ten states `download_job.state` stores and the one
//! guarded transition between them.
//!
//! Everything that moves a job goes through [`transition`], so the rules that FLOWS SM1,
//! `DATA_MODEL`, and ARCHITECTURE §15 state but no `CHECK` can express are enforced in one place:
//!
//! - **I4** — a non-terminal job does not advance past `inspecting` without a persisted safety
//!   report. The two events that cross the gate carry the [`SafetyReport`] itself: the value
//!   [`TorrentInspector`](crate::inspect::TorrentInspector) produces and
//!   `download_job.safety_report_json` stores. There is no other spelling of those events, so no
//!   caller can reach `metadata` by writing a state down.
//! - **A refusal with no override and a refusal the user may take anyway go different ways.**
//!   Both park the job in `blocked`, because the schema has one blocked state and the report is
//!   persisted so the decision is still explainable after a restart — but only an
//!   [`Verdict::OptInRequired`] torrent can be re-inspected into an [`Verdict::Allowed`] report,
//!   so [`DownloadEvent::Consented`] lifts one and is refused for the other, with the reason
//!   naming which.
//! - **Terminal states absorb.** Nothing leaves `completed`, `failed`, or `removed`: a
//!   completion that arrives after a removal must not resurrect the job, and a retry after a
//!   failure is a new job rather than an edge (see [`DownloadState::Failed`]).
//! - **A full disk pauses rather than fails** (ARCHITECTURE §15), and the pause is reversible.
//!
//! Rehydration is the one place a state is written down rather than reached:
//! [`DownloadState::parse`] reads a value the database already stored, which is not an advance.
//! A repository that writes a job's `state` must have obtained it from [`transition`] first.

use std::fmt;

use thiserror::Error;

use crate::inspect::{SafetyReport, Verdict};
use crate::stored_enum;

stored_enum! {
    /// A job's state, as `download_job.state` stores it.
    ///
    /// The variants are in the `CHECK` constraint's order and a test asserts that order against
    /// `DATA_MODEL` §2.7. Each doc names the edges FLOWS SM1 draws out of the state, so a state
    /// with no edge, or an edge into a state that cannot hold it, is visible in review and not
    /// only in a failing test.
    DownloadState {
        /// Accepted into the queue, waiting for room in the active set.
        ///
        /// A job that cannot start stays here rather than failing: the free-space floor is part
        /// of admission (ARCHITECTURE §10), so a full disk blocks a new download instead of
        /// failing one.
        Queued => "queued",
        /// Metadata is being fetched and the safety gate is running.
        ///
        /// No byte is written in this state (FLOWS F6). Together with [`Self::Queued`], it is
        /// one of the two states a job may be in while `safety_report_json` is still `NULL`, and
        /// I4 is the rule that a job does not leave here without one.
        Inspecting => "inspecting",
        /// The gate allowed the torrent and it is being handed to the engine.
        ///
        /// Reaching this state requires a report in hand (I4): see [`transition`].
        Metadata => "metadata",
        /// The transfer is running.
        Downloading => "downloading",
        /// Stopped deliberately and resumable: by the user, by the bandwidth policy, or by a
        /// full disk, which pauses rather than fails (ARCHITECTURE §15).
        Paused => "paused",
        /// The payload is complete and the job is uploading it back.
        Seeding => "seeding",
        /// Terminal: the payload was imported into the library (FLOWS F6).
        Completed => "completed",
        /// Terminal: an unrecoverable engine error, or no metadata or peers within the timeout.
        ///
        /// SM1 draws `failed --> queued: retry with a different release`. That retry is not an
        /// edge here: a different release is a different info hash, and `download_job_hash_uq`
        /// covers only live states because a terminal job is history (`DATA_MODEL` §2.7), so the
        /// retry inserts a new `download_job` row. Keeping `failed` absorbing is also what stops
        /// a late completion from being mistaken for progress on a job that is over.
        Failed => "failed",
        /// The safety gate refused the torrent, with the report persisted so the refusal is
        /// explainable after a restart (FLOWS F6).
        ///
        /// This is where a rejected job waits for the user, and it is not an end: `Consented`
        /// lifts an opt-in refusal, and SM1's `blocked --> removed` is the user dismissing a row
        /// they do not intend to override. A finding either blocks, requires the advanced opt-in,
        /// or does not exist, so there is no warn level that could park a job here.
        Blocked => "blocked",
        /// Terminal: the user removed the transfer, or dismissed a block.
        Removed => "removed",
    }
}

impl DownloadState {
    /// Whether nothing may leave this state at all.
    ///
    /// FLOWS SM1 names these three: "Terminal states: `completed`, `removed`, `failed` (after
    /// retry exhaustion)". `blocked` is not one of them even though it is not live in the
    /// engine: SM1 gives it a dismissal edge, and a job whose report can still be overridden is
    /// waiting for the user rather than finished.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Removed)
    }
}

/// Something that happened to a job, which may or may not move it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadEvent {
    /// Room appeared in the active set (SM1's `queued --> inspecting`).
    Admitted,
    /// The gate produced a report from its first pass, with the opt-in not granted.
    ///
    /// This is the only event that carries a job out of `inspecting` and into `metadata`
    /// (through [`Verdict::Allowed`]), and the report cannot be built without running
    /// [`TorrentInspector::inspect`](crate::inspect::TorrentInspector::inspect) or reading the
    /// persisted `safety_report_json`. That is I4 as a type rather than as a rule a caller has
    /// to remember.
    Inspected(SafetyReport),
    /// The user took the advanced opt-in, and the gate was run again with `OptIn::Granted`.
    ///
    /// The verdict decides the outcome, and it is not the user's to assert: an opt-in refusal
    /// re-inspects to [`Verdict::Allowed`] and lifts the block, while a traversal, a link, or a
    /// program re-inspects to [`Verdict::Blocked`] however many times the user agrees, because
    /// `inspect` returns `Allowed` only when every finding is one the opt-in covers. A caller
    /// that has not re-inspected passes the old report and is told so.
    Consented(SafetyReport),
    /// The engine accepted the torrent and the file selection (SM1's `metadata -->
    /// downloading`).
    EngineAccepted,
    /// No metadata, or no peers, within the deadline (SM1's `metadata --> failed`).
    MetadataTimedOut,
    /// The user, or the bandwidth policy, stopped a transfer (SM1's `downloading --> paused`
    /// and `seeding --> paused`).
    Paused,
    /// The user resumed a stopped transfer (SM1's `paused --> downloading`).
    Resumed,
    /// The payload is complete (SM1's `downloading --> seeding`).
    PayloadComplete,
    /// The seeding policy is satisfied (SM1's `seeding --> completed`).
    SeedingSatisfied,
    /// An unrecoverable engine error (SM1's `downloading --> failed`).
    EngineFailed,
    /// Free space fell below the floor (ARCHITECTURE §15).
    ///
    /// A level rather than an edge: it pauses a transfer and changes nothing for a job that is
    /// not writing bytes, so reporting the same condition twice is not an error.
    DiskFull,
    /// The user removed a transfer, or dismissed a block (SM1's three `--> removed` edges).
    ///
    /// Removal and dismissal are one event because they reach one state and write one
    /// `removed_at`; SM1 draws them separately only because the affordance differs.
    Removed,
    /// A statistics tick. It carries no state, and it is the event most likely to arrive late.
    Progress,
}

impl DownloadEvent {
    /// The event's name, without its report.
    ///
    /// A rejection names the event it refused, and a report's findings carry entry paths copied
    /// from a torrent somebody else wrote; an error string or a log line is not the place to
    /// repeat them.
    fn name(&self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::Inspected(_) => "inspected",
            Self::Consented(_) => "consented",
            Self::EngineAccepted => "engine accepted",
            Self::MetadataTimedOut => "metadata timed out",
            Self::Paused => "paused",
            Self::Resumed => "resumed",
            Self::PayloadComplete => "payload complete",
            Self::SeedingSatisfied => "seeding satisfied",
            Self::EngineFailed => "engine failed",
            Self::DiskFull => "disk full",
            Self::Removed => "removed",
            Self::Progress => "progress",
        }
    }
}

impl fmt::Display for DownloadEvent {
    /// The same name [`DownloadEvent::name`] gives, so an event can be interpolated into a
    /// message without its report coming with it.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// Why an event did not move a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TransitionRejected {
    /// Nothing leaves a terminal state (FLOWS SM1). A completion that arrives after a removal
    /// is refused here rather than allowed to resurrect the job.
    #[error("{state:?} is terminal, so {event} cannot leave it")]
    Terminal {
        /// The state the event arrived in.
        state: DownloadState,
        /// The refused event's name.
        event: &'static str,
    },
    /// The event is legal somewhere in the machine but not from this state: a duplicate engine
    /// acceptance, a second pause, a completion that arrives while the transfer is stopped.
    #[error("{event} is out of order in {state:?}")]
    OutOfOrder {
        /// The state the event arrived in.
        state: DownloadState,
        /// The refused event's name.
        event: &'static str,
    },
    /// The report the consent carried still needs the advanced opt-in, so the user's answer did
    /// not reach an inspection: run the gate again with `OptIn::Granted` and consent with the
    /// report that produces.
    #[error(
        "{event} carries a report that still needs the advanced opt-in, so it cannot lift a block in {state:?}"
    )]
    OptInNotGranted {
        /// The state the consent arrived in.
        state: DownloadState,
        /// The refused event's name.
        event: &'static str,
    },
    /// The report refused this torrent with no override — traversal, a link, a program, or an
    /// extension nobody classified (SECURITY T1 to T3) — so no consent can lift the block.
    #[error(
        "{event} carries a report that refused this torrent with no override, so it cannot lift a block in {state:?}"
    )]
    NoOverride {
        /// The state the consent arrived in.
        state: DownloadState,
        /// The refused event's name.
        event: &'static str,
    },
}

/// Applies one event to one state, or explains why the event cannot move it.
///
/// This is the only function that computes a next state, which is what makes the rules above
/// structural rather than advisory: a job cannot reach `metadata` without an
/// [`DownloadEvent::Inspected`] or [`DownloadEvent::Consented`], and neither can be built
/// without a [`SafetyReport`] (I4).
///
/// Each state's legal exits are in one arm, so the machine reads as the ten states it has rather
/// than as a table of pairs. Three groups of cells are deliberate rather than incidental:
///
/// - A [`DownloadEvent::DiskFull`] returns the state unchanged everywhere except `downloading`,
///   because a full disk is a level: the condition is still true, and a caller reporting it
///   again is not in error.
/// - [`DownloadEvent::Progress`] carries no state, so it returns the state it was given from any
///   non-terminal state. A tick that arrives after the job has finished is a different matter:
///   the job is terminal and the tick is refused, which tells the caller its coalescing is
///   behind rather than hiding the fact behind an `Ok`.
/// - [`DownloadEvent::Consented`] is legal only in `blocked`. A consent answers a refusal, and a
///   job the gate allowed never waited for one.
///
/// # Errors
///
/// Returns [`TransitionRejected`] when the event is not legal from the state it arrived in.
pub fn transition(
    state: DownloadState,
    event: DownloadEvent,
) -> Result<DownloadState, TransitionRejected> {
    // The name is read before the event is matched, so every rejection can name what it refused
    // without having to move a report back out of the pattern that consumed it.
    let name = event.name();
    match state {
        // The absorbing rule is one arm, so it holds for every event, including one that is
        // legal in the state the job was in before it finished.
        DownloadState::Completed | DownloadState::Failed | DownloadState::Removed => {
            Err(TransitionRejected::Terminal { state, event: name })
        }
        DownloadState::Queued => match event {
            DownloadEvent::Admitted => Ok(DownloadState::Inspecting),
            DownloadEvent::DiskFull | DownloadEvent::Progress => Ok(state),
            _ => Err(TransitionRejected::OutOfOrder { state, event: name }),
        },
        DownloadState::Inspecting => match event {
            // The first pass: `Allowed` is the one verdict that advances without asking, and both
            // refusals park the job where an override can be offered (SECURITY T1/T2 versus T4).
            DownloadEvent::Inspected(report) => Ok(match report.verdict() {
                Verdict::Allowed => DownloadState::Metadata,
                Verdict::OptInRequired | Verdict::Blocked => DownloadState::Blocked,
            }),
            DownloadEvent::DiskFull | DownloadEvent::Progress => Ok(state),
            _ => Err(TransitionRejected::OutOfOrder { state, event: name }),
        },
        DownloadState::Blocked => match event {
            // The user's answer, carrying the report the re-inspection with the opt-in granted
            // produced. Only `Allowed` lifts the block, and a hard refusal cannot produce one.
            DownloadEvent::Consented(report) => match report.verdict() {
                Verdict::Allowed => Ok(DownloadState::Metadata),
                Verdict::OptInRequired => {
                    Err(TransitionRejected::OptInNotGranted { state, event: name })
                }
                Verdict::Blocked => Err(TransitionRejected::NoOverride { state, event: name }),
            },
            DownloadEvent::Removed => Ok(DownloadState::Removed),
            DownloadEvent::DiskFull | DownloadEvent::Progress => Ok(state),
            _ => Err(TransitionRejected::OutOfOrder { state, event: name }),
        },
        DownloadState::Metadata => match event {
            DownloadEvent::EngineAccepted => Ok(DownloadState::Downloading),
            DownloadEvent::MetadataTimedOut => Ok(DownloadState::Failed),
            DownloadEvent::DiskFull | DownloadEvent::Progress => Ok(state),
            _ => Err(TransitionRejected::OutOfOrder { state, event: name }),
        },
        DownloadState::Downloading => match event {
            // A full disk joins the two deliberate stops, because ARCHITECTURE §15 pauses a
            // transfer rather than failing it.
            DownloadEvent::Paused | DownloadEvent::DiskFull => Ok(DownloadState::Paused),
            DownloadEvent::PayloadComplete => Ok(DownloadState::Seeding),
            DownloadEvent::EngineFailed => Ok(DownloadState::Failed),
            DownloadEvent::Removed => Ok(DownloadState::Removed),
            DownloadEvent::Progress => Ok(state),
            _ => Err(TransitionRejected::OutOfOrder { state, event: name }),
        },
        DownloadState::Paused => match event {
            DownloadEvent::Resumed => Ok(DownloadState::Downloading),
            DownloadEvent::Removed => Ok(DownloadState::Removed),
            DownloadEvent::DiskFull | DownloadEvent::Progress => Ok(state),
            _ => Err(TransitionRejected::OutOfOrder { state, event: name }),
        },
        DownloadState::Seeding => match event {
            DownloadEvent::Paused => Ok(DownloadState::Paused),
            DownloadEvent::SeedingSatisfied => Ok(DownloadState::Completed),
            DownloadEvent::DiskFull | DownloadEvent::Progress => Ok(state),
            _ => Err(TransitionRejected::OutOfOrder { state, event: name }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inspect::{
        EntryKind, InspectionLimits, OptIn, TorrentDescription, TorrentEntry, TorrentInspector,
    };
    use crate::path::PathGuard;
    use proptest::prelude::*;

    /// The root every fixture path is judged against.
    fn guard() -> PathGuard {
        PathGuard::new("/downloads").expect("the fixture root is absolute")
    }

    /// Caps wide enough that only the rule under test can fire.
    fn inspector() -> TorrentInspector {
        TorrentInspector::new(InspectionLimits::new(1_000_000, u64::MAX))
    }

    /// Runs the real inspector over one entry, so every report here is one the production code
    /// could have produced.
    fn report(path: &str, opt_in: OptIn) -> SafetyReport {
        let description =
            TorrentDescription::new(vec![TorrentEntry::new(path, 1, EntryKind::File)]);
        inspector().inspect(&description, &guard(), opt_in)
    }

    /// A torrent the gate allows outright.
    fn allowed() -> SafetyReport {
        report("Movie.2024.1080p.mkv", OptIn::NotGranted)
    }

    /// A nested archive: refused until the user takes the advanced opt-in (SECURITY T4).
    fn opt_in_required() -> SafetyReport {
        report("extras.zip", OptIn::NotGranted)
    }

    /// Traversal: refused with no override (SECURITY T1).
    fn blocked() -> SafetyReport {
        report("../authorized_keys", OptIn::NotGranted)
    }

    /// The report granting the advanced opt-in for the archive above produces.
    fn granted() -> SafetyReport {
        report("extras.zip", OptIn::Granted)
    }

    /// The report a verdict implies, from the inspector rather than assembled by hand.
    fn carried(verdict: Verdict) -> SafetyReport {
        match verdict {
            Verdict::Allowed => allowed(),
            Verdict::OptInRequired => opt_in_required(),
            Verdict::Blocked => blocked(),
        }
    }

    /// What an event is without its report: `Copy`, so the matrix can be a `const`, and separate
    /// from [`DownloadEvent`], so a report is only built when a cell is exercised.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Kind {
        /// [`DownloadEvent::Admitted`].
        Admitted,
        /// [`DownloadEvent::Inspected`] with a report carrying the verdict.
        Inspected(Verdict),
        /// [`DownloadEvent::Consented`] with a report carrying the verdict.
        Consented(Verdict),
        /// [`DownloadEvent::EngineAccepted`].
        EngineAccepted,
        /// [`DownloadEvent::MetadataTimedOut`].
        MetadataTimedOut,
        /// [`DownloadEvent::Paused`].
        Paused,
        /// [`DownloadEvent::Resumed`].
        Resumed,
        /// [`DownloadEvent::PayloadComplete`].
        PayloadComplete,
        /// [`DownloadEvent::SeedingSatisfied`].
        SeedingSatisfied,
        /// [`DownloadEvent::EngineFailed`].
        EngineFailed,
        /// [`DownloadEvent::DiskFull`].
        DiskFull,
        /// [`DownloadEvent::Removed`].
        Removed,
        /// [`DownloadEvent::Progress`].
        Progress,
    }

    impl Kind {
        /// The real event, carrying the report its verdict implies.
        fn event(self) -> DownloadEvent {
            match self {
                Self::Admitted => DownloadEvent::Admitted,
                Self::Inspected(verdict) => DownloadEvent::Inspected(carried(verdict)),
                Self::Consented(verdict) => DownloadEvent::Consented(carried(verdict)),
                Self::EngineAccepted => DownloadEvent::EngineAccepted,
                Self::MetadataTimedOut => DownloadEvent::MetadataTimedOut,
                Self::Paused => DownloadEvent::Paused,
                Self::Resumed => DownloadEvent::Resumed,
                Self::PayloadComplete => DownloadEvent::PayloadComplete,
                Self::SeedingSatisfied => DownloadEvent::SeedingSatisfied,
                Self::EngineFailed => DownloadEvent::EngineFailed,
                Self::DiskFull => DownloadEvent::DiskFull,
                Self::Removed => DownloadEvent::Removed,
                Self::Progress => DownloadEvent::Progress,
            }
        }
    }

    /// Every event kind a caller can produce, including each verdict a report can carry.
    const KINDS: &[Kind] = &[
        Kind::Admitted,
        Kind::Inspected(Verdict::Allowed),
        Kind::Inspected(Verdict::OptInRequired),
        Kind::Inspected(Verdict::Blocked),
        Kind::Consented(Verdict::Allowed),
        Kind::Consented(Verdict::OptInRequired),
        Kind::Consented(Verdict::Blocked),
        Kind::EngineAccepted,
        Kind::MetadataTimedOut,
        Kind::Paused,
        Kind::Resumed,
        Kind::PayloadComplete,
        Kind::SeedingSatisfied,
        Kind::EngineFailed,
        Kind::DiskFull,
        Kind::Removed,
        Kind::Progress,
    ];

    /// The legal cells of the state × event matrix: `(from, event, to)`. Anything absent is
    /// illegal, which is the claim the matrix test checks against [`transition`] both ways.
    const LEGAL: &[(DownloadState, Kind, DownloadState)] = &[
        (
            DownloadState::Queued,
            Kind::Admitted,
            DownloadState::Inspecting,
        ),
        (DownloadState::Queued, Kind::DiskFull, DownloadState::Queued),
        (DownloadState::Queued, Kind::Progress, DownloadState::Queued),
        (
            DownloadState::Inspecting,
            Kind::Inspected(Verdict::Allowed),
            DownloadState::Metadata,
        ),
        (
            DownloadState::Inspecting,
            Kind::Inspected(Verdict::OptInRequired),
            DownloadState::Blocked,
        ),
        (
            DownloadState::Inspecting,
            Kind::Inspected(Verdict::Blocked),
            DownloadState::Blocked,
        ),
        (
            DownloadState::Inspecting,
            Kind::DiskFull,
            DownloadState::Inspecting,
        ),
        (
            DownloadState::Inspecting,
            Kind::Progress,
            DownloadState::Inspecting,
        ),
        (
            DownloadState::Metadata,
            Kind::EngineAccepted,
            DownloadState::Downloading,
        ),
        (
            DownloadState::Metadata,
            Kind::MetadataTimedOut,
            DownloadState::Failed,
        ),
        (
            DownloadState::Metadata,
            Kind::DiskFull,
            DownloadState::Metadata,
        ),
        (
            DownloadState::Metadata,
            Kind::Progress,
            DownloadState::Metadata,
        ),
        (
            DownloadState::Downloading,
            Kind::Paused,
            DownloadState::Paused,
        ),
        (
            DownloadState::Downloading,
            Kind::DiskFull,
            DownloadState::Paused,
        ),
        (
            DownloadState::Downloading,
            Kind::PayloadComplete,
            DownloadState::Seeding,
        ),
        (
            DownloadState::Downloading,
            Kind::EngineFailed,
            DownloadState::Failed,
        ),
        (
            DownloadState::Downloading,
            Kind::Removed,
            DownloadState::Removed,
        ),
        (
            DownloadState::Downloading,
            Kind::Progress,
            DownloadState::Downloading,
        ),
        (
            DownloadState::Paused,
            Kind::Resumed,
            DownloadState::Downloading,
        ),
        (DownloadState::Paused, Kind::Removed, DownloadState::Removed),
        (DownloadState::Paused, Kind::DiskFull, DownloadState::Paused),
        (DownloadState::Paused, Kind::Progress, DownloadState::Paused),
        (DownloadState::Seeding, Kind::Paused, DownloadState::Paused),
        (
            DownloadState::Seeding,
            Kind::SeedingSatisfied,
            DownloadState::Completed,
        ),
        (
            DownloadState::Seeding,
            Kind::DiskFull,
            DownloadState::Seeding,
        ),
        (
            DownloadState::Seeding,
            Kind::Progress,
            DownloadState::Seeding,
        ),
        (
            DownloadState::Blocked,
            Kind::Consented(Verdict::Allowed),
            DownloadState::Metadata,
        ),
        (
            DownloadState::Blocked,
            Kind::Removed,
            DownloadState::Removed,
        ),
        (
            DownloadState::Blocked,
            Kind::DiskFull,
            DownloadState::Blocked,
        ),
        (
            DownloadState::Blocked,
            Kind::Progress,
            DownloadState::Blocked,
        ),
    ];

    /// The matrix's own answer for one cell, or `None` when the cell is illegal.
    fn declared(state: DownloadState, kind: Kind) -> Option<DownloadState> {
        LEGAL
            .iter()
            .find(|(from, candidate, _)| *from == state && *candidate == kind)
            .map(|(_, _, to)| *to)
    }

    fn terminal() -> Vec<DownloadState> {
        DownloadState::ALL
            .iter()
            .copied()
            .filter(|state| state.is_terminal())
            .collect()
    }

    fn state_strategy() -> impl Strategy<Value = DownloadState> {
        prop::sample::select(DownloadState::ALL)
    }

    fn kind_strategy() -> impl Strategy<Value = Kind> {
        prop::sample::select(KINDS)
    }

    /// Whether the event was refused for the absorbing reason, as a function so that
    /// `prop_assert!` can name it without a struct pattern in its format string.
    fn is_terminal_rejection(state: DownloadState, kind: Kind) -> bool {
        matches!(
            transition(state, kind.event()),
            Err(TransitionRejected::Terminal { .. })
        )
    }

    #[test]
    fn every_state_matches_the_check_constraint_it_mirrors() {
        assert_eq!(
            DownloadState::ALL
                .iter()
                .map(|state| state.as_str())
                .collect::<Vec<_>>(),
            [
                "queued",
                "inspecting",
                "metadata",
                "downloading",
                "paused",
                "seeding",
                "completed",
                "failed",
                "blocked",
                "removed",
            ],
            "download_job.state does not match its CHECK constraint"
        );
    }

    #[test]
    fn every_state_round_trips_through_its_stored_text_and_json() {
        for state in DownloadState::ALL {
            assert_eq!(DownloadState::parse(state.as_str()), Some(*state));
            let json = serde_json::to_string(state).expect("serializes");
            assert_eq!(json, format!("\"{}\"", state.as_str()));
            assert_eq!(
                serde_json::from_str::<DownloadState>(&json).expect("deserializes"),
                *state
            );
        }
        assert_eq!(DownloadState::parse("complete"), None);
        assert_eq!(DownloadState::parse("inspecting "), None);
        assert!(serde_json::from_str::<DownloadState>("\"cancelled\"").is_err());
    }

    #[test]
    fn terminal_states_are_exactly_the_ones_flows_names() {
        assert_eq!(
            terminal(),
            vec![
                DownloadState::Completed,
                DownloadState::Failed,
                DownloadState::Removed,
            ],
            "FLOWS SM1: terminal states are completed, removed, and failed after retries"
        );
    }

    /// Every cell of the state × event matrix, in both directions: a cell the table declares
    /// legal must produce exactly that state, and a cell it does not declare must be rejected.
    #[test]
    fn only_the_declared_cells_of_the_matrix_are_legal() {
        for state in DownloadState::ALL {
            for kind in KINDS {
                match (transition(*state, kind.event()), declared(*state, *kind)) {
                    (Ok(next), Some(expected)) => assert_eq!(
                        next, expected,
                        "{state:?} + {kind:?} moved to {next:?}, not {expected:?}"
                    ),
                    (Err(_), None) => {}
                    (Ok(next), None) => panic!(
                        "{state:?} + {kind:?} moved to {next:?}, but the table calls it illegal"
                    ),
                    (Err(rejection), Some(expected)) => panic!(
                        "{state:?} + {kind:?} should move to {expected:?}, but was rejected: \
                         {rejection}"
                    ),
                }
            }
        }
    }

    /// A table with a duplicate cell could hide a missing one, and an entry naming an event
    /// outside [`KINDS`] would never be exercised by the matrix test above.
    #[test]
    fn the_declared_matrix_has_no_duplicate_cell_and_no_stray_kinds() {
        for (index, (state, kind, _)) in LEGAL.iter().enumerate() {
            assert!(
                !LEGAL[..index]
                    .iter()
                    .any(|(seen_state, seen_kind, _)| seen_state == state && seen_kind == kind),
                "{state:?} + {kind:?} is declared twice"
            );
            assert!(
                DownloadState::ALL.contains(state),
                "{state:?} is not a state"
            );
            assert!(
                KINDS.contains(kind),
                "{kind:?} is not a kind the matrix tests"
            );
        }
    }

    /// Rejection is not one bucket: the reason separates racing a finish, asking the wrong
    /// question, and being refused by the security gate.
    #[test]
    fn a_rejection_names_why_it_was_refused() {
        assert_eq!(
            transition(
                DownloadState::Blocked,
                DownloadEvent::Consented(opt_in_required())
            ),
            Err(TransitionRejected::OptInNotGranted {
                state: DownloadState::Blocked,
                event: "consented",
            })
        );
        assert_eq!(
            transition(DownloadState::Blocked, DownloadEvent::Consented(blocked())),
            Err(TransitionRejected::NoOverride {
                state: DownloadState::Blocked,
                event: "consented",
            })
        );
        assert_eq!(
            transition(DownloadState::Downloading, DownloadEvent::EngineAccepted),
            Err(TransitionRejected::OutOfOrder {
                state: DownloadState::Downloading,
                event: "engine accepted",
            })
        );
        assert_eq!(
            transition(DownloadState::Removed, DownloadEvent::Progress),
            Err(TransitionRejected::Terminal {
                state: DownloadState::Removed,
                event: "progress",
            })
        );
    }

    /// A terminal state absorbs every event, not just the late one a test thought of.
    #[test]
    fn a_terminal_state_absorbs_every_event() {
        for state in terminal() {
            for kind in KINDS {
                assert_eq!(
                    transition(state, kind.event()),
                    Err(TransitionRejected::Terminal {
                        state,
                        event: kind.event().name(),
                    }),
                    "{state:?} let {kind:?} out"
                );
            }
        }
    }

    /// Acceptance: every state is reachable, and it is reached by the events the table declares
    /// rather than by naming the variant.
    #[test]
    fn every_state_is_reachable_from_the_initial_state() {
        let mut reached = vec![DownloadState::Queued];
        let mut frontier = vec![DownloadState::Queued];
        while let Some(state) = frontier.pop() {
            for kind in KINDS {
                if let Ok(next) = transition(state, kind.event())
                    && !reached.contains(&next)
                {
                    reached.push(next);
                    frontier.push(next);
                }
            }
        }
        for state in DownloadState::ALL {
            assert!(reached.contains(state), "{state:?} is unreachable");
        }
        assert_eq!(reached.len(), DownloadState::ALL.len());
    }

    /// I4, as a test over the whole matrix: `metadata` is only ever entered by an event carrying
    /// a report, so no event without one can advance a job past `inspecting`.
    #[test]
    fn reaching_metadata_always_consumes_a_safety_report() {
        for state in DownloadState::ALL {
            // A full disk or a statistics tick inside `metadata` returns the state it was given;
            // that is not an entry into it.
            if *state == DownloadState::Metadata {
                continue;
            }
            for kind in KINDS {
                if transition(*state, kind.event()) != Ok(DownloadState::Metadata) {
                    continue;
                }
                assert!(
                    matches!(kind, Kind::Inspected(_) | Kind::Consented(_)),
                    "I4: {state:?} reached metadata on {kind:?}, which carries no report"
                );
            }
        }
    }

    /// A refusal with no override and a refusal the user may take anyway lead to different
    /// places, and which is which is the report's decision rather than the caller's.
    #[test]
    fn a_hard_refusal_and_an_opt_in_refusal_lead_to_different_places() {
        // Both park the job in `blocked`, with the report persisted, because that is where an
        // unsettled refusal waits and where it can be dismissed.
        assert_eq!(
            transition(
                DownloadState::Inspecting,
                DownloadEvent::Inspected(opt_in_required())
            ),
            Ok(DownloadState::Blocked)
        );
        assert_eq!(
            transition(
                DownloadState::Inspecting,
                DownloadEvent::Inspected(blocked())
            ),
            Ok(DownloadState::Blocked)
        );

        // Only the opt-in one comes out, because only it re-inspects to `Allowed`.
        assert_eq!(
            transition(DownloadState::Blocked, DownloadEvent::Consented(granted())),
            Ok(DownloadState::Metadata)
        );
        assert_eq!(
            transition(DownloadState::Blocked, DownloadEvent::Consented(blocked())),
            Err(TransitionRejected::NoOverride {
                state: DownloadState::Blocked,
                event: "consented",
            })
        );

        // The line is the inspector's, not the caller's: the traversal is refused with the
        // opt-in granted and the archive is allowed by it.
        assert_eq!(
            report("../authorized_keys", OptIn::Granted).verdict(),
            Verdict::Blocked
        );
        assert_eq!(
            report("extras.zip", OptIn::Granted).verdict(),
            Verdict::Allowed
        );

        // Consent answers a refusal, so a job the gate allowed never takes one.
        assert_eq!(
            transition(
                DownloadState::Inspecting,
                DownloadEvent::Consented(allowed())
            ),
            Err(TransitionRejected::OutOfOrder {
                state: DownloadState::Inspecting,
                event: "consented",
            })
        );
    }

    /// ARCHITECTURE §15: a full disk pauses an active transfer rather than failing it, and the
    /// paused job has a way back to running.
    #[test]
    fn a_full_disk_pauses_rather_than_fails_and_the_job_can_resume() {
        let paused = transition(DownloadState::Downloading, DownloadEvent::DiskFull)
            .expect("a full disk pauses a transfer");
        assert_eq!(paused, DownloadState::Paused);
        assert_ne!(
            paused,
            DownloadState::Failed,
            "a full disk is not a failure"
        );
        assert_eq!(
            transition(paused, DownloadEvent::Resumed),
            Ok(DownloadState::Downloading),
            "the resume affordance has a transition behind it"
        );
        // The condition is a level, so the same report while paused changes nothing.
        assert_eq!(
            transition(paused, DownloadEvent::DiskFull),
            Ok(DownloadState::Paused)
        );
        // A job that has not started writing bytes stays where it is: the free-space floor
        // decides admission, and a queued job is not admitted.
        assert_eq!(
            transition(DownloadState::Queued, DownloadEvent::DiskFull),
            Ok(DownloadState::Queued)
        );
    }

    /// AGENTS §4's duplicate and concurrent rows: each of these is either refused or ignored on
    /// purpose, and the choice is written down.
    #[test]
    fn duplicate_and_late_events_are_refused_or_ignored_by_rule() {
        // A completion after a cancel is refused: the job is history and must not come back.
        assert_eq!(
            transition(DownloadState::Removed, DownloadEvent::PayloadComplete),
            Err(TransitionRejected::Terminal {
                state: DownloadState::Removed,
                event: "payload complete",
            })
        );
        // A progress tick after a terminal state is refused, so a caller whose coalescing is
        // behind learns it instead of seeing an `Ok`.
        assert_eq!(
            transition(DownloadState::Completed, DownloadEvent::Progress),
            Err(TransitionRejected::Terminal {
                state: DownloadState::Completed,
                event: "progress",
            })
        );
        // A re-delivered report after the job has advanced is refused rather than applied twice.
        assert_eq!(
            transition(DownloadState::Metadata, DownloadEvent::Inspected(allowed())),
            Err(TransitionRejected::OutOfOrder {
                state: DownloadState::Metadata,
                event: "inspected",
            })
        );
        // Duplicate commands are out of order: a second acceptance, a second pause, a resume of
        // a job that is already running.
        assert_eq!(
            transition(DownloadState::Downloading, DownloadEvent::EngineAccepted),
            Err(TransitionRejected::OutOfOrder {
                state: DownloadState::Downloading,
                event: "engine accepted",
            })
        );
        assert_eq!(
            transition(DownloadState::Paused, DownloadEvent::Paused),
            Err(TransitionRejected::OutOfOrder {
                state: DownloadState::Paused,
                event: "paused",
            })
        );
        assert_eq!(
            transition(DownloadState::Downloading, DownloadEvent::Resumed),
            Err(TransitionRejected::OutOfOrder {
                state: DownloadState::Downloading,
                event: "resumed",
            })
        );
        // A completion that arrives while the transfer is stopped is refused rather than
        // inferred: the engine is not running, so its completion would be stale, and the
        // seeding policy must not apply to a transfer whose last pieces never arrived.
        assert_eq!(
            transition(DownloadState::Paused, DownloadEvent::PayloadComplete),
            Err(TransitionRejected::OutOfOrder {
                state: DownloadState::Paused,
                event: "payload complete",
            })
        );
    }

    /// A job is paused before it is removed, because SM1 gives `seeding` no removal edge: a
    /// seeding job that is removed has to be stopped first, and F12 says the same about deleting
    /// a file that is still being seeded.
    #[test]
    fn a_seeding_job_is_stopped_before_it_is_removed() {
        assert_eq!(
            transition(DownloadState::Seeding, DownloadEvent::Removed),
            Err(TransitionRejected::OutOfOrder {
                state: DownloadState::Seeding,
                event: "removed",
            })
        );
        assert_eq!(
            transition(DownloadState::Seeding, DownloadEvent::Paused),
            Ok(DownloadState::Paused)
        );
        assert_eq!(
            transition(DownloadState::Paused, DownloadEvent::Removed),
            Ok(DownloadState::Removed)
        );
    }

    proptest! {
        #[test]
        fn the_same_state_and_event_always_produce_the_same_result(
            state in state_strategy(),
            kind in kind_strategy(),
        ) {
            prop_assert_eq!(
                transition(state, kind.event()),
                transition(state, kind.event())
            );
        }

        #[test]
        fn a_transition_agrees_with_the_declared_matrix(
            state in state_strategy(),
            kind in kind_strategy(),
        ) {
            prop_assert_eq!(transition(state, kind.event()).ok(), declared(state, kind));
        }

        #[test]
        fn no_event_leaves_a_terminal_state(
            state in state_strategy(),
            kind in kind_strategy(),
        ) {
            if state.is_terminal() {
                prop_assert!(is_terminal_rejection(state, kind), "{state:?} let {kind:?} out");
            }
        }

        /// I4 along a whole path, not one step: a job can only be anywhere past `inspecting`
        /// if an event carrying a report was applied to get there. A rejected event grants
        /// nothing.
        #[test]
        fn no_event_path_advances_past_inspecting_without_a_report(
            kinds in prop::collection::vec(kind_strategy(), 0..24),
        ) {
            let mut state = DownloadState::Queued;
            let mut reported = false;
            for kind in kinds {
                if let Ok(next) = transition(state, kind.event()) {
                    state = next;
                    reported |= matches!(kind, Kind::Inspected(_) | Kind::Consented(_));
                }
                if !matches!(state, DownloadState::Queued | DownloadState::Inspecting) {
                    prop_assert!(reported, "I4: reached {:?} with no safety report", state);
                }
            }
        }
    }
}
