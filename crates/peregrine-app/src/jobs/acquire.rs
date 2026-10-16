//! Auto-acquiring one title: find the best release on the configured indexer and queue it.
//!
//! A job rather than a command because it leaves the machine three times — a Prowlarr search, a
//! metadata fetch over BitTorrent, and the engine's own add — and ARCHITECTURE §7 rule 2 puts any
//! of those past a command's budget. The selection itself is pure and lives in peregrine-domain
//! (rank::best_candidate); this file only supplies the candidates and the profile.

use std::sync::Arc;

use peregrine_domain::rank::{Candidate, best_candidate};
use peregrine_domain::{AccountScope, Clock, NonEmptyText, ReleaseName};
use peregrine_indexers::{IndexerRelease, TorrentSource as IndexerSource};
use peregrine_secrets::SecretStore;
use peregrine_storage::{AddedBy, Database, DownloadSource, JobChange, MagnetUri, NewDownloadJob};
use peregrine_sync::{Job, JobContext, JobError, JobFuture};
use peregrine_torrent::{TorrentMetadataFetch, TorrentSource as EngineSource};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::acquisition::{AcquisitionService, StartRequest};
use crate::events::{Event, EventBus, JOB_PROGRESS};
use crate::indexers;

/// The kind this job registers under, in the scheduler and in the journal.
pub const KIND: &str = "acquisition.acquire";

/// The default priority: a person asked for this download, so it is the work a user just asked for
/// rather than background maintenance.
pub const PRIORITY: i32 = 5;

/// How many indexer results a search asks for.
const SEARCH_LIMIT: u32 = 100;

/// Which title to acquire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    /// The catalogue row.
    pub title_id: i64,
}

/// The dedupe key: one live acquire per title.
#[must_use]
pub fn dedupe_key(title_id: i64) -> String {
    format!("acquisition.acquire:{title_id}")
}

/// Finds one title's best release and hands it to the acquisition service.
pub struct AcquireJob {
    database: Arc<Database>,
    bus: Arc<EventBus>,
    store: Arc<dyn SecretStore>,
    metadata: Arc<dyn TorrentMetadataFetch>,
    acquisition: Arc<AcquisitionService>,
    clock: Arc<dyn Clock + Send + Sync>,
    account: AccountScope,
    journal_id: i64,
    target: Target,
    min_seeders: u32,
}

impl AcquireJob {
    /// A job that acquires one title under one account.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        database: Arc<Database>,
        bus: Arc<EventBus>,
        store: Arc<dyn SecretStore>,
        metadata: Arc<dyn TorrentMetadataFetch>,
        acquisition: Arc<AcquisitionService>,
        clock: Arc<dyn Clock + Send + Sync>,
        account: AccountScope,
        journal_id: i64,
        target: Target,
        min_seeders: u32,
    ) -> Self {
        Self {
            database,
            bus,
            store,
            metadata,
            acquisition,
            clock,
            account,
            journal_id,
            target,
            min_seeders,
        }
    }

    /// Reports progress, which a Downloads card follows.
    async fn phase(&self, job: ulid::Ulid, attempt: u32, permille: u16) {
        if let Err(error) = self
            .database
            .update_job(
                self.journal_id,
                JobChange::Progress {
                    permille: i64::from(permille),
                },
            )
            .await
        {
            debug!(%error, "job progress could not be journalled");
        }
        self.bus.publish(Event::new(
            JOB_PROGRESS,
            job.to_string(),
            crate::jobs::progress(job, self.journal_id, permille, attempt),
        ));
    }

    /// The work, owning everything it borrows.
    async fn acquire(
        &mut self,
        job: ulid::Ulid,
        attempt: u32,
        cancellation: CancellationToken,
    ) -> peregrine_sync::JobResult {
        self.phase(job, attempt, 50).await;

        let identity = self
            .database
            .title_identity(self.account, self.target.title_id)
            .await
            .map_err(|error| JobError::failed(format!("the title could not be read: {error}")))?
            .ok_or_else(|| JobError::failed("that title is not in this account's library"))?;

        let profile = self
            .database
            .default_quality_profile()
            .await
            .map_err(|error| {
                JobError::failed(format!("the quality profile could not be read: {error}"))
            })?
            .ok_or_else(|| {
                JobError::failed("no quality profile is set, so no release can be chosen")
            })?;

        self.phase(job, attempt, 200).await;

        let releases =
            indexers::search_async(Arc::clone(&self.store), identity.name.clone(), SEARCH_LIMIT)
                .await
                .map_err(|error| match error {
                    indexers::IndexerConfigError::Unconfigured => {
                        JobError::failed("no indexer is configured")
                    }
                    other => JobError::failed(format!("the indexer search failed: {other}")),
                })?;

        if cancellation.is_cancelled() {
            return Err(JobError::Cancelled);
        }

        // The seeder floor and the parse happen here, before the pure ranking rule sees anything: a
        // result with too few seeds is not a candidate, and one whose name will not parse is not one
        // either.
        let mut candidates: Vec<Candidate> = Vec::new();
        let mut kept: Vec<&IndexerRelease> = Vec::new();
        for release in &releases {
            if release.seeders.unwrap_or(0) < self.min_seeders {
                continue;
            }
            candidates.push(Candidate::new(
                ReleaseName::parse(&release.raw_title),
                release.size_bytes.unwrap_or(0),
            ));
            kept.push(release);
        }

        let Some(index) = best_candidate(&candidates, &profile) else {
            return Err(JobError::failed(
                "no release the quality profile accepts met the seeder floor",
            ));
        };
        let chosen = kept[index];

        self.phase(job, attempt, 500).await;

        // A magnet is what the metadata fetch and the engine both take; an info hash is completed
        // into one, because every indexer that reports a bare hash means the same urn:btih form.
        let magnet_uri = match &chosen.source {
            IndexerSource::Magnet(uri) => uri.clone(),
            IndexerSource::InfoHash(hash) => format!("magnet:?xt=urn:btih:{hash}"),
        };
        let engine_source =
            EngineSource::Magnet(NonEmptyText::new(&magnet_uri).map_err(|error| {
                JobError::failed(format!("the release's magnet is not usable: {error}"))
            })?);

        let fetched = self
            .metadata
            .fetch_metadata(&engine_source)
            .await
            .map_err(|error| {
                JobError::failed(format!(
                    "the torrent metadata could not be fetched: {error}"
                ))
            })?;

        if cancellation.is_cancelled() {
            return Err(JobError::Cancelled);
        }

        self.phase(job, attempt, 900).await;

        let save_path = NonEmptyText::new(&folder_for(&identity.name)).map_err(|error| {
            JobError::failed(format!("the destination folder is not usable: {error}"))
        })?;
        let row_source =
            DownloadSource::Magnet(MagnetUri::new(NonEmptyText::new(&magnet_uri).map_err(
                |error| JobError::failed(format!("the release's magnet is not usable: {error}")),
            )?));

        let job_row = NewDownloadJob {
            title_id: self.target.title_id,
            episode_id: None,
            release_title: chosen.raw_title.clone(),
            release_source: None,
            release_group: None,
            resolution: None,
            video_codec: None,
            hdr_format: None,
            audio_summary: None,
            reported_size_bytes: chosen.size_bytes,
            info_hash: None,
            source: row_source,
            save_path,
            priority: 100,
            added_by: AddedBy::Auto,
            added_at: self.clock.now(),
        };

        let request = StartRequest {
            account: self.account,
            job: job_row,
            description: fetched.description().clone(),
            info_hash: fetched.info_hash(),
        };

        let started = self
            .acquisition
            .start(request)
            .await
            .map_err(|error| JobError::failed(error.to_string()))?;

        self.phase(job, attempt, 1000).await;
        Ok(Box::new(started.job.id))
    }
}

impl Job for AcquireJob {
    fn run(&mut self, context: &JobContext<'_>) -> JobFuture<'_> {
        let job = context.id();
        let attempt = context.attempt();
        let cancellation = context.cancellation().clone();
        Box::pin(async move {
            let outcome = self.acquire(job, attempt, cancellation).await;
            // The scheduler's journal line says only "failed"; the sentence a person needs — no
            // profile, no indexer, no acceptable release — is here or nowhere. These messages are
            // built to carry no secret, magnet, or path.
            if let Err(error) = &outcome {
                warn!(%error, "the acquire job failed");
            }
            outcome
        })
    }
}

/// A safe single-segment folder for a release, derived from the title's name.
///
/// The guard would reject a traversal, but a title whose name contains a separator should not decide
/// the shape of the download tree at all, so separators become underscores here.
fn folder_for(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c == '/' || c.is_control() { '_' } else { c })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "Downloads".to_owned()
    } else {
        trimmed.to_owned()
    }
}
