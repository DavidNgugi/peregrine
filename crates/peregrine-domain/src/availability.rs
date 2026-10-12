//! Media availability: the SM2 state the poster overlay, the badges, and the availability filter
//! show for one library item.
//!
//! [`availability`] is a derivation, not a column ([FLOWS] SM2). It reads a [`MediaItem`], that
//! item's `download_job` set, and its `media_file` set, and computes the answer from those three
//! inputs every time. Nothing writes the answer back, no clock is read, and the order the jobs
//! arrive in cannot change the result: the same inputs always derive the same value.
//!
//! # What decides, in one order
//!
//! Evidence is read in one order, and the first line that has any decides:
//!
//! - **File rows.** A row is a copy that exists: every row present is `available`, some present is
//!   `partial`, none present is `missing`. A missing row is kept rather than deleted (I11), which
//!   is why its absence from disk is not the same as its absence from the table. Which copy is
//!   preferred (`media_item.preferred_file_id`) does not enter into it: `partial` is about how many
//!   copies exist, and choosing which one to play is the player's decision rather than this one.
//! - **A live job.** With no file row at all, a job that is not terminal ([`DownloadState`]) is an
//!   acquisition in flight, so the item is `acquiring`. A live job also outranks a *newer*
//!   terminal one: an older transfer still running beside a newer failed attempt at another
//!   release is a download in progress, not a failed item.
//! - **The newest terminal job**, once nothing is live: `failed` is `unavailable`, `completed`
//!   with no rows left is `removed` (the payload was imported, so the rows are copies that were
//!   deleted), and `removed` leaves the question to what was asked.
//! - **The item's own row, read as history.** With no rows and no jobs, `media_item.state` is the
//!   last record of what happened: `available` or `partial` there with no file rows means the rows
//!   were deleted (`removed`), and `failed` there means the failure is the newest thing known
//!   (`unavailable`). `wanted` and `downloading` record no outcome, so the ask decides.
//! - **What was asked.** [`Asked`], the one input the item's row cannot supply.
//!
//! Files outrank jobs because a present copy is playable whatever the transfer is doing, and SM2
//! draws no edge from `available`, `partial`, or `missing` into `acquiring` — an upgrade in flight
//! does not make a playable item unplayable. The reverse is also deliberate: a `downloading` row
//! with no live job is stale, and the job set, not the row, is the evidence that a transfer is
//! running.
//!
//! # The newest job
//!
//! A retry after a failure is a new job rather than an edge of the old one ([`DownloadState`]), so
//! an item can have several. The newest is the greatest `(added_at, id)` in the set the caller
//! passes. `added_at` is the instant the row already recorded — this crate reads no clock, so the
//! ordering has to arrive as a value — and `id` breaks a same-millisecond tie, because
//! `download_job.id` is a monotonic rowid and the later insert has the greater one. Both are
//! inputs, both are unique per row, and a permutation of the slice therefore derives the same
//! answer. Two entries that share a row's identity are that row twice; a caller that hands the
//! function one id twice with two different states is describing something the table cannot hold.
//!
//! [`ItemJob`] is the projection of a `download_job` row this derivation reads, not the row type:
//! the row carries what the engine, the gate, the tracker, and the audit trail each need, and this
//! decision needs three of its columns.
//!
//! # Relationship to [`MediaItemState`]
//!
//! Availability is a **superset** of the stored column, and `MediaItemState` is untouched. Three
//! spellings mean the same thing in both — `wanted`, `available`, `partial` — and the other six
//! have no column spelling at all: `media_item.state` cannot hold them. The many-to-one map a
//! writer applies when it caches the derivation into the column is
//!
//! | [`Availability`] | `media_item.state` |
//! | --- | --- |
//! | `discovered`, `watchlisted`, `wanted` | `wanted` — the column collapses SM2's three pre-acquisition states into one word |
//! | `acquiring` | `downloading` |
//! | `available` | `available` |
//! | `partial` | `partial` |
//! | `missing` | `partial` — the item still has file rows, but none of them is present |
//! | `unavailable` | `failed` |
//! | `removed` | `partial`, or the item row is deleted (FLOWS F12 leaves that to the deletion path) |
//!
//! The derivation never reads that column as the answer and never writes it; a writer that caches
//! the result owns that write, and SM2's invalidation is what recomputes it.
//!
//! [`Asked`] exists because of the first row of that table: the watchlist is title-scoped
//! (`watchlist_entry`), so the state before a file or a job exists cannot be read from the item
//! row, and the caller that knows it passes it in rather than letting this function guess.
//!
//! [FLOWS]: ../../../docs/FLOWS.md

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::download::DownloadState;
use crate::library::{MediaFile, MediaItem, MediaItemState};

/// What the user has asked for, when nothing is owned and no job records it.
///
/// `media_item.state` has one spelling, `wanted`, for SM2's three states before a file or a job
/// exists, and the watchlist hangs off a title rather than an item, so neither the item's row nor
/// its jobs can tell these apart. The caller can, which is why this is a parameter: it is used
/// only while nothing on disk and no job outranks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Asked {
    /// The title is known and nobody has asked for it.
    Nothing,
    /// It is on the watchlist.
    Watchlisted,
    /// Acquisition has been requested, by the user or by an auto-acquire rule.
    Acquisition,
}

/// One of an item's download jobs, as far as availability reads it.
///
/// The derivation needs a job's state to know whether the acquisition is in flight, and its
/// `(added_at, id)` to know which of several jobs is newest. A repository builds this from the
/// `download_job` row it already has; nothing here writes one back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemJob {
    /// The `download_job.id`, which breaks a same-instant tie because the rowid increases.
    pub id: i64,
    /// The job's state in SM1's machine.
    pub state: DownloadState,
    /// The instant the row recorded for `download_job.added_at`.
    pub added_at: Timestamp,
}

/// The availability of one media item, in SM2's vocabulary.
///
/// This is not `media_item.state`: it is derived from the item and its evidence, it has six values
/// the column cannot hold, and one of its values is the column's word for three different things.
/// The spelling [`Availability::as_str`] returns is the one that leaves the core — the badge, the
/// filter, and the availability field of an agent-facing search result (ADR 0017) — so it is
/// stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    /// Known, and nothing asked for.
    Discovered,
    /// On the watchlist, with no acquisition requested yet.
    Watchlisted,
    /// Acquisition requested, with no job and no file yet.
    Wanted,
    /// An acquisition this item can see is in flight.
    Acquiring,
    /// Every file row is present.
    Available,
    /// Some file rows are present and some are not.
    Partial,
    /// The item has file rows and none of them is present.
    Missing,
    /// Every attempt this item has a record of failed.
    Unavailable,
    /// The item once had files and now has no file rows at all.
    Removed,
}

impl Availability {
    /// Every value, in the order FLOWS SM2's diagram reaches them.
    pub const ALL: [Self; 9] = [
        Self::Discovered,
        Self::Watchlisted,
        Self::Wanted,
        Self::Acquiring,
        Self::Available,
        Self::Partial,
        Self::Missing,
        Self::Unavailable,
        Self::Removed,
    ];

    /// The stable spelling the badge, the filter, and the agent surface read.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Discovered => "discovered",
            Self::Watchlisted => "watchlisted",
            Self::Wanted => "wanted",
            Self::Acquiring => "acquiring",
            Self::Available => "available",
            Self::Partial => "partial",
            Self::Missing => "missing",
            Self::Unavailable => "unavailable",
            Self::Removed => "removed",
        }
    }
}

/// Derives what SM2 would call this item's state, from the item, its jobs, and its files.
///
/// The rule set and the reasoning behind the order are in the module documentation; this function
/// is that order and nothing else. It reads no clock and no ambient state, and the order `jobs`
/// arrives in does not matter: every input is a parameter, so the result is a function of their
/// values. `jobs` is the *whole* job set for the item — a caller that passes a filtered slice is
/// under-reporting an acquisition that is still running.
#[must_use]
pub fn availability(
    item: &MediaItem,
    asked: Asked,
    jobs: &[ItemJob],
    files: &[MediaFile],
) -> Availability {
    // A file row is a copy that exists, and the count is the point: one copy present of several is
    // `partial`, not `available`, and one copy absent of one is `missing`, not `partial`.
    if !files.is_empty() {
        let present = files
            .iter()
            .filter(|file| file.missing_since.is_none())
            .count();
        return if present == files.len() {
            Availability::Available
        } else if present > 0 {
            Availability::Partial
        } else {
            Availability::Missing
        };
    }

    // A live job is an acquisition in flight, whatever it is doing: waiting for the gate, paused,
    // seeding, or running. It outranks a newer terminal job, so this is a test over the whole set
    // rather than a look at the newest one.
    if jobs.iter().any(|job| !job.state.is_terminal()) {
        return Availability::Acquiring;
    }

    if let Some(newest) = newest(jobs) {
        return match newest.state {
            DownloadState::Failed => Availability::Unavailable,
            DownloadState::Completed => Availability::Removed,
            DownloadState::Removed => from_asked(asked),
            // A live state cannot be newest here, because the test above returned for any live
            // job at all. Listing the states rather than wildcarding them means a new state in
            // SM1's machine is a compile error in this match, and the answer given is the same
            // one the test above would have given.
            DownloadState::Queued
            | DownloadState::Inspecting
            | DownloadState::Metadata
            | DownloadState::Downloading
            | DownloadState::Paused
            | DownloadState::Seeding
            | DownloadState::Blocked => Availability::Acquiring,
        };
    }

    // Nothing on disk and nothing in flight: the item's own row is the only record of what
    // happened before, and it is read for exactly that, never as the answer.
    match item.state {
        MediaItemState::Available | MediaItemState::Partial => Availability::Removed,
        MediaItemState::Failed => Availability::Unavailable,
        MediaItemState::Wanted | MediaItemState::Downloading => from_asked(asked),
    }
}

/// The greatest `(added_at, id)`, which is the newest job the set holds.
///
/// The pair is a row's identity, so the order is total for any set the table could hold and the
/// answer does not depend on which way the slice is walked.
fn newest(jobs: &[ItemJob]) -> Option<&ItemJob> {
    jobs.iter().max_by_key(|job| (job.added_at, job.id))
}

/// What an ask means while nothing is owned and no job is running.
const fn from_asked(asked: Asked) -> Availability {
    match asked {
        Asked::Nothing => Availability::Discovered,
        Asked::Watchlisted => Availability::Watchlisted,
        Asked::Acquisition => Availability::Wanted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::MediaOrigin;
    use crate::text::NonEmptyText;
    use proptest::prelude::*;

    /// One row of the acceptance table: the inputs the derivation reads and the value it must
    /// derive from them.
    ///
    /// `cached` is `media_item.state`, which the derivation reads only as history, and `preferred`
    /// is `media_item.preferred_file_id`. A job is `(id, state, added_at)` and a file is
    /// `(id, present)`.
    struct Case {
        /// What the row pins, so a failure names it.
        what: &'static str,
        /// The item's stored state.
        cached: MediaItemState,
        /// What the user has asked for.
        asked: Asked,
        /// The item's preferred file id, when it has one.
        preferred: Option<i64>,
        /// The item's jobs.
        jobs: &'static [(i64, DownloadState, i64)],
        /// The item's files.
        files: &'static [(i64, bool)],
        /// The availability the row must derive.
        expected: Availability,
    }

    /// The acceptance table: one row per SM2 state, then one per combination the states turn on.
    const CASES: &[Case] = &[
        // One row per state SM2 draws, in the diagram's order.
        Case {
            what: "nothing owned and nothing asked is discovered",
            cached: MediaItemState::Wanted,
            asked: Asked::Nothing,
            preferred: None,
            jobs: &[],
            files: &[],
            expected: Availability::Discovered,
        },
        Case {
            what: "an item on the watchlist with nothing requested is watchlisted",
            cached: MediaItemState::Wanted,
            asked: Asked::Watchlisted,
            preferred: None,
            jobs: &[],
            files: &[],
            expected: Availability::Watchlisted,
        },
        Case {
            what: "an acquisition asked for with no job yet is wanted",
            cached: MediaItemState::Wanted,
            asked: Asked::Acquisition,
            preferred: None,
            jobs: &[],
            files: &[],
            expected: Availability::Wanted,
        },
        Case {
            what: "a transfer that has not started is already acquiring",
            cached: MediaItemState::Downloading,
            asked: Asked::Acquisition,
            preferred: None,
            jobs: &[(1, DownloadState::Queued, 1_000)],
            files: &[],
            expected: Availability::Acquiring,
        },
        Case {
            what: "every file present is available",
            cached: MediaItemState::Available,
            asked: Asked::Acquisition,
            preferred: Some(1),
            jobs: &[],
            files: &[(1, true), (2, true)],
            expected: Availability::Available,
        },
        Case {
            what: "one copy of several gone is partial",
            cached: MediaItemState::Partial,
            asked: Asked::Acquisition,
            preferred: Some(1),
            jobs: &[],
            files: &[(1, true), (2, false)],
            expected: Availability::Partial,
        },
        Case {
            what: "every file row marked missing is missing, not partial",
            cached: MediaItemState::Partial,
            asked: Asked::Acquisition,
            preferred: Some(1),
            jobs: &[],
            files: &[(1, false), (2, false)],
            expected: Availability::Missing,
        },
        Case {
            what: "every attempt failed is unavailable",
            cached: MediaItemState::Failed,
            asked: Asked::Acquisition,
            preferred: None,
            jobs: &[
                (1, DownloadState::Failed, 1_000),
                (2, DownloadState::Failed, 2_000),
            ],
            files: &[],
            expected: Availability::Unavailable,
        },
        Case {
            what: "an item whose file rows are gone is removed",
            cached: MediaItemState::Available,
            asked: Asked::Acquisition,
            preferred: None,
            jobs: &[],
            files: &[],
            expected: Availability::Removed,
        },
        // The combinations the states above turn on.
        Case {
            what: "a retry supersedes the failed job it replaced",
            cached: MediaItemState::Failed,
            asked: Asked::Acquisition,
            preferred: None,
            jobs: &[
                (1, DownloadState::Failed, 1_000),
                (2, DownloadState::Downloading, 2_000),
            ],
            files: &[],
            expected: Availability::Acquiring,
        },
        Case {
            what: "a newer failed attempt does not override an older live job",
            cached: MediaItemState::Wanted,
            asked: Asked::Acquisition,
            preferred: None,
            jobs: &[
                (1, DownloadState::Downloading, 1_000),
                (2, DownloadState::Failed, 2_000),
            ],
            files: &[],
            expected: Availability::Acquiring,
        },
        Case {
            what: "a completion newer than a seeding job does not override it",
            cached: MediaItemState::Wanted,
            asked: Asked::Acquisition,
            preferred: None,
            jobs: &[
                (1, DownloadState::Seeding, 1_000),
                (2, DownloadState::Completed, 2_000),
            ],
            files: &[],
            expected: Availability::Acquiring,
        },
        Case {
            what: "a job held at the safety gate is still acquiring, not unavailable",
            cached: MediaItemState::Wanted,
            asked: Asked::Acquisition,
            preferred: None,
            jobs: &[(1, DownloadState::Blocked, 1_000)],
            files: &[],
            expected: Availability::Acquiring,
        },
        Case {
            what: "a stopped transfer is still acquiring",
            cached: MediaItemState::Downloading,
            asked: Asked::Acquisition,
            preferred: None,
            jobs: &[(1, DownloadState::Paused, 1_000)],
            files: &[],
            expected: Availability::Acquiring,
        },
        Case {
            what: "the newest terminal job decides when none is live",
            cached: MediaItemState::Failed,
            asked: Asked::Acquisition,
            preferred: None,
            jobs: &[
                (1, DownloadState::Failed, 1_000),
                (2, DownloadState::Removed, 2_000),
            ],
            files: &[],
            expected: Availability::Wanted,
        },
        Case {
            what: "a completed import whose rows are gone is a removal",
            cached: MediaItemState::Wanted,
            asked: Asked::Acquisition,
            preferred: None,
            jobs: &[(1, DownloadState::Completed, 1_000)],
            files: &[],
            expected: Availability::Removed,
        },
        Case {
            what: "a removed job leaves the item where the ask puts it",
            cached: MediaItemState::Wanted,
            asked: Asked::Watchlisted,
            preferred: None,
            jobs: &[(1, DownloadState::Removed, 1_000)],
            files: &[],
            expected: Availability::Watchlisted,
        },
        Case {
            what: "a same-instant tie is broken by the row id, lower id first",
            cached: MediaItemState::Failed,
            asked: Asked::Acquisition,
            preferred: None,
            jobs: &[
                (1, DownloadState::Removed, 1_000),
                (2, DownloadState::Failed, 1_000),
            ],
            files: &[],
            expected: Availability::Unavailable,
        },
        Case {
            what: "a same-instant tie is broken by the row id, higher id first",
            cached: MediaItemState::Failed,
            asked: Asked::Acquisition,
            preferred: None,
            jobs: &[
                (2, DownloadState::Failed, 1_000),
                (1, DownloadState::Removed, 1_000),
            ],
            files: &[],
            expected: Availability::Unavailable,
        },
        Case {
            what: "the newest terminal job is the greatest instant, not the greatest id",
            cached: MediaItemState::Failed,
            asked: Asked::Acquisition,
            preferred: None,
            jobs: &[
                (2, DownloadState::Failed, 1_000),
                (1, DownloadState::Removed, 2_000),
            ],
            files: &[],
            expected: Availability::Wanted,
        },
        Case {
            what: "a duplicate job row changes nothing",
            cached: MediaItemState::Failed,
            asked: Asked::Acquisition,
            preferred: None,
            jobs: &[
                (1, DownloadState::Failed, 1_000),
                (1, DownloadState::Failed, 1_000),
            ],
            files: &[],
            expected: Availability::Unavailable,
        },
        Case {
            what: "a one-file item with the file present is available, not partial",
            cached: MediaItemState::Available,
            asked: Asked::Nothing,
            preferred: Some(1),
            jobs: &[],
            files: &[(1, true)],
            expected: Availability::Available,
        },
        Case {
            what: "a one-file item with the file missing is missing, not partial",
            cached: MediaItemState::Available,
            asked: Asked::Nothing,
            preferred: Some(1),
            jobs: &[],
            files: &[(1, false)],
            expected: Availability::Missing,
        },
        Case {
            what: "a preferred file that is missing leaves the item partial",
            cached: MediaItemState::Partial,
            asked: Asked::Acquisition,
            preferred: Some(2),
            jobs: &[],
            files: &[(1, true), (2, false)],
            expected: Availability::Partial,
        },
        Case {
            what: "a preferred file id that names no row is ignored",
            cached: MediaItemState::Available,
            asked: Asked::Nothing,
            preferred: Some(99),
            jobs: &[],
            files: &[(1, true)],
            expected: Availability::Available,
        },
        Case {
            what: "a present copy outranks a row that records a failure",
            cached: MediaItemState::Failed,
            asked: Asked::Nothing,
            preferred: Some(1),
            jobs: &[(1, DownloadState::Failed, 1_000)],
            files: &[(1, true)],
            expected: Availability::Available,
        },
        Case {
            what: "a live upgrade does not make a playable item acquiring",
            cached: MediaItemState::Available,
            asked: Asked::Nothing,
            preferred: Some(1),
            jobs: &[(1, DownloadState::Downloading, 1_000)],
            files: &[(1, true)],
            expected: Availability::Available,
        },
        Case {
            what: "a file that reappears restores the item",
            cached: MediaItemState::Partial,
            asked: Asked::Nothing,
            preferred: Some(1),
            jobs: &[],
            files: &[(1, true), (2, true)],
            expected: Availability::Available,
        },
        Case {
            what: "a stale downloading row with no job is not a live acquisition",
            cached: MediaItemState::Downloading,
            asked: Asked::Acquisition,
            preferred: None,
            jobs: &[],
            files: &[],
            expected: Availability::Wanted,
        },
    ];

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millisecond(millis).expect("a millisecond inside the representable range")
    }

    fn text(value: &str) -> NonEmptyText {
        NonEmptyText::new(value).expect("the fixture text is not empty")
    }

    fn item(cached: MediaItemState, preferred: Option<i64>) -> MediaItem {
        MediaItem {
            id: 1,
            title_id: 1,
            episode_id: None,
            state: cached,
            origin: MediaOrigin::Torrent,
            preferred_file_id: preferred,
            quality_label: Some("1080p".to_owned()),
            created_at: at(1_700_000_000_000),
            updated_at: at(1_700_000_000_000),
        }
    }

    fn job(id: i64, state: DownloadState, millis: i64) -> ItemJob {
        ItemJob {
            id,
            state,
            added_at: at(millis),
        }
    }

    fn file(id: i64, present: bool) -> MediaFile {
        MediaFile {
            id,
            media_item_id: 1,
            path: text(&format!("/media/file-{id}.mkv")),
            root_id: None,
            size_bytes: 1,
            mtime_ms: at(1_690_000_000_000),
            added_at: at(1_695_000_000_000),
            missing_since: (!present).then(|| at(1_700_000_000_000)),
            container: None,
            duration_ms: None,
            video_codec: None,
            video_profile: None,
            width: None,
            height: None,
            bit_depth: None,
            hdr_format: None,
            frame_rate: None,
            bitrate_kbps: None,
            audio_codec: None,
            audio_channels: None,
            audio_layout: None,
            audio_languages: None,
            subtitle_format: None,
            probe_version: None,
            probe_json: None,
            quality_label: None,
            updated_at: at(1_700_000_000_000),
        }
    }

    /// The derivation over one table row, in the order the row declares its jobs.
    fn run(case: &Case) -> Availability {
        run_with(case, case.jobs)
    }

    fn run_with(case: &Case, jobs: &[(i64, DownloadState, i64)]) -> Availability {
        let jobs: Vec<ItemJob> = jobs
            .iter()
            .map(|(id, state, millis)| job(*id, *state, *millis))
            .collect();
        let files: Vec<MediaFile> = case
            .files
            .iter()
            .map(|(id, present)| file(*id, *present))
            .collect();
        availability(
            &item(case.cached, case.preferred),
            case.asked,
            &jobs,
            &files,
        )
    }

    /// Where a value sits in the diagram's order, for comparing whole sets of answers.
    fn position(value: Availability) -> usize {
        Availability::ALL
            .iter()
            .position(|candidate| *candidate == value)
            .expect("every value is in ALL")
    }

    /// The acceptance: every row of the table derives exactly what it declares.
    #[test]
    fn the_table_derives_every_combination_it_declares() {
        for case in CASES {
            assert_eq!(run(case), case.expected, "{}", case.what);
        }
    }

    /// Acceptance, the other half: the table reaches every state SM2 draws, so no value of
    /// [`Availability`] is a value nothing can produce.
    #[test]
    fn every_sm2_state_is_reachable_from_the_table() {
        let mut reached: Vec<Availability> = CASES.iter().map(run).collect();
        reached.sort_by_key(|value| position(*value));
        reached.dedup();
        assert_eq!(
            reached,
            Availability::ALL.to_vec(),
            "a state SM2 draws has no combination that derives it"
        );
    }

    /// Two rows with the same inputs and different expectations would hide a wrong one behind the
    /// other, so the table refuses to hold them.
    #[test]
    fn the_table_holds_no_duplicate_combination() {
        for (index, case) in CASES.iter().enumerate() {
            let repeated = CASES[..index].iter().any(|seen| {
                seen.cached == case.cached
                    && seen.asked == case.asked
                    && seen.preferred == case.preferred
                    && seen.jobs == case.jobs
                    && seen.files == case.files
            });
            assert!(!repeated, "{} repeats an earlier combination", case.what);
        }
    }

    /// The spellings are the diagram's node names, pinned here because this crate cannot read the
    /// document (AGENTS R6) — the same job the stored enums do for their `CHECK` clauses.
    #[test]
    fn the_spellings_are_the_diagram_node_names() {
        assert_eq!(
            Availability::ALL
                .iter()
                .map(|value| value.as_str())
                .collect::<Vec<_>>(),
            [
                "discovered",
                "watchlisted",
                "wanted",
                "acquiring",
                "available",
                "partial",
                "missing",
                "unavailable",
                "removed",
            ],
            "FLOWS SM2's node names changed"
        );
    }

    /// The stable spelling and the wire spelling are one spelling.
    #[test]
    fn every_value_round_trips_through_its_wire_spelling() {
        for value in Availability::ALL {
            let json = serde_json::to_string(&value).expect("serializes");
            assert_eq!(json, format!("\"{}\"", value.as_str()));
            assert_eq!(
                serde_json::from_str::<Availability>(&json).expect("deserializes"),
                value
            );
        }
        assert!(
            serde_json::from_str::<Availability>("\"failed\"").is_err(),
            "the stored column's word is not an availability value"
        );
    }

    /// The superset claim, as a test: the two enums share three spellings and the column keeps its
    /// own five, so deriving availability did not silently redefine a stored word.
    #[test]
    fn only_the_spellings_it_shares_with_the_column_are_stored_words() {
        let shared: Vec<&str> = Availability::ALL
            .iter()
            .map(|value| value.as_str())
            .filter(|spelling| MediaItemState::parse(spelling).is_some())
            .collect();
        assert_eq!(shared, ["wanted", "available", "partial"]);
        assert_eq!(
            MediaItemState::parse("downloading"),
            Some(MediaItemState::Downloading)
        );
        assert_eq!(
            MediaItemState::parse("failed"),
            Some(MediaItemState::Failed)
        );
        assert_eq!(MediaItemState::parse("unavailable"), None);
        assert_eq!(MediaItemState::parse("missing"), None);
    }

    #[test]
    fn an_item_with_no_files_and_no_jobs_answers_with_what_was_asked() {
        let bare = item(MediaItemState::Wanted, None);
        assert_eq!(
            availability(&bare, Asked::Nothing, &[], &[]),
            Availability::Discovered
        );
        assert_eq!(
            availability(&bare, Asked::Watchlisted, &[], &[]),
            Availability::Watchlisted
        );
        assert_eq!(
            availability(&bare, Asked::Acquisition, &[], &[]),
            Availability::Wanted
        );
    }

    #[test]
    fn a_retry_supersedes_the_failed_job_it_replaced() {
        let jobs = [
            job(1, DownloadState::Failed, 1_000),
            job(2, DownloadState::Downloading, 2_000),
        ];
        assert_eq!(
            availability(
                &item(MediaItemState::Failed, None),
                Asked::Acquisition,
                &jobs,
                &[]
            ),
            Availability::Acquiring,
            "the newest job is live, so the item is downloading again"
        );
    }

    #[test]
    fn a_newer_terminal_job_does_not_override_an_older_live_one() {
        let jobs = [
            job(1, DownloadState::Downloading, 1_000),
            job(2, DownloadState::Failed, 2_000),
        ];
        assert_eq!(
            availability(
                &item(MediaItemState::Wanted, None),
                Asked::Acquisition,
                &jobs,
                &[]
            ),
            Availability::Acquiring,
            "one attempt failing does not make an item unavailable while another runs"
        );
    }

    #[test]
    fn every_attempt_failed_reports_unavailable() {
        let jobs = [
            job(1, DownloadState::Failed, 1_000),
            job(2, DownloadState::Failed, 2_000),
        ];
        assert_eq!(
            availability(
                &item(MediaItemState::Failed, None),
                Asked::Acquisition,
                &jobs,
                &[]
            ),
            Availability::Unavailable
        );
    }

    #[test]
    fn a_missing_file_is_partial_only_while_another_copy_is_present() {
        let one_gone = [file(1, false)];
        assert_eq!(
            availability(
                &item(MediaItemState::Available, Some(1)),
                Asked::Nothing,
                &[],
                &one_gone
            ),
            Availability::Missing,
            "one row, one row missing, is missing"
        );
        let one_gone_of_two = [file(1, true), file(2, false)];
        assert_eq!(
            availability(
                &item(MediaItemState::Partial, Some(1)),
                Asked::Nothing,
                &[],
                &one_gone_of_two
            ),
            Availability::Partial,
            "the count decides, not merely that a row is absent"
        );
        let one_gone_of_three = [file(1, true), file(2, false), file(3, false)];
        assert_eq!(
            availability(
                &item(MediaItemState::Partial, Some(1)),
                Asked::Nothing,
                &[],
                &one_gone_of_three
            ),
            Availability::Partial
        );
    }

    #[test]
    fn a_preferred_file_that_is_missing_leaves_the_item_partial() {
        let files = [file(1, true), file(2, false)];
        let preferred_gone = item(MediaItemState::Partial, Some(2));
        assert_eq!(
            availability(&preferred_gone, Asked::Nothing, &[], &files),
            Availability::Partial,
            "the copy that plays is not the copy that was preferred"
        );
        let preferred_present = item(MediaItemState::Partial, Some(1));
        assert_eq!(
            availability(&preferred_present, Asked::Nothing, &[], &files),
            Availability::Partial,
            "which copy is preferred does not change what the item has"
        );
    }

    #[test]
    fn a_file_that_reappears_restores_the_item() {
        let row = item(MediaItemState::Available, Some(1));
        assert_eq!(
            availability(&row, Asked::Nothing, &[], &[file(1, false)]),
            Availability::Missing
        );
        assert_eq!(
            availability(&row, Asked::Nothing, &[], &[file(1, true)]),
            Availability::Available
        );
    }

    #[test]
    fn a_stale_row_never_overrides_the_evidence() {
        // A present file outranks a row that records a failure.
        assert_eq!(
            availability(
                &item(MediaItemState::Failed, None),
                Asked::Nothing,
                &[],
                &[file(1, true)]
            ),
            Availability::Available
        );
        // A row that records a download with no job behind it is stale: the job set is the
        // evidence that a transfer is running.
        assert_eq!(
            availability(
                &item(MediaItemState::Downloading, None),
                Asked::Acquisition,
                &[],
                &[]
            ),
            Availability::Wanted
        );
        // A row that still claims files whose rows are gone records the deletion, not the files.
        assert_eq!(
            availability(
                &item(MediaItemState::Available, None),
                Asked::Acquisition,
                &[],
                &[]
            ),
            Availability::Removed
        );
    }

    #[test]
    fn a_job_waiting_for_the_gate_is_still_acquiring() {
        let jobs = [job(1, DownloadState::Blocked, 1_000)];
        assert_eq!(
            availability(
                &item(MediaItemState::Wanted, None),
                Asked::Acquisition,
                &jobs,
                &[]
            ),
            Availability::Acquiring,
            "a refused torrent waits for the user in the job's own state, not as a failed item"
        );
    }

    #[test]
    fn how_the_item_came_to_exist_does_not_change_its_availability() {
        let files = [file(1, true)];
        for origin in MediaOrigin::ALL {
            let mut scanned = item(MediaItemState::Available, Some(1));
            scanned.origin = *origin;
            assert_eq!(
                availability(&scanned, Asked::Nothing, &[], &files),
                Availability::Available,
                "origin is history, not evidence of a copy"
            );
        }
    }

    #[test]
    fn reversing_the_jobs_of_every_table_row_changes_nothing() {
        for case in CASES {
            let forward = run(case);
            let mut reversed = case.jobs.to_vec();
            reversed.reverse();
            assert_eq!(
                run_with(case, &reversed),
                forward,
                "{} changed when its jobs were reversed",
                case.what
            );
        }
    }

    fn state_strategy() -> impl Strategy<Value = MediaItemState> {
        prop::sample::select(MediaItemState::ALL)
    }

    fn asked_strategy() -> impl Strategy<Value = Asked> {
        prop::sample::select(&ASKED[..])
    }

    fn job_state_strategy() -> impl Strategy<Value = DownloadState> {
        prop::sample::select(DownloadState::ALL)
    }

    fn job_strategy() -> impl Strategy<Value = (DownloadState, i64)> {
        (job_state_strategy(), 0i64..2_000_000)
    }

    fn file_strategy() -> impl Strategy<Value = (i64, bool)> {
        (0i64..8, any::<bool>())
    }

    /// The three asks, so a property can reach every branch that consults one.
    static ASKED: [Asked; 3] = [Asked::Nothing, Asked::Watchlisted, Asked::Acquisition];

    /// A generated job set, with the unique row ids a real `download_job` set has.
    fn jobs_from(entries: &[(DownloadState, i64)]) -> Vec<ItemJob> {
        entries
            .iter()
            .zip(1i64..)
            .map(|((state, millis), id)| job(id, *state, *millis))
            .collect()
    }

    fn files_from(entries: &[(i64, bool)]) -> Vec<MediaFile> {
        entries
            .iter()
            .map(|(id, present)| file(*id, *present))
            .collect()
    }

    proptest! {
        /// Totality: every combination of the three inputs derives a value, and which family of
        /// values it is in is decided by whether there is a file row at all.
        #[test]
        fn the_derivation_is_total_over_generated_combinations(
            cached in state_strategy(),
            asked in asked_strategy(),
            jobs in prop::collection::vec(job_strategy(), 0..6),
            files in prop::collection::vec(file_strategy(), 0..6),
        ) {
            let jobs = jobs_from(&jobs);
            let files = files_from(&files);
            let derived = availability(&item(cached, None), asked, &jobs, &files);
            prop_assert!(
                Availability::ALL.contains(&derived),
                "{derived:?} is not an availability"
            );
            let from_files = matches!(
                derived,
                Availability::Available | Availability::Partial | Availability::Missing
            );
            prop_assert_eq!(
                from_files,
                !files.is_empty(),
                "only a file row can answer with a file-derived state"
            );
        }

        /// The same inputs always derive the same answer: there is no clock, no ordering of the
        /// jobs, and no ambient state for a second call to see.
        #[test]
        fn the_same_inputs_always_derive_the_same_answer(
            cached in state_strategy(),
            asked in asked_strategy(),
            jobs in prop::collection::vec(job_strategy(), 0..6),
            files in prop::collection::vec(file_strategy(), 0..6),
        ) {
            let jobs = jobs_from(&jobs);
            let files = files_from(&files);
            prop_assert_eq!(
                availability(&item(cached, None), asked, &jobs, &files),
                availability(&item(cached, None), asked, &jobs, &files)
            );
        }

        /// Permuting the job slice changes nothing. The keys are arbitrary, so equal keys keep
        /// their relative order and the result is a permutation of the same multiset.
        #[test]
        fn a_shuffled_job_set_derives_the_same_answer(
            cached in state_strategy(),
            asked in asked_strategy(),
            entries in prop::collection::vec((any::<u8>(), job_state_strategy(), 0i64..2_000_000), 0..6),
            files in prop::collection::vec(file_strategy(), 0..6),
        ) {
            let declared: Vec<(DownloadState, i64)> = entries
                .iter()
                .map(|(_, state, millis)| (*state, *millis))
                .collect();
            let jobs = jobs_from(&declared);
            let mut keyed: Vec<(u8, ItemJob)> = entries
                .iter()
                .zip(jobs.iter())
                .map(|((key, _, _), job)| (*key, *job))
                .collect();
            keyed.sort_by_key(|(key, _)| *key);
            let shuffled: Vec<ItemJob> = keyed.into_iter().map(|(_, job)| job).collect();
            prop_assert_eq!(shuffled.len(), jobs.len());
            let files = files_from(&files);
            prop_assert_eq!(
                availability(&item(cached, None), asked, &jobs, &files),
                availability(&item(cached, None), asked, &shuffled, &files)
            );
        }

        /// A clock that moved on does not change the answer: shifting every instant by the same
        /// amount preserves the order the decision reads and nothing else about it is used.
        #[test]
        fn shifting_every_instant_does_not_change_the_answer(
            cached in state_strategy(),
            asked in asked_strategy(),
            entries in prop::collection::vec(job_strategy(), 0..6),
            files in prop::collection::vec(file_strategy(), 0..6),
            shift in 1i64..1_000_000,
        ) {
            let jobs = jobs_from(&entries);
            let shifted: Vec<ItemJob> = jobs
                .iter()
                .map(|job| ItemJob {
                    added_at: at(job.added_at.as_millisecond() + shift),
                    ..*job
                })
                .collect();
            let files = files_from(&files);
            prop_assert_eq!(
                availability(&item(cached, None), asked, &jobs, &files),
                availability(&item(cached, None), asked, &shifted, &files)
            );
        }
    }
}
