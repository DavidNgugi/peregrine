//! The library scan job: walking every root, and reconciling only what it finished walking.
//!
//! `docs/FLOWS.md` F8 gives the shape: the library service submits one `library.scan` job, the job
//! walks the roots with the ignore rules, and the reconciliation marks the files the walk did not
//! find. This module is that job, and it holds the acceptance `PG-E14-04` states — *cancelling
//! mid-scan leaves consistent state* — in one rule:
//!
//! **A root is reconciled only if its walk finished.** The reconciliation marks absent files
//! missing, so running it against a walk that stopped halfway would mark every file the walk had
//! not reached yet as missing. That is not a recoverable mistake dressed up as a small one: it is
//! the same "your library is gone" state a flat battery or an unplugged drive produces, and
//! `PG-E14-16` exists to keep it from happening. The scanner's refusal to return a partial outcome
//! ([`ScanError::Cancelled`](crate::library::ScanError::Cancelled)) is what makes the rule
//! enforceable rather than merely intended, and it is why this job treats every walk failure the
//! same way: report it, and reconcile nothing.
//!
//! A root that *did* finish is reconciled even when a later root is cancelled. That is consistent
//! by the same rule — each root's reconciliation is based on a complete walk of that root — and it
//! is also the honest reading of a cancel: work already finished is not undone.
//!
//! # Progress
//!
//! Roots, not files. How many files a tree holds is only known once it has been walked, so a
//! file-counted bar would have no denominator until the work was over. The roots are known before
//! the first walk starts, which is what makes `progress_permille` mean something here.

use std::sync::Arc;

use peregrine_domain::{Clock, ScanPolicy};
use peregrine_storage::{Database, JobChange, JobPhase, PageRequest};
use peregrine_sync::{Job, JobContext, JobError, JobFuture, JobOutput};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use ulid::Ulid;

use crate::events::{Event, EventBus, JOB_PROGRESS, JOB_STATE};
use crate::jobs::{self, CANCELLED_CODE};
use crate::library::{IndexReport, LibraryService, ScanError, Scanner, index_candidates};

/// The kind this job registers under, in the scheduler and in the journal.
///
/// The same string `docs/FLOWS.md` F8 journals and the UI branches on.
pub const KIND: &str = "library.scan";

/// Scheduling priority: the schema's default, so the journal and the queue agree.
///
/// A scan is background work: it is not urgent, and it must not starve playback or an
/// acquisition the user is waiting for. The per-kind limits in `PG-E6-03` are where that is
/// enforced; this is the priority it starts from.
pub const PRIORITY: i32 = 100;

/// How many directories one root's walk reads at once.
///
/// Eight, because the work is IO-bound and each unit is a directory listing plus a stat per entry:
/// enough to keep a network volume busy, few enough that a scan on a spinning disk does not
/// multiply its seek time by its concurrency. `PG-E14-14` is where this number meets a budget.
pub const WALK_CONCURRENCY: usize = 8;

/// How many roots one scan reads.
///
/// A page rather than every root, because the page size is the service's and a library with
/// thousands of roots is a misconfiguration rather than a case to optimise for. The rest are
/// scanned by the next job.
pub const ROOT_PAGE: u32 = 100;

impl ScanSummary {
    /// Folds one root's finished work in.
    fn record(
        &mut self,
        files: usize,
        report: &crate::library::Reconciliation,
        indexed: &IndexReport,
    ) {
        self.roots_scanned += 1;
        self.files_found += files;
        self.marked_missing += report.marked_missing.len();
        self.restored += report.restored.len();
        self.indexed.created_titles += indexed.created_titles;
        self.indexed.created_items += indexed.created_items;
        self.indexed.created_files += indexed.created_files;
        self.indexed.refreshed_files += indexed.refreshed_files;
        self.indexed.unmatched += indexed.unmatched;
    }
}

/// What one finished scan found.
///
/// The payload a caller reads from the job, and what a summary line is built from. Roots are
/// counted separately from files because "nothing was scanned" and "a scan found nothing" are
/// different answers: the first is a root whose walk failed, and the second is an empty folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanSummary {
    /// How many roots the walk finished.
    pub roots_scanned: usize,
    /// How many roots the walk did not finish, with the reason.
    pub roots_failed: Vec<(i64, String)>,
    /// How many files the completed walks found.
    pub files_found: usize,
    /// How many files the reconciliation marked missing.
    pub marked_missing: usize,
    /// How many files it found again and cleared.
    pub restored: usize,
    /// What the indexing wrote for the roots it walked.
    pub indexed: IndexReport,
}

/// What one root contributed to the summary.
enum RootOutcome {
    /// The walk finished and the reconciliation ran.
    Scanned {
        /// How many candidates the walk found.
        files: usize,
        /// What the reconciliation changed.
        report: crate::library::Reconciliation,
        /// What the indexing wrote.
        indexed: IndexReport,
    },
    /// The root was not scanned, with the reason a log line carries.
    Failed(String),
    /// Cancellation was observed while this root was being walked.
    Cancelled,
}

/// A job that walks every library root and reconciles what it finds.
pub struct LibraryScanJob {
    database: Arc<Database>,
    bus: Arc<EventBus>,
    clock: Arc<dyn Clock + Send + Sync>,
    /// The journal row this job reports into.
    journal_id: i64,
    /// Whether the running attempt has already been reported to the journal.
    reported_start: bool,
    policy: ScanPolicy,
}

impl LibraryScanJob {
    /// A scan job for the journal row `journal_id`.
    #[must_use]
    pub fn new(
        database: Arc<Database>,
        bus: Arc<EventBus>,
        clock: Arc<dyn Clock + Send + Sync>,
        journal_id: i64,
    ) -> Self {
        Self {
            database,
            bus,
            clock,
            journal_id,
            reported_start: false,
            policy: ScanPolicy::new(),
        }
    }

    /// Walks every root on the page, reconciling each one whose walk finished.
    async fn attempt(
        &mut self,
        job: Ulid,
        attempt: u32,
        cancellation: CancellationToken,
    ) -> Result<JobOutput, JobError> {
        if !self.reported_start {
            self.reported_start = true;
            self.record(
                job,
                JobChange::Started {
                    at: self.clock.now().as_millisecond(),
                    attempt: i64::from(attempt),
                },
            )
            .await;
            self.publish_state(job, JobPhase::Running, attempt);
            info!(job = %job, kind = KIND, "the library scan started");
        }

        let roots = match self
            .database
            .list_library_roots(PageRequest::first(ROOT_PAGE))
            .await
        {
            Ok(roots) => roots,
            Err(error) => {
                self.record(
                    job,
                    JobChange::Finished {
                        at: self.clock.now().as_millisecond(),
                        phase: JobPhase::Failed,
                        error_code: Some(jobs::INTERNAL_CODE.to_owned()),
                        error: Some(error.to_string()),
                    },
                )
                .await;
                return Err(JobError::Failed {
                    message: "the library roots could not be read".to_owned(),
                });
            }
        };

        let service = LibraryService::new(Arc::clone(&self.database));
        let scanner = Scanner::new(self.policy);
        let mut summary = ScanSummary::default();
        let total = roots.len().max(1);

        for (index, row) in roots.iter().enumerate() {
            if cancellation.is_cancelled() {
                self.finish_cancelled(job, attempt).await;
                return Err(JobError::Cancelled);
            }

            match self
                .scan_root(
                    &service,
                    scanner,
                    row.id,
                    row.root.path.as_str(),
                    &cancellation,
                )
                .await
            {
                RootOutcome::Scanned {
                    files,
                    report,
                    indexed,
                } => summary.record(files, &report, &indexed),
                RootOutcome::Failed(reason) => {
                    warn!(job = %job, root_id = row.id, %reason, "a root was not scanned");
                    summary.roots_failed.push((row.id, reason));
                }
                RootOutcome::Cancelled => {
                    self.finish_cancelled(job, attempt).await;
                    return Err(JobError::Cancelled);
                }
            }
            self.publish_progress(job, index + 1, total, attempt);
        }

        let at = self.clock.now().as_millisecond();
        self.record(
            job,
            JobChange::Finished {
                at,
                phase: JobPhase::Succeeded,
                error_code: None,
                error: None,
            },
        )
        .await;
        self.publish_state(job, JobPhase::Succeeded, attempt);
        info!(
            job = %job,
            kind = KIND,
            roots = summary.roots_scanned,
            failed = summary.roots_failed.len(),
            files = summary.files_found,
            indexed = summary.indexed.created_files,
            titles = summary.indexed.created_titles,
            unmatched = summary.indexed.unmatched,
            missing = summary.marked_missing,
            restored = summary.restored,
            "the library scan finished"
        );
        Ok(Box::new(summary))
    }

    /// Walks one root and reconciles it if the walk finished.
    ///
    /// The whole of `PG-E14-04`'s rule is in the two failure arms: a walk that did not finish
    /// reconciles nothing, because the reconciliation marks absent files missing and a walk that
    /// stopped halfway has not established that anything is absent. A walk that finished and a
    /// reconciliation that failed is a different answer again — the root is reported rather than
    /// counted, and the next scan tries it again.
    async fn scan_root(
        &self,
        service: &LibraryService,
        scanner: Scanner,
        root_id: i64,
        path: &str,
        cancellation: &CancellationToken,
    ) -> RootOutcome {
        let outcome = match scanner
            .scan_bounded(std::path::Path::new(path), WALK_CONCURRENCY, cancellation)
            .await
        {
            Ok(outcome) => outcome,
            Err(ScanError::Cancelled) => return RootOutcome::Cancelled,
            Err(error) => return RootOutcome::Failed(error.to_string()),
        };

        // Index first, reconcile second: the reconciliation is a statement about the rows the
        // indexing writes, and running it the other way round would mark a file missing and then
        // index it in the same pass.
        let indexed = match index_candidates(
            &self.database,
            root_id,
            &outcome.candidates,
            self.clock.now(),
        )
        .await
        {
            Ok(report) => report,
            Err(error) => return RootOutcome::Failed(error.to_string()),
        };

        match service
            .reconcile(root_id, &outcome.candidates, self.clock.now())
            .await
        {
            Ok(report) => RootOutcome::Scanned {
                files: outcome.candidates.len(),
                report,
                indexed,
            },
            Err(error) => RootOutcome::Failed(error.to_string()),
        }
    }

    /// Publishes how many roots are done, at the IPC budget's rate rather than per root.
    fn publish_progress(&self, job: Ulid, done: usize, total: usize, attempt: u32) {
        let permille = permille_of(done, total);
        self.bus.publish(Event::new(
            JOB_PROGRESS,
            job.to_string(),
            jobs::progress(job, self.journal_id, permille, attempt),
        ));
    }

    /// Applies one change to this job's journal row.
    ///
    /// A journal failure is logged and does not fail the work, for the reason the hello job gives:
    /// refusing to scan because the record could not be written would turn a full disk into a
    /// library that never updates.
    async fn record(&self, job: Ulid, change: JobChange) {
        match self.database.update_job(self.journal_id, change).await {
            Ok(true) => {}
            Ok(false) => warn!(
                job = %job,
                journal_id = self.journal_id,
                "the journal row is already terminal; the change was dropped"
            ),
            Err(error) => {
                warn!(job = %job, journal_id = self.journal_id, %error, "the journal write failed");
            }
        }
    }

    /// Publishes the phase the journal row already carries.
    fn publish_state(&self, job: Ulid, phase: JobPhase, attempt: u32) {
        self.bus.publish(Event::new(
            JOB_STATE,
            job.to_string(),
            jobs::state(job, self.journal_id, KIND, phase, attempt),
        ));
    }

    /// Records that the job stopped because it was asked to.
    async fn finish_cancelled(&self, job: Ulid, attempt: u32) {
        self.record(
            job,
            JobChange::Finished {
                at: self.clock.now().as_millisecond(),
                phase: JobPhase::Canceled,
                error_code: Some(CANCELLED_CODE.to_owned()),
                error: None,
            },
        )
        .await;
        self.publish_state(job, JobPhase::Canceled, attempt);
        info!(job = %job, kind = KIND, "the library scan stopped on request");
    }
}

impl Job for LibraryScanJob {
    fn run(&mut self, context: &JobContext<'_>) -> JobFuture<'_> {
        let job = context.id();
        let attempt = context.attempt();
        let cancellation = context.cancellation().clone();
        Box::pin(self.attempt(job, attempt, cancellation))
    }
}

/// Completion in permille, clamped so a finished scan always reports exactly 1000.
fn permille_of(done: usize, total: usize) -> u16 {
    if total == 0 || done >= total {
        return 1000;
    }
    u16::try_from(done.saturating_mul(1000) / total).unwrap_or(1000)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use peregrine_domain::{
        AccountId, AccountScope, LibraryRoot, LibraryRootKind, MediaFile, MediaItem,
        MediaItemState, MediaOrigin, NonEmptyText, ScanMode, TitleKind, TitleSubtype,
    };
    use peregrine_storage::{JobRecord, NewJob, StubTitle};
    use peregrine_sync::{JobHandle, JobOutcome, JobSpec, Scheduler, SchedulerConfig};
    use peregrine_testkit::{CountingIds, FakeClock};
    use tempfile::TempDir;

    use crate::events::EventSink;
    use crate::testing::RecordingSink;

    use super::*;

    const NOW: i64 = 1_700_000_000_000;

    fn clock() -> Arc<dyn Clock + Send + Sync> {
        Arc::new(FakeClock::at(NOW))
    }

    fn text(value: &str) -> NonEmptyText {
        NonEmptyText::new(value).expect("fixture text is not empty")
    }

    /// The summary a successful scan answers with.
    ///
    /// Downcasting is how the scheduler stays generic over payloads (it carries `Box<dyn Any>` and
    /// never inspects it), so a test that reads one has to name the type it expects.
    fn summary_of(outcome: &JobOutcome) -> ScanSummary {
        match outcome {
            JobOutcome::Succeeded(output) => output
                .downcast_ref::<ScanSummary>()
                .expect("a scan succeeds with a summary")
                .clone(),
            other => panic!("expected a successful scan, got {other:?}"),
        }
    }

    /// The fixture instant, as the journal and the domain both carry it.
    fn at(millis: i64) -> jiff::Timestamp {
        jiff::Timestamp::from_millisecond(millis).expect("the tests stay inside jiff's range")
    }

    /// A library of one root, one item, and one film on disk.
    struct Library {
        directory: TempDir,
        database: Arc<Database>,
        bus: Arc<EventBus>,
        sink: Arc<RecordingSink>,
        root_id: i64,
        file_id: i64,
        film: PathBuf,
    }

    async fn library(name: &str, folders: usize) -> Library {
        let directory = TempDir::new().unwrap();
        let media = directory.path().join(format!("Movies-{name}"));
        let film = media.join("Alien (1979).mkv");
        fs::create_dir_all(&media).unwrap();
        fs::write(&film, b"a film").unwrap();

        // The folders are what makes a walk take long enough for a cancel to land inside it.
        for index in 0..folders {
            let nested = media.join(format!("Collection {index}"));
            fs::create_dir_all(&nested).unwrap();
            for part in 0..8 {
                fs::write(nested.join(format!("Feature {part}.mkv")), b"a film").unwrap();
            }
        }

        let database = Arc::new(
            Database::open(directory.path().join("peregrine.sqlite"))
                .await
                .unwrap(),
        );
        let root_id = database
            .insert_library_root(LibraryRoot {
                id: 0,
                path: text(media.to_str().unwrap()),
                kind: LibraryRootKind::Movies,
                scan_mode: ScanMode::Incremental,
                last_scan_at: None,
                created_at: at(NOW),
            })
            .await
            .unwrap();
        let title_id = database
            .insert_stub_title(StubTitle {
                kind: TitleKind::Movie,
                subtype: TitleSubtype::Standard,
                name: "Alien".to_owned(),
                original_name: None,
                sort_name: "alien".to_owned(),
                at: NOW,
            })
            .await
            .unwrap();
        let item_id = database
            .insert_media_item(MediaItem {
                id: 0,
                title_id,
                episode_id: None,
                state: MediaItemState::Available,
                origin: MediaOrigin::Scan,
                preferred_file_id: None,
                quality_label: None,
                created_at: at(NOW),
                updated_at: at(NOW),
            })
            .await
            .unwrap();
        let file_id = database
            .insert_media_file(MediaFile {
                id: 0,
                media_item_id: item_id,
                path: text(film.to_str().unwrap()),
                root_id: Some(root_id),
                size_bytes: fs::metadata(&film).unwrap().len(),
                mtime_ms: at(NOW),
                added_at: at(NOW),
                missing_since: None,
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
                updated_at: at(NOW),
            })
            .await
            .unwrap();

        let sink = Arc::new(RecordingSink::default());
        let bus = Arc::new(EventBus::new(Arc::clone(&sink) as Arc<dyn EventSink>));
        Library {
            directory,
            database,
            bus,
            sink,
            root_id,
            file_id,
            film,
        }
    }

    impl Library {
        async fn journal_row(&self, id: i64) -> JobRecord {
            self.database
                .job(id)
                .await
                .unwrap()
                .expect("the row exists")
        }

        async fn row(&self) -> MediaFile {
            self.database
                .media_file(
                    AccountScope::unrestricted(AccountId::new(1).expect("account one")),
                    self.file_id,
                )
                .await
                .unwrap()
                .expect("the row is never deleted")
        }

        /// Submits a scan and answers the scheduler and its handle.
        async fn start(&self) -> (Scheduler, JobHandle, i64) {
            let journal_id = self
                .database
                .insert_job(NewJob {
                    kind: KIND.to_owned(),
                    priority: PRIORITY,
                    dedupe_key: None,
                    payload_json: "{}".to_owned(),
                    max_attempts: 1,
                    created_at: NOW,
                })
                .await
                .unwrap();
            let scheduler = Scheduler::new(SchedulerConfig::new(
                clock(),
                Arc::new(CountingIds::default()),
            ));
            let job = LibraryScanJob::new(
                Arc::clone(&self.database),
                Arc::clone(&self.bus),
                clock(),
                journal_id,
            );
            let handle = scheduler
                .submit(
                    JobSpec {
                        kind: KIND.to_owned(),
                        priority: PRIORITY,
                        dedupe_key: None,
                        max_attempts: 1,
                    },
                    job,
                )
                .await
                .unwrap();
            (scheduler, handle, journal_id)
        }
    }

    #[tokio::test]
    async fn a_finished_scan_reconciles_the_root_it_walked() {
        // The ordinary path, and the one that proves the rule is not "never reconcile": the film is
        // gone from disk, the walk finishes, and the row is stamped missing rather than deleted.
        let library = library("finished", 4).await;
        fs::remove_file(&library.film).unwrap();

        let (_scheduler, handle, journal_id) = library.start().await;
        let completion = handle.wait().await;

        assert!(
            matches!(completion.outcome(), JobOutcome::Succeeded(_)),
            "{:?}",
            completion.outcome()
        );
        let row = library.row().await;
        assert_eq!(
            row.missing_since,
            Some(at(NOW)),
            "the walk finished, so its reconciliation is the library's answer"
        );
        assert!(
            library.journal_row(journal_id).await.state.is_terminal(),
            "and the journal row closed"
        );
    }

    #[tokio::test]
    async fn a_cancelled_scan_never_stamps_a_file_missing() {
        // PG-E14-04's acceptance. The film is gone from disk, so a reconciliation would stamp it —
        // and the point is that a cancelled walk must not reach one. The assertion is conditional on
        // the terminal state because a cancel's timing is not something a test can pin: what it
        // pins is the *rule*, which is that a missing mark appears only behind a finished walk.
        let library = library("cancelled", 200).await;
        fs::remove_file(&library.film).unwrap();

        let (_scheduler, mut handle, journal_id) = library.start().await;
        handle.cancel();

        let completion = handle.wait().await;
        let row = library.row().await;
        match completion.outcome() {
            JobOutcome::Cancelled => {
                assert_eq!(
                    row.missing_since, None,
                    "a cancelled walk reached no reconciliation, so nothing is missing"
                );
                assert_eq!(
                    library
                        .journal_row(journal_id)
                        .await
                        .last_error_code
                        .as_deref(),
                    Some(CANCELLED_CODE)
                );
            }
            JobOutcome::Succeeded(_) => {
                assert_eq!(
                    row.missing_since,
                    Some(at(NOW)),
                    "the walk finished before the cancel landed, so the scan did its job"
                );
            }
            other @ JobOutcome::Failed(_) => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_root_that_cannot_be_walked_is_reported_and_changes_nothing() {
        // The same rule reached the other way, and deterministically: a root whose directory is not
        // there fails its walk, so nothing about it is reconciled. This is the unplugged-drive case
        // PG-E14-16 owns, held here at the layer that could break it.
        let library = library("unreachable", 0).await;
        fs::remove_dir_all(library.directory.path().join("Movies-unreachable")).unwrap();

        let (_scheduler, handle, _journal_id) = library.start().await;
        let completion = handle.wait().await;

        assert!(
            matches!(completion.outcome(), JobOutcome::Succeeded(_)),
            "a scan that could not read its only root still completes, reporting it: {:?}",
            completion.outcome()
        );
        let summary = summary_of(completion.outcome());
        assert_eq!(summary.roots_failed.len(), 1, "{summary:?}");
        assert_eq!(summary.roots_scanned, 0);
        assert_eq!(
            library.row().await.missing_since,
            None,
            "a library that could not be read is not a library whose files are gone"
        );
    }

    #[tokio::test]
    async fn a_library_with_no_roots_completes_and_says_so() {
        // The empty case: no roots is not a failure, and the summary has to be able to say "nothing
        // was scanned" without pretending something was.
        let library = library("no-roots", 0).await;
        library
            .database
            .remove_library_root(library.root_id)
            .await
            .unwrap();

        let (_scheduler, handle, journal_id) = library.start().await;
        let completion = handle.wait().await;

        assert_eq!(summary_of(completion.outcome()), ScanSummary::default());
        assert!(matches!(completion.outcome(), JobOutcome::Succeeded(_)));
        assert!(library.journal_row(journal_id).await.state.is_terminal());
    }

    #[tokio::test]
    async fn a_scan_reports_its_roots_as_progress_and_publishes_the_terminal_phase() {
        // Progress counts roots, because how many files a tree holds is only known once it has been
        // walked. Two roots and a limit of one page means the second half of the work is visible.
        let library = library("progress", 2).await;
        let second = library.directory.path().join("Shows-progress");
        fs::create_dir_all(&second).unwrap();
        fs::write(second.join("S01E01.mkv"), b"an episode").unwrap();
        library
            .database
            .insert_library_root(LibraryRoot {
                id: 0,
                path: text(second.to_str().unwrap()),
                kind: LibraryRootKind::Shows,
                scan_mode: ScanMode::Incremental,
                last_scan_at: None,
                created_at: at(NOW),
            })
            .await
            .unwrap();

        let (_scheduler, handle, _journal_id) = library.start().await;
        let completion = handle.wait().await;
        library.bus.shutdown().await;

        assert!(matches!(completion.outcome(), JobOutcome::Succeeded(_)));
        let delivered = library.sink.delivered();
        let phases: Vec<String> = delivered
            .iter()
            .filter_map(|event| event.payload["phase"].as_str())
            .map(str::to_owned)
            .collect();
        // Only the terminal phase is asserted. A scan of a library this size finishes inside one
        // coalescing window, and the bus is *supposed* to supersede a `running` update with the
        // terminal one rather than deliver both — asserting otherwise would pin the event count,
        // which is the bus's business and not this job's.
        assert_eq!(
            phases.last().map(String::as_str),
            Some("succeeded"),
            "{phases:?}"
        );
        let progress: Vec<u64> = delivered
            .iter()
            .filter_map(|event| event.payload["progressPermille"].as_u64())
            .collect();
        assert!(
            progress.contains(&1000) || progress.last().copied() == Some(500),
            "roots are counted as they finish: {progress:?}"
        );
    }
}
