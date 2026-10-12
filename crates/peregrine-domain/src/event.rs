//! The declared event vocabulary: every state-changing operation this crate expresses, named
//! once so a repository cannot write a name of its own.
//!
//! `event_log` (`DATA_MODEL` §2.8) is the durable audit trail: `kind` says what happened, and
//! `subject_kind` with `subject_id` says what it happened to. The schema puts no `CHECK` on
//! `kind`, so a repository could write `download.faled` and nothing failed. [`Event`] is that
//! constraint as a type: it is the only way to produce a `kind`, `peregrine-storage`'s
//! `NewEvent` holds one instead of a string, and a caller that invents a name does not compile.
//!
//! Three vocabularies meet around events, and they are deliberately different:
//!
//! - [`crate::notify::NotificationEvent`] is the closed catalogue of what may raise a
//!   notification, and `notification.event` stores its keys. It is a reaction to an operation
//!   rather than the operation itself, and it is smaller.
//! - `peregrine-app`'s bus topics (`job://progress`; ARCHITECTURE §7 rule 4) are the live,
//!   versioned streams a UI subscribes to. They are transient and carry payloads.
//! - [`Event`] is the durable one: a variant per operation that changes a row, named the way
//!   `DATA_MODEL` §2.8 and FLOWS already spell it (`download.failed`, `progress.updated`).
//!
//! # What earns a variant
//!
//! An operation earns a variant only when this crate expresses it *and* it changes a row the
//! event can be about, because `event_log` stores the subject as an id. That is the download
//! state machine (SM1) and the watch-progress writes of F10, and nothing else yet: availability
//! (SM2) is a derivation that writes nothing, a playback session (SM3) is not a row, and the
//! `notification` and `job` tables are their own durable records. Monitoring, agent access, the
//! media server, and local accounts are phases whose tables no migration creates. A variant for
//! any of those would name something this build cannot do (AGENTS R3).
//!
//! # Names
//!
//! A name is a contract with two audiences that cannot be regenerated: the UI branches and
//! filters on it, and rows already written hold it. A rename therefore costs a data migration
//! plus a UI change rather than an edit, which is why `declared_names_are_pinned` fails on one.
//!
//! `stored_enum!` is the house pattern for an enum that mirrors a `CHECK` constraint, and
//! [`SubjectKind`] uses it. [`Event`] cannot: its variants carry the subject's id, and its
//! spellings are namespaced names (`download.failed`) rather than `snake_case` identifiers, so
//! the name, the parse, and the subject mapping are written by hand — and `as_str`'s exhaustive
//! `match` is what makes a variant without a name a build failure rather than a silent string.

use crate::download::DownloadState;
use crate::stored_enum;

stored_enum! {
    /// The table the row an event is about lives in, as `event_log.subject_kind` stores it.
    ///
    /// The column has no `CHECK`, so this enum is the whole constraint on it. Each spelling is
    /// the table's own name from `DATA_MODEL` §2, which is what lets a reader resolve the
    /// subject without a second mapping — and what makes `notification.subject_kind`'s
    /// `download` spelling for the same row a divergence (see `docs/DATA_MODEL.md` §2.8).
    SubjectKind {
        /// A `download_job` row.
        DownloadJob => "download_job",
        /// A `title` row.
        Title => "title",
        /// An `episode` row.
        Episode => "episode",
    }
}

/// The title or episode a watch-progress row belongs to.
///
/// `watch_progress` is unique on `(account_id, entity_key)`, where the key is the episode when
/// there is one and the title otherwise (`DATA_MODEL` §2.6), so this is the subject a progress
/// event has. The account is deliberately not part of it: `event_log` has no account column, so
/// a progress event names the content and not the row it changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WatchSubject {
    /// A `title` row: a film, or a position recorded before the episode is known.
    Title(i64),
    /// An `episode` row.
    Episode(i64),
}

impl WatchSubject {
    /// The subject row's id.
    #[must_use]
    pub const fn id(self) -> i64 {
        match self {
            Self::Title(id) | Self::Episode(id) => id,
        }
    }

    /// The table the subject row lives in.
    #[must_use]
    pub const fn kind(self) -> SubjectKind {
        match self {
            Self::Title(_) => SubjectKind::Title,
            Self::Episode(_) => SubjectKind::Episode,
        }
    }
}

/// One state-changing operation, in the vocabulary `event_log.kind` stores.
///
/// Every variant carries the subject's row id, so an event about nothing is not constructible,
/// and every variant's subject kind follows from the variant rather than from a second field a
/// caller could contradict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Event {
    /// A `download_job` row was inserted: SM1's `[*] --> queued`.
    DownloadQueued {
        /// The `download_job.id`.
        job_id: i64,
    },
    /// The safety gate is about to run: SM1's `queued --> inspecting`.
    DownloadInspecting {
        /// The `download_job.id`.
        job_id: i64,
    },
    /// The first inspection allowed the torrent: SM1's `inspecting --> metadata`.
    DownloadApproved {
        /// The `download_job.id`.
        job_id: i64,
    },
    /// The gate refused the torrent and the job waits for the user: SM1's
    /// `inspecting --> blocked`.
    DownloadBlocked {
        /// The `download_job.id`.
        job_id: i64,
    },
    /// The user took the advanced opt-in and the re-inspection allowed it: SM1's
    /// `blocked --> metadata`. This is the override PG-E11-10 audits.
    DownloadOverridden {
        /// The `download_job.id`.
        job_id: i64,
    },
    /// The engine accepted the torrent and its file selection: SM1's
    /// `metadata --> downloading`.
    DownloadStarted {
        /// The `download_job.id`.
        job_id: i64,
    },
    /// The transfer stopped and can resume: SM1's `downloading --> paused` and
    /// `seeding --> paused`, including the full-disk pause of ARCHITECTURE §15.
    DownloadPaused {
        /// The `download_job.id`.
        job_id: i64,
    },
    /// The transfer resumed: SM1's `paused --> downloading`.
    DownloadResumed {
        /// The `download_job.id`.
        job_id: i64,
    },
    /// The payload is complete and uploading back: SM1's `downloading --> seeding`.
    DownloadSeeding {
        /// The `download_job.id`.
        job_id: i64,
    },
    /// The seeding policy was satisfied: SM1's `seeding --> completed`.
    DownloadCompleted {
        /// The `download_job.id`.
        job_id: i64,
    },
    /// An attempt failed: SM1's `metadata --> failed` and `downloading --> failed`.
    DownloadFailed {
        /// The `download_job.id`.
        job_id: i64,
    },
    /// The user removed a transfer or dismissed a block: SM1's `--> removed` edges.
    DownloadRemoved {
        /// The `download_job.id`.
        job_id: i64,
    },
    /// A position was recorded: F10's coalesced write, [`crate::library::WatchProgress::record`],
    /// or [`crate::library::WatchProgress::set_position`].
    ProgressUpdated {
        /// The title or episode whose row was written.
        subject: WatchSubject,
    },
    /// A row was marked watched: the policy's [`crate::watch::finalize`] applied by
    /// [`crate::library::WatchProgress::apply_finalize`], or the user's
    /// [`crate::library::WatchProgress::mark_watched`].
    ProgressWatched {
        /// The title or episode whose row was written.
        subject: WatchSubject,
    },
    /// The user cleared the watched flag ([`crate::library::WatchProgress::mark_unwatched`]).
    ProgressUnwatched {
        /// The title or episode whose row was written.
        subject: WatchSubject,
    },
}

impl Event {
    /// The value `event_log.kind` stores, for example `download.failed`.
    ///
    /// The `match` is exhaustive on purpose: a variant added without a name does not compile,
    /// which is the strongest form of "every variant has one" available without a macro.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DownloadQueued { .. } => "download.queued",
            Self::DownloadInspecting { .. } => "download.inspecting",
            Self::DownloadApproved { .. } => "download.approved",
            Self::DownloadBlocked { .. } => "download.blocked",
            Self::DownloadOverridden { .. } => "download.overridden",
            Self::DownloadStarted { .. } => "download.started",
            Self::DownloadPaused { .. } => "download.paused",
            Self::DownloadResumed { .. } => "download.resumed",
            Self::DownloadSeeding { .. } => "download.seeding",
            Self::DownloadCompleted { .. } => "download.completed",
            Self::DownloadFailed { .. } => "download.failed",
            Self::DownloadRemoved { .. } => "download.removed",
            Self::ProgressUpdated { .. } => "progress.updated",
            Self::ProgressWatched { .. } => "progress.watched",
            Self::ProgressUnwatched { .. } => "progress.unwatched",
        }
    }

    /// The `event_log.subject_kind` the row needs.
    #[must_use]
    pub const fn subject_kind(self) -> SubjectKind {
        match self {
            Self::DownloadQueued { .. }
            | Self::DownloadInspecting { .. }
            | Self::DownloadApproved { .. }
            | Self::DownloadBlocked { .. }
            | Self::DownloadOverridden { .. }
            | Self::DownloadStarted { .. }
            | Self::DownloadPaused { .. }
            | Self::DownloadResumed { .. }
            | Self::DownloadSeeding { .. }
            | Self::DownloadCompleted { .. }
            | Self::DownloadFailed { .. }
            | Self::DownloadRemoved { .. } => SubjectKind::DownloadJob,
            Self::ProgressUpdated { subject }
            | Self::ProgressWatched { subject }
            | Self::ProgressUnwatched { subject } => subject.kind(),
        }
    }

    /// The `event_log.subject_id` the row needs.
    #[must_use]
    pub const fn subject_id(self) -> i64 {
        match self {
            Self::DownloadQueued { job_id }
            | Self::DownloadInspecting { job_id }
            | Self::DownloadApproved { job_id }
            | Self::DownloadBlocked { job_id }
            | Self::DownloadOverridden { job_id }
            | Self::DownloadStarted { job_id }
            | Self::DownloadPaused { job_id }
            | Self::DownloadResumed { job_id }
            | Self::DownloadSeeding { job_id }
            | Self::DownloadCompleted { job_id }
            | Self::DownloadFailed { job_id }
            | Self::DownloadRemoved { job_id } => job_id,
            Self::ProgressUpdated { subject }
            | Self::ProgressWatched { subject }
            | Self::ProgressUnwatched { subject } => subject.id(),
        }
    }

    /// The event a `download_job` records when [`crate::download::transition`] moves it from
    /// `from` to `to`.
    ///
    /// `None` when the edge changes nothing — `from == to`, which is a full-disk level or a
    /// progress tick rather than a transition — or when this build declares no event for it.
    /// The second case is what makes a new edge in SM1 fail
    /// `the_download_vocabulary_covers_the_machine` instead of going unrecorded, and what keeps
    /// a repository from having to decide which name an edge gets.
    #[must_use]
    pub const fn download_edge(
        job_id: i64,
        from: DownloadState,
        to: DownloadState,
    ) -> Option<Self> {
        use DownloadState::{
            Blocked, Completed, Downloading, Failed, Inspecting, Metadata, Paused, Queued, Removed,
            Seeding,
        };
        Some(match (from, to) {
            (Queued, Inspecting) => Self::DownloadInspecting { job_id },
            (Inspecting, Metadata) => Self::DownloadApproved { job_id },
            (Inspecting, Blocked) => Self::DownloadBlocked { job_id },
            (Blocked, Metadata) => Self::DownloadOverridden { job_id },
            (Metadata, Downloading) => Self::DownloadStarted { job_id },
            (Downloading | Seeding, Paused) => Self::DownloadPaused { job_id },
            (Paused, Downloading) => Self::DownloadResumed { job_id },
            (Downloading, Seeding) => Self::DownloadSeeding { job_id },
            (Seeding, Completed) => Self::DownloadCompleted { job_id },
            (Metadata | Downloading, Failed) => Self::DownloadFailed { job_id },
            (Blocked | Downloading | Paused, Removed) => Self::DownloadRemoved { job_id },
            _ => return None,
        })
    }

    /// Rebuilds the event a row records, or `None` when this build does not declare the kind or
    /// the row's subject is not one that kind is about.
    ///
    /// This is the read half of the vocabulary: a row written by a later version parses to
    /// `None` rather than to a wrong event, so a reader can say "this build does not know that
    /// name" instead of guessing.
    #[must_use]
    pub fn parse(kind: &str, subject_kind: SubjectKind, subject_id: i64) -> Option<Self> {
        Some(match kind {
            "download.queued" => Self::DownloadQueued {
                job_id: download_job(subject_kind, subject_id)?,
            },
            "download.inspecting" => Self::DownloadInspecting {
                job_id: download_job(subject_kind, subject_id)?,
            },
            "download.approved" => Self::DownloadApproved {
                job_id: download_job(subject_kind, subject_id)?,
            },
            "download.blocked" => Self::DownloadBlocked {
                job_id: download_job(subject_kind, subject_id)?,
            },
            "download.overridden" => Self::DownloadOverridden {
                job_id: download_job(subject_kind, subject_id)?,
            },
            "download.started" => Self::DownloadStarted {
                job_id: download_job(subject_kind, subject_id)?,
            },
            "download.paused" => Self::DownloadPaused {
                job_id: download_job(subject_kind, subject_id)?,
            },
            "download.resumed" => Self::DownloadResumed {
                job_id: download_job(subject_kind, subject_id)?,
            },
            "download.seeding" => Self::DownloadSeeding {
                job_id: download_job(subject_kind, subject_id)?,
            },
            "download.completed" => Self::DownloadCompleted {
                job_id: download_job(subject_kind, subject_id)?,
            },
            "download.failed" => Self::DownloadFailed {
                job_id: download_job(subject_kind, subject_id)?,
            },
            "download.removed" => Self::DownloadRemoved {
                job_id: download_job(subject_kind, subject_id)?,
            },
            "progress.updated" => Self::ProgressUpdated {
                subject: watch_subject(subject_kind, subject_id)?,
            },
            "progress.watched" => Self::ProgressWatched {
                subject: watch_subject(subject_kind, subject_id)?,
            },
            "progress.unwatched" => Self::ProgressUnwatched {
                subject: watch_subject(subject_kind, subject_id)?,
            },
            _ => return None,
        })
    }
}

/// The job id when the row's subject is a download job, so a download name cannot be read back
/// with, say, a title id.
const fn download_job(subject_kind: SubjectKind, subject_id: i64) -> Option<i64> {
    match subject_kind {
        SubjectKind::DownloadJob => Some(subject_id),
        SubjectKind::Title | SubjectKind::Episode => None,
    }
}

/// The content subject when the row's subject is a title or an episode.
const fn watch_subject(subject_kind: SubjectKind, subject_id: i64) -> Option<WatchSubject> {
    match subject_kind {
        SubjectKind::DownloadJob => None,
        SubjectKind::Title => Some(WatchSubject::Title(subject_id)),
        SubjectKind::Episode => Some(WatchSubject::Episode(subject_id)),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// The names a repository may write, in declaration order.
    ///
    /// This list is the contract the module documentation describes: `declared_names_are_pinned`
    /// fails when one is renamed or dropped, and the coverage test below reads it as the whole
    /// download vocabulary.
    pub(crate) const DECLARED: &[&str] = &[
        "download.queued",
        "download.inspecting",
        "download.approved",
        "download.blocked",
        "download.overridden",
        "download.started",
        "download.paused",
        "download.resumed",
        "download.seeding",
        "download.completed",
        "download.failed",
        "download.removed",
        "progress.updated",
        "progress.watched",
        "progress.unwatched",
    ];

    /// One sample of every variant, so the tests below walk the whole vocabulary.
    ///
    /// The ids are arbitrary: what is under test is the round trip through a row, not the value.
    /// A variant added without a sample here is caught by review rather than by the compiler —
    /// the compiler catches the missing *name*, through [`Event::as_str`].
    fn samples() -> Vec<Event> {
        vec![
            Event::DownloadQueued { job_id: 1 },
            Event::DownloadInspecting { job_id: 1 },
            Event::DownloadApproved { job_id: 1 },
            Event::DownloadBlocked { job_id: 1 },
            Event::DownloadOverridden { job_id: 1 },
            Event::DownloadStarted { job_id: 1 },
            Event::DownloadPaused { job_id: 1 },
            Event::DownloadResumed { job_id: 1 },
            Event::DownloadSeeding { job_id: 1 },
            Event::DownloadCompleted { job_id: 1 },
            Event::DownloadFailed { job_id: 1 },
            Event::DownloadRemoved { job_id: 1 },
            Event::ProgressUpdated {
                subject: WatchSubject::Episode(9),
            },
            Event::ProgressWatched {
                subject: WatchSubject::Episode(9),
            },
            Event::ProgressUnwatched {
                subject: WatchSubject::Episode(9),
            },
        ]
    }

    /// The names are the UI's and the stored rows' contract, so a rename is a deliberate edit
    /// here rather than an accident in a producer.
    #[test]
    fn declared_names_are_pinned() {
        let rendered: Vec<&str> = samples().iter().map(|event| event.as_str()).collect();
        assert_eq!(
            rendered, DECLARED,
            "the event vocabulary changed; a rename is a migration plus a UI change"
        );
    }

    #[test]
    fn declared_names_are_unique() {
        let mut names = DECLARED.to_vec();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), DECLARED.len(), "two events share a name");
    }

    /// The write half and the read half agree for every variant: what a repository appends is
    /// what a reader rebuilds from the row's three columns.
    #[test]
    fn every_declared_event_round_trips_through_parse() {
        for event in samples() {
            let parsed = Event::parse(event.as_str(), event.subject_kind(), event.subject_id());
            assert_eq!(parsed, Some(event), "{} did not round-trip", event.as_str());
        }

        for subject in [WatchSubject::Title(3), WatchSubject::Episode(4)] {
            let event = Event::ProgressUpdated { subject };
            assert_eq!(event.subject_id(), subject.id());
            assert_eq!(
                Event::parse(event.as_str(), event.subject_kind(), event.subject_id()),
                Some(event),
                "both content subjects must round-trip"
            );
        }
    }

    /// An undeclared name is not a near miss: a row written by another version reads back as
    /// unknown rather than as the event it resembles.
    #[test]
    fn parse_refuses_a_name_this_build_does_not_declare() {
        for name in [
            "",
            "media.added",
            "media.deleted",
            "download.faled",
            "Download.Failed",
            "download.failed ",
            "progress.updated2",
        ] {
            assert_eq!(
                Event::parse(name, SubjectKind::DownloadJob, 1),
                None,
                "{name:?} must be refused"
            );
        }
    }

    /// A row whose subject is the wrong kind is not an event either: a download name cannot be
    /// read back with a title, and a progress name cannot be read back with a job.
    #[test]
    fn parse_refuses_a_subject_the_event_is_not_about() {
        assert_eq!(Event::parse("download.failed", SubjectKind::Title, 1), None);
        assert_eq!(
            Event::parse("download.failed", SubjectKind::Episode, 1),
            None
        );
        assert_eq!(
            Event::parse("progress.updated", SubjectKind::DownloadJob, 1),
            None
        );
    }

    /// PG-E4-16: the declared names cover SM1 exactly. Every state-changing cell of the
    /// machine's own matrix is named, and every declared download name describes such a cell —
    /// so a variant is neither missing nor speculative.
    #[test]
    fn the_download_vocabulary_covers_the_machine() {
        let mut named = BTreeSet::new();
        for (from, event, to) in crate::download::tests::legal_cells() {
            let next = crate::download::transition(from, event)
                .expect("the matrix and the machine disagree");
            assert_eq!(next, to, "the matrix and the machine disagree");

            match Event::download_edge(1, from, to) {
                Some(declared) => {
                    assert_ne!(
                        next,
                        from,
                        "{from:?} -> {to:?} changes nothing but declares {}",
                        declared.as_str()
                    );
                    named.insert(declared.as_str());
                }
                None => assert_eq!(
                    next, from,
                    "{from:?} -> {to:?} changes the state and has no declared event"
                ),
            }
        }

        // `[*] --> queued` is the row's insert rather than a cell the matrix can express, so it
        // is named here instead of by the walk.
        named.insert(Event::DownloadQueued { job_id: 1 }.as_str());

        let declared: BTreeSet<&str> = DECLARED
            .iter()
            .copied()
            .filter(|name| name.starts_with("download."))
            .collect();
        assert_eq!(named, declared, "the vocabulary and SM1's matrix disagree");
    }

    /// The download bridge answers only for edges the machine really has: a state that absorbs
    /// has no edge out of it, and an undeclared pair is not an event.
    #[test]
    fn download_edges_are_only_the_machines_own() {
        assert_eq!(
            Event::download_edge(1, DownloadState::Queued, DownloadState::Queued),
            None,
            "an unchanged state is not an event"
        );
        assert_eq!(
            Event::download_edge(1, DownloadState::Completed, DownloadState::Downloading),
            None,
            "a terminal state has no exit"
        );
        assert_eq!(
            Event::download_edge(1, DownloadState::Queued, DownloadState::Removed),
            None,
            "SM1 does not have this edge yet"
        );
        assert_eq!(
            Event::download_edge(7, DownloadState::Downloading, DownloadState::Failed),
            Some(Event::DownloadFailed { job_id: 7 })
        );
    }
}
