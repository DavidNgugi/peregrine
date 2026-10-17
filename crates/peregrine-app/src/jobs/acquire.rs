//! Auto-acquiring one title: find the best release on the configured indexer and queue it.
//!
//! A job rather than a command because it leaves the machine three times — a Prowlarr search, a
//! metadata fetch over `BitTorrent`, and the engine's own add — and ARCHITECTURE §7 rule 2 puts any
//! of those past a command's budget. The selection itself is pure and lives in peregrine-domain
//! (`rank::best_candidate`); this file only supplies the candidates and the profile.

use std::sync::Arc;

use peregrine_domain::rank::{Candidate, ReleaseRanker, alternative_candidates, ranked_candidates};
use peregrine_domain::{
    AccountScope, Clock, DEFAULT_HD_CEILING_BYTES, NonEmptyText, ReleaseName, Resolution, Verdict,
};
use peregrine_indexers::{FetchedTorrent, IndexerRelease, TorrentSource as IndexerSource};
use peregrine_secrets::SecretStore;
use peregrine_storage::{
    AddedBy, Database, DownloadSource, JobChange, JobPhase, MagnetUri, NewDownloadJob, NewOffer,
};
use peregrine_sync::{Job, JobContext, JobError, JobFuture};
use peregrine_torrent::{TorrentMetadataFetch, TorrentSource as EngineSource};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::acquisition::{AcquisitionService, StartRequest};
use crate::events::{Event, EventBus, JOB_PROGRESS};
use crate::indexers;
use crate::notify::NotifyOffers;

/// The kind this job registers under, in the scheduler and in the journal.
pub const KIND: &str = "acquisition.acquire";

/// The default priority: a person asked for this download, so it is the work a user just asked for
/// rather than background maintenance.
pub const PRIORITY: i32 = 5;

/// How many indexer results a search asks for.
const SEARCH_LIMIT: u32 = 100;

/// How many ranked candidates one acquire will try before it reports the last refusal.
///
/// Each try costs a metadata fetch over `BitTorrent` — seconds to a minute — so this bounds one user
/// action while still stepping past a handful of releases the gate refuses: a size the profile
/// allowed but the volume cannot hold, a name the guard refuses, a swarm with no peers left.
const MAX_CANDIDATES_TRIED: usize = 5;

/// A release the user picked from the offers.
///
/// Carried into the job so the chosen release is acquired without a second search; the safety
/// gate and the free-space admission still run, because a manual choice is not a safety bypass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChosenOffer {
    /// The release name as the indexer stated it.
    pub raw_title: String,
    /// Which indexer it came from.
    pub indexer: Option<String>,
    /// Its stated size.
    pub size_bytes: Option<u64>,
    /// Its stated seeders.
    pub seeders: Option<u32>,
    /// Where its bytes are fetched from, looked up from the row rather than sent over IPC.
    pub source: IndexerSource,
}

/// One episode to acquire rather than the whole title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpisodePick {
    /// The season the episode belongs to.
    pub season: u32,
    /// The episode's number within that season.
    pub episode: u32,
}

/// Which title to acquire, which episode of it, and the release to acquire when the user has chosen
/// one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The catalogue row.
    pub title_id: i64,
    /// The one episode to acquire, or None to acquire the title as a whole.
    pub episode: Option<EpisodePick>,
    /// The release the user picked, or None to search.
    pub chosen: Option<ChosenOffer>,
}

/// The dedupe key: one live acquire per title, and per episode when one is named.
#[must_use]
pub fn dedupe_key(title_id: i64, episode: Option<EpisodePick>) -> String {
    match episode {
        Some(pick) => format!(
            "acquisition.acquire:{title_id}:s{:02}e{:02}",
            pick.season, pick.episode
        ),
        None => format!("acquisition.acquire:{title_id}"),
    }
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
    /// The one notification this job raises, so the job does not name the service's four ports.
    offers: Arc<dyn NotifyOffers>,
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
        offers: Arc<dyn NotifyOffers>,
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
            offers,
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

    /// Records the typed failure on this job's own row, so the page that follows it can read the
    /// reason back. The scheduler's journal line carries only "failed", which is not a sentence a
    /// person can act on.
    async fn record_failure(&self, error: &JobError) {
        let cancelled = matches!(error, JobError::Cancelled);
        let phase = if cancelled {
            JobPhase::Canceled
        } else {
            JobPhase::Failed
        };
        let code = if cancelled {
            "cancelled".to_owned()
        } else {
            "acquisition_failed".to_owned()
        };
        if let Err(update) = self
            .database
            .update_job(
                self.journal_id,
                JobChange::Finished {
                    at: self.clock.now().as_millisecond(),
                    phase,
                    error_code: Some(code),
                    error: Some(error.to_string()),
                },
            )
            .await
        {
            debug!(%update, "the acquire job's failure could not be journalled");
        }
    }

    /// The work, owning everything it borrows.
    #[allow(clippy::too_many_lines)] // The job is one loop; splitting it hides the order the steps run in.
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

        if let Some(chosen) = self.target.chosen.clone() {
            // The user picked this release from the offers: no search and no ranking, but the gate
            // and the admission below still run. A manual choice never bypasses a safety check.
            self.phase(job, attempt, 500).await;
            let release = IndexerRelease {
                raw_title: chosen.raw_title.clone(),
                title: ReleaseName::parse(&chosen.raw_title),
                size_bytes: chosen.size_bytes,
                seeders: chosen.seeders,
                leechers: None,
                source: chosen.source,
                indexer: chosen.indexer,
                published: None,
                guid: None,
            };
            let request = self.prepare(&release, &identity.name).await?;
            return self.start_candidate(job, attempt, request).await;
        }

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

        let releases = indexers::search_async(
            Arc::clone(&self.store),
            search_query(&identity.name, self.target.episode),
            SEARCH_LIMIT,
        )
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

        // The seeder floor, the size ceiling, and the parse happen here, before the pure ranking
        // rule sees anything: a result with too few seeds is not a candidate, one whose name will
        // not parse is not one either, and one the profile's own ceiling refuses — or the built-in
        // HD ceiling when the profile sets none — cannot win a decision the inspection cap would
        // then reverse.
        let effective_ceiling = profile
            .rules()
            .size_caps()
            .ceiling()
            .unwrap_or(DEFAULT_HD_CEILING_BYTES);
        let mut candidates: Vec<Candidate> = Vec::new();
        let mut kept: Vec<&IndexerRelease> = Vec::new();
        for release in &releases {
            if release.seeders.unwrap_or(0) < self.min_seeders {
                continue;
            }
            let size_bytes = release.size_bytes.unwrap_or(0);
            if size_bytes > effective_ceiling {
                continue;
            }
            let name = ReleaseName::parse(&release.raw_title);
            // An episode acquire takes only a release that names exactly that episode: a season
            // pack or a multi-episode span is a different download, not a substitute for one.
            if self
                .target
                .episode
                .is_some_and(|pick| !name.is_single_episode(pick.season, pick.episode))
            {
                continue;
            }
            candidates.push(Candidate::new(name, size_bytes));
            kept.push(release);
        }

        let ranked = ranked_candidates(&candidates, &profile);
        if ranked.is_empty() {
            // The preferred quality found nothing, but the releases that do exist are worth
            // offering: "nothing at 1080p, but 2160p x3 and 720p x1 exist" is a choice, where "no
            // release matched" is a dead end. They are stored with their sources so the user can
            // pick one without the search running again.
            let alternatives = alternative_candidates(&candidates, &profile);
            if self
                .persist_offers(&candidates, &kept, &alternatives, &profile)
                .await
            {
                // Once per search that produced offers, not once per offer: the service coalesces a
                // repeat on its dedupe key, and the centre row is written before any toast (R31).
                self.offers
                    .offers_available(
                        self.account,
                        self.target.title_id,
                        Some(identity.name.clone()),
                    )
                    .await;
            }
            let summary = summarise_alternatives(&candidates, &alternatives);
            warn!(available = %summary, "no release matched the preferred quality");
            return Err(JobError::failed(format!(
                "nothing matched your preferred quality; other qualities available: {summary}"
            )));
        }

        self.phase(job, attempt, 500).await;

        // A candidate the gate refuses must not end the attempt while another accepted candidate
        // exists: the ranker knows a release's name and stated size, and the gate knows the
        // destination and the volume, so only the gate can refuse what the ranker chose. Walk the
        // ranked list — the search runs once — and gate each candidate before `start` persists
        // anything.
        let mut refused: Option<StartRequest> = None;
        let mut last_error: Option<JobError> = None;
        for index in ranked.into_iter().take(MAX_CANDIDATES_TRIED) {
            if cancellation.is_cancelled() {
                return Err(JobError::Cancelled);
            }
            let chosen = kept[index];
            let request = match self.prepare(chosen, &identity.name).await {
                Ok(request) => request,
                Err(error) => {
                    // One release whose metadata will not arrive is one candidate lost, not the
                    // attempt: its swarm may be dead while the next release is seeded.
                    debug!(%error, "a ranked candidate could not be prepared");
                    last_error = Some(error);
                    continue;
                }
            };
            let verdict = self
                .acquisition
                .verdict_for(request.job.save_path.as_str(), &request.description)
                .map_err(|error| JobError::failed(error.to_string()))?;
            if verdict == Verdict::Blocked {
                refused = Some(request);
                continue;
            }
            return self.start_candidate(job, attempt, request).await;
        }

        // Every tried candidate was refused: persist the last verdict, so the queue shows one
        // blocked row with its reasons rather than the page showing nothing happened.
        if let Some(request) = refused {
            return self.start_candidate(job, attempt, request).await;
        }
        Err(last_error.unwrap_or_else(|| {
            JobError::failed("no release the quality profile accepts could be prepared")
        }))
    }

    /// Stores what a search found at other qualities, so a person can pick one later.
    ///
    /// The source goes into the row and stays there: the chooser is answered from the metadata
    /// columns only, and the source is looked up by id when a choice names one. A failure is a
    /// warning rather than the job's outcome: an unstored offer costs a choice, not the search.
    async fn persist_offers(
        &self,
        candidates: &[Candidate],
        kept: &[&IndexerRelease],
        alternatives: &[usize],
        profile: &peregrine_domain::QualityProfile,
    ) -> bool {
        let mut offers = Vec::with_capacity(alternatives.len());
        for (position, index) in alternatives.iter().enumerate() {
            let (Some(release), Some(candidate)) = (kept.get(*index), candidates.get(*index))
            else {
                continue;
            };
            // describe() is the domain's own audit line for this outcome, so the reason stored
            // here is the same text the "why this release" panel would show.
            let ranking = ReleaseRanker::rank(candidate, profile);
            let reasons_json = ranking.describe();
            let Ok(source_json) = serde_json::to_string(&release.source) else {
                continue;
            };
            offers.push(NewOffer {
                release_title: release.raw_title.clone(),
                indexer: release.indexer.clone(),
                score: ranking.score().map_or(0.0, |score| score as f64),
                rank_position: i64::try_from(position).unwrap_or(i64::MAX),
                reasons_json,
                source_json,
                size_bytes: release
                    .size_bytes
                    .and_then(|value| i64::try_from(value).ok()),
                seeders: release.seeders.map(i64::from),
            });
        }
        if offers.is_empty() {
            return false;
        }
        match self
            .database
            .replace_release_offers(
                self.target.title_id,
                &offers,
                self.clock.now().as_millisecond(),
            )
            .await
        {
            Ok(_stored) => true,
            Err(error) => {
                warn!(%error, "the offers a search found could not be stored");
                false
            }
        }
    }

    /// One ranked candidate as a start request: its source, its metadata, and the destination
    /// folder. A metadata failure belongs to that candidate, so the caller decides whether to step
    /// on to the next.
    async fn prepare(
        &self,
        chosen: &IndexerRelease,
        title_name: &str,
    ) -> Result<StartRequest, JobError> {
        // What the engine is handed: a magnet when the indexer gave one, an info hash completed
        // into the same urn:btih form, or the .torrent bytes fetched from the URL Prowlarr serves
        // as its magnetUrl -- a proxy download URL, not a magnet, which the engine would try to GET.
        let mut magnet_uri = String::new();
        let engine_source = match &chosen.source {
            IndexerSource::Magnet(uri) => {
                magnet_uri = uri.clone();
                EngineSource::Magnet(NonEmptyText::new(uri).map_err(|error| {
                    JobError::failed(format!("the release's magnet is not usable: {error}"))
                })?)
            }
            IndexerSource::InfoHash(hash) => {
                magnet_uri = format!("magnet:?xt=urn:btih:{hash}");
                EngineSource::Magnet(NonEmptyText::new(&magnet_uri).map_err(|error| {
                    JobError::failed(format!("the release's magnet is not usable: {error}"))
                })?)
            }
            IndexerSource::TorrentUrl(url) => {
                let fetched = indexers::fetch_torrent_async(Arc::clone(&self.store), url.clone())
                    .await
                    .map_err(|error| {
                        JobError::failed(format!(
                            "the release's torrent could not be fetched: {error}"
                        ))
                    })?;
                match fetched {
                    FetchedTorrent::Bytes(bytes) => EngineSource::TorrentFile(bytes),
                    // The proxy answered 301 with a magnet, which is the source the engine wanted
                    // all along; the row keeps it, so nothing has to be re-derived from bytes.
                    FetchedTorrent::Magnet(uri) => {
                        magnet_uri.clone_from(&uri);
                        EngineSource::Magnet(NonEmptyText::new(&uri).map_err(|error| {
                            JobError::failed(format!("the release's magnet is not usable: {error}"))
                        })?)
                    }
                }
            }
        };

        let fetched = self
            .metadata
            .fetch_metadata(&engine_source)
            .await
            .map_err(|error| {
                JobError::failed(format!(
                    "the torrent metadata could not be fetched: {error}"
                ))
            })?;

        // The row's source is always a magnet: a .torrent file carries no trackers once it is bytes
        // on disk, and the fetched info hash is what the engine and the safety gate both key on.
        if magnet_uri.is_empty() {
            magnet_uri = format!("magnet:?xt=urn:btih:{}", fetched.info_hash().to_hex());
        }

        let save_path = NonEmptyText::new(&folder_for(title_name)).map_err(|error| {
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

        Ok(StartRequest {
            account: self.account,
            job: job_row,
            description: fetched.description().clone(),
            info_hash: fetched.info_hash(),
        })
    }

    /// Hands one prepared candidate to the acquisition service, which persists the row and either
    /// starts the transfer or parks a refusal the queue can show.
    async fn start_candidate(
        &self,
        job: ulid::Ulid,
        attempt: u32,
        request: StartRequest,
    ) -> peregrine_sync::JobResult {
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
                self.record_failure(error).await;
            }
            outcome
        })
    }
}

/// The indexer query for a title, narrowed to one episode when the target names one.
fn search_query(name: &str, episode: Option<EpisodePick>) -> String {
    match episode {
        Some(pick) => format!("{name} S{:02}E{:02}", pick.season, pick.episode),
        None => name.to_owned(),
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

/// A short "2160p x3, 720p x1" summary of the qualities the profile refused, for the sentence a
/// person reads when nothing at their preferred quality was found.
///
/// Only resolutions the parser actually read are counted: a release it could not name is not an
/// offer anyone can act on, and a count that included one would overstate what exists.
fn summarise_alternatives(candidates: &[Candidate], alternatives: &[usize]) -> String {
    let mut counts: Vec<(Resolution, usize)> = Vec::new();
    for index in alternatives {
        let Some(resolution) = candidates[*index].name.resolution() else {
            continue;
        };
        match counts.iter_mut().find(|(value, _)| *value == resolution) {
            Some((_, count)) => *count += 1,
            None => counts.push((resolution, 1)),
        }
    }
    if counts.is_empty() {
        return "none".to_owned();
    }
    counts
        .iter()
        .map(|(resolution, count)| format!("{} x{count}", resolution.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_query_is_the_title_name() {
        assert_eq!(search_query("Breaking Bad", None), "Breaking Bad");
    }

    #[test]
    fn an_episode_query_pads_its_markers_to_two_digits() {
        assert_eq!(
            search_query(
                "Breaking Bad",
                Some(EpisodePick {
                    season: 1,
                    episode: 2
                })
            ),
            "Breaking Bad S01E02"
        );
        assert_eq!(
            search_query(
                "Breaking Bad",
                Some(EpisodePick {
                    season: 12,
                    episode: 103,
                })
            ),
            "Breaking Bad S12E103",
            "a number wider than two digits is not truncated"
        );
    }

    #[test]
    fn an_episode_gets_its_own_dedupe_key() {
        let episode_two = EpisodePick {
            season: 1,
            episode: 2,
        };
        let episode_three = EpisodePick {
            season: 1,
            episode: 3,
        };
        assert_eq!(dedupe_key(7, None), "acquisition.acquire:7");
        assert_eq!(
            dedupe_key(7, Some(episode_two)),
            "acquisition.acquire:7:s01e02"
        );
        assert_ne!(
            dedupe_key(7, Some(episode_two)),
            dedupe_key(7, Some(episode_three))
        );
        assert_ne!(dedupe_key(7, None), dedupe_key(7, Some(episode_two)));
    }
}
