//! Auto-acquiring one title: find the best release on the configured indexer and queue it.
//!
//! A job rather than a command because it leaves the machine three times — a Prowlarr search, a
//! metadata fetch over `BitTorrent`, and the engine's own add — and ARCHITECTURE §7 rule 2 puts any
//! of those past a command's budget. The selection itself is pure and lives in peregrine-domain
//! (`rank::best_candidate`); this file only supplies the candidates and the profile.

use std::cmp::Reverse;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use peregrine_domain::language;
use peregrine_domain::rank::{
    Candidate, Ranking, ReleaseRanker, alternative_candidates, ranked_candidates,
};
use peregrine_domain::{
    AccountScope, Clock, DEFAULT_HD_CEILING_BYTES, DownloadState, NonEmptyText, NzbHash,
    PayloadDescription, Protocol, ReleaseName, Resolution, TitleKind, Verdict, release_names_title,
};
use peregrine_indexers::{
    FetchedRelease, IndexerRelease, ReleaseSource, TorrentSource as IndexerSource, UsenetSource,
};
use peregrine_secrets::SecretStore;
use peregrine_storage::{
    AddedBy, Database, DownloadSource, JobChange, JobPhase, MagnetUri, NewDownloadJob, NewOffer,
    PageRequest,
};
use peregrine_sync::{Job, JobContext, JobError, JobFuture};
use peregrine_torrent::{TorrentMetadataFetch, TorrentSource as EngineSource};
use peregrine_usenet::NzbDocument;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::acquisition::{AcquisitionService, StartRequest, Started, UsenetStartRequest};
use crate::events::{Event, EventBus, JOB_PROGRESS};
use crate::indexers;
use crate::notify::NotifyAcquisition;

/// The kind this job registers under, in the scheduler and in the journal.
pub const KIND: &str = "acquisition.acquire";

/// The default priority: a person asked for this download, so it is the work a user just asked for
/// rather than background maintenance.
pub const PRIORITY: i32 = 5;

/// How many indexer results a search asks for.
const SEARCH_LIMIT: u32 = 100;

/// How many episodes of one season the fan-out reads.
///
/// The same generous bound the title screen reads; a season with more is a season no provider has
/// published, and the read is bounded rather than paged for the reason that screen's doc gives.
const SEASON_EPISODE_LIMIT: u32 = 500;

/// How many seasons of one title the pack selection reads to find the wanted one's count.
///
/// The same generous bound the title screen's season switcher uses: a show with more seasons than
/// this is not a show any provider has published.
const SEASON_LIMIT: u32 = 100;

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
    pub source: ReleaseSource,
}

/// One episode to acquire rather than the whole title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpisodePick {
    /// The season the episode belongs to.
    pub season: u32,
    /// The episode's number within that season.
    pub episode: u32,
}

/// What the catalogue says a search result has to name before it can be a candidate.
///
/// Carried as one value rather than three parameters because every search this job performs
/// applies the same rule: the title's words, whether it is a film, and the year a film was
/// released (`None` for a series, whose year is not evidence about which part is wanted).
#[derive(Debug, Clone, Copy)]
struct WantedTitle<'a> {
    /// The catalogue's display name.
    name: &'a str,
    /// A film or a series.
    kind: TitleKind,
    /// The year a film was released, when the catalogue knows one.
    year: Option<u32>,
}

impl WantedTitle<'_> {
    /// True when `release` names this title in the shape the request needs.
    fn names(&self, release: &ReleaseName) -> bool {
        release_names_title(release, self.name, self.kind, self.year)
    }
}

/// What one episode's own search achieved, which is what the season's failure sentence counts.
#[derive(Debug, Clone)]
enum EpisodeAttempt {
    /// The episode's release was queued.
    Queued,
    /// No release named the episode after the title and episode filters.
    NoRelease,
    /// Releases named the episode, but none could be started; the reason is what the
    /// fan-out's warning reports.
    Refused {
        /// Why the releases that named the episode could not be started.
        reason: String,
    },
}

/// The indexer operations an acquire job performs.
///
/// A port rather than a free function so the season fan-out can be tested without a Prowlarr
/// instance or a network (AGENTS R14): a test supplies a scripted search, and the shipping adapter
/// is the configured indexer this process already reads.
pub trait ReleaseIndexer: Send + Sync {
    /// Searches the configured indexer for `query`.
    fn search<'a>(
        &'a self,
        query: String,
        limit: u32,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Vec<IndexerRelease>, indexers::IndexerConfigError>>
                + Send
                + 'a,
        >,
    >;

    /// Fetches the bytes behind a release's HTTP source.
    fn fetch<'a>(
        &'a self,
        source: ReleaseSource,
    ) -> Pin<
        Box<dyn Future<Output = Result<FetchedRelease, indexers::IndexerConfigError>> + Send + 'a>,
    >;
}

/// The configured Prowlarr, behind [`ReleaseIndexer`].
pub struct ConfiguredReleaseIndexer {
    store: Arc<dyn SecretStore>,
}

impl ConfiguredReleaseIndexer {
    /// Builds the adapter over the credential store the process already holds.
    #[must_use]
    pub fn new(store: Arc<dyn SecretStore>) -> Self {
        Self { store }
    }
}

impl ReleaseIndexer for ConfiguredReleaseIndexer {
    fn search<'a>(
        &'a self,
        query: String,
        limit: u32,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Vec<IndexerRelease>, indexers::IndexerConfigError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move { indexers::search_async(Arc::clone(&self.store), query, limit).await })
    }

    fn fetch<'a>(
        &'a self,
        source: ReleaseSource,
    ) -> Pin<
        Box<dyn Future<Output = Result<FetchedRelease, indexers::IndexerConfigError>> + Send + 'a>,
    > {
        Box::pin(async move { indexers::fetch_async(Arc::clone(&self.store), source).await })
    }
}

/// One candidate prepared for the service, in the protocol it arrived as.
enum Prepared {
    /// A torrent, with the metadata the gate judges.
    Torrent(Box<StartRequest>),
    /// A Usenet post, with the parsed NZB the gate judges.
    Usenet(Box<UsenetStartRequest>),
}

/// The live queue rows this title already holds for one season.
#[derive(Debug, Default)]
struct LiveSeason {
    /// A live pack row covers every episode of the season. Only a row whose release names a pack
    /// sets this: an episode row with no `episode_id` is unlinked, not a pack.
    season_covered: bool,
    /// The episode rows that are live.
    episodes: HashSet<i64>,
}

/// Which title to acquire, which part of it, and the release to acquire when the user has chosen
/// one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The catalogue row.
    pub title_id: i64,
    /// The whole season to acquire, when the request names one.
    pub season: Option<u32>,
    /// The one episode to acquire, or None to acquire the title as a whole.
    pub episode: Option<EpisodePick>,
    /// The release the user picked, or None to search.
    pub chosen: Option<ChosenOffer>,
}

/// The dedupe key: one live acquire per title, per season, or per episode — whichever was asked
/// for — so a second request joins the work already running rather than starting it twice.
#[must_use]
pub fn dedupe_key(title_id: i64, season: Option<u32>, episode: Option<EpisodePick>) -> String {
    match (episode, season) {
        (Some(pick), _) => format!(
            "acquisition.acquire:{title_id}:s{:02}e{:02}",
            pick.season, pick.episode
        ),
        (None, Some(season)) => format!("acquire:season:{title_id}:{season}"),
        (None, None) => format!("acquisition.acquire:{title_id}"),
    }
}

/// What a release is for a season request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeasonCandidate {
    /// One torrent carrying the whole season.
    Pack,
    /// One episode of the season.
    Episode,
}

/// Whether a release can serve a request for a whole season, and as which shape.
///
/// A pack must parse to exactly the requested season; a single episode must parse to the same
/// season and to one episode. A release naming another season is a different download however well
/// seeded it is, and a multi-episode span inside one season is neither a pack nor one episode.
fn season_candidate(name: &ReleaseName, season: u32) -> Option<SeasonCandidate> {
    if name.season() != Some(season) {
        return None;
    }
    if pack_like(name) {
        return Some(SeasonCandidate::Pack);
    }
    match name.episodes() {
        Some(span) if !span.is_multi() => Some(SeasonCandidate::Episode),
        _ => None,
    }
}

/// Whether a release is a pack this job can narrow to one season.
///
/// A name that states a season and no episode is a season pack. A complete-series name that
/// spells its episode range (`S01E01-S04E52`) is one too: its season field is where the range
/// starts, and the entries are what place each file. A span inside one season stays neither a
/// pack nor one episode for a season request.
fn pack_like(name: &ReleaseName) -> bool {
    name.is_season_pack() || (name.is_multi_season() && name.episodes().is_some())
}

/// Whether a season pack's selection can be decided from its name alone.
///
/// The file list is the real evidence, and fetching it costs a metadata round trip, so the peer
/// comparison must not choose a pack the selector is already known to refuse. A multi-season pack
/// that names no episodes numbers its files absolutely across the range, and nothing says where
/// the wanted season ends. A pack whose name carries `SxxEyy` markers can be filtered per entry,
/// so it stays usable until the file list says otherwise.
fn pack_selection_viable(name: &ReleaseName) -> bool {
    !name.is_multi_season() || name.episodes().is_some()
}

/// Why one ranked candidate could not be prepared.
enum PrepareError {
    /// The fetched file list cannot be narrowed to the requested season. The selector's refusal
    /// stays as it is; a season job can fall back to the season's episodes instead of ending.
    Unselectable(JobError),
    /// The candidate was lost for a reason that is not its season shape.
    Failed(JobError),
}

impl PrepareError {
    /// The sentence the job reports when there is no fallback to take.
    fn into_job_error(self) -> JobError {
        match self {
            Self::Unselectable(error) | Self::Failed(error) => error,
        }
    }
}

impl From<JobError> for PrepareError {
    fn from(error: JobError) -> Self {
        Self::Failed(error)
    }
}

/// Why a ranked walk started nothing.
enum RankedFailure {
    /// At least one pack's fetched file list could not be narrowed to the requested season.
    Unselectable { refused: usize },
    /// The preferred quality accepts none of the candidates. A season job can still try its
    /// episodes; a title or episode request reports the sentence.
    NothingMatched(JobError),
    /// No candidate could be prepared for any other reason.
    Failed(JobError),
}

impl RankedFailure {
    /// The sentence a caller with no fallback reports.
    fn into_job_error(self) -> JobError {
        match self {
            Self::Unselectable { refused } => JobError::failed(format!(
                "{refused} season pack(s) could not be narrowed to the requested season"
            )),
            Self::NothingMatched(error) | Self::Failed(error) => error,
        }
    }
}

/// Which shape a season request takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeasonStrategy {
    /// Acquire one torrent that carries the season.
    Pack,
    /// Acquire a single episode of the season.
    Episode,
}

/// **Coverage wins.** A season search answers packs and single episodes together, and the two are
/// different downloads: a pack is one transfer covering every episode, while an episode names one
/// file of the season. A usable pack therefore satisfies the request the person made, and the
/// per-episode fan-out is the fallback for a search that found no usable pack — never a faster
/// route chosen because one episode's swarm is larger.
///
/// Seeders still order packs against each other when the quality ranker scores them equally, which
/// is the only comparison they decide. They no longer weigh a pack against the fan-out.
fn season_strategy(packs: &[&IndexerRelease]) -> SeasonStrategy {
    if packs.is_empty() {
        SeasonStrategy::Episode
    } else {
        SeasonStrategy::Pack
    }
}

/// A pack's selection: the entries whose own names place them in `season`, or — when a
/// single-season pack numbers its files absolutely — every entry in file order mapped to
/// episodes 1..N.
///
/// A pack's release name states the season once; its entries repeat it, and a torrent that bundles
/// several seasons (or a complete-series set) repeats several. `ReleaseName::parse` on each entry
/// path is the same parser that placed the file, so a name the importer would put in another season
/// is not written here either. That stays the rule whenever the entries name a season at all.
///
/// Some packs instead number every file within the season (`Peppa Pig - 04 - Muddy Puddles.mkv`,
/// `01 - ....mkv`). There is no season in those names, so the only thing that places them is the
/// pack's own name plus their order. That is a safe mapping only for a single-season pack, and only
/// when the file count fits the season the database describes: for a multi-season pack the files
/// run across seasons and nothing says where one ends, so the job refuses rather than guessing.
///
/// The skipped count is returned for the job's log.
///
/// # Errors
///
/// A pack that yields no wanted entry, whose absolute numbering cannot be attributed to the
/// request, or whose file count does not fit the season is refused with the sentence the job
/// reports, rather than added with nothing selected.
fn season_pack_selection(
    description: &PayloadDescription,
    pack: &ReleaseName,
    season: u32,
    season_episode_count: Option<u32>,
) -> Result<(Vec<usize>, usize), JobError> {
    let entry_seasons: Vec<Option<u32>> = description
        .entries()
        .iter()
        .map(|entry| ReleaseName::parse(entry.path()).season())
        .collect();

    if entry_seasons.iter().any(Option::is_some) {
        let mut wanted = Vec::new();
        let mut skipped = 0;
        for (index, entry_season) in entry_seasons.iter().enumerate() {
            if *entry_season == Some(season) {
                wanted.push(index);
            } else {
                skipped += 1;
            }
        }
        if wanted.is_empty() {
            return Err(JobError::failed(format!(
                "the release has no files for season {season}, so it was not added"
            )));
        }
        return Ok((wanted, skipped));
    }

    if description.entries().is_empty() {
        return Err(JobError::failed(format!(
            "the release has no files for season {season}, so it was not added"
        )));
    }
    if pack.is_multi_season() {
        return Err(JobError::failed(format!(
            "the release numbers its files without a season in each name and spans more than one \
             season, so it cannot tell where season {season} ends; it was not added"
        )));
    }
    if pack.season() != Some(season) {
        return Err(JobError::failed(format!(
            "the release numbers its files without a season in each name and does not name \
             season {season}, so it cannot tell which files belong to it; it was not added"
        )));
    }

    // Zero means the library has no count to compare against: a mapping guessed from that would
    // invent an episode number nobody described.
    let Some(episode_count) = season_episode_count.filter(|count| *count > 0) else {
        return Err(JobError::failed(format!(
            "season {season} has no episode count in the library, so the release's {} files \
             cannot be mapped to its episodes; it was not added",
            description.entries().len()
        )));
    };
    let files = u32::try_from(description.entries().len()).unwrap_or(u32::MAX);
    if files > episode_count {
        return Err(JobError::failed(format!(
            "the release has {files} files but season {season} has {episode_count} episodes, so \
             the files cannot be mapped to episodes; it was not added"
        )));
    }
    Ok(((0..description.entries().len()).collect(), 0))
}

/// Finds one title's best release and hands it to the acquisition service.
pub struct AcquireJob {
    database: Arc<Database>,
    bus: Arc<EventBus>,
    store: Arc<dyn SecretStore>,
    indexer: Arc<dyn ReleaseIndexer>,
    metadata: Arc<dyn TorrentMetadataFetch>,
    acquisition: Arc<AcquisitionService>,
    clock: Arc<dyn Clock + Send + Sync>,
    account: AccountScope,
    journal_id: i64,
    target: Target,
    min_seeders: u32,
    /// The audio language the account prefers, which orders the ranked candidates.
    preferred_audio_language: String,
    /// What this job may raise, so the job does not name the service's four ports.
    notices: Arc<dyn NotifyAcquisition>,
    /// The title's display name once the scoped identity read has supplied it.
    ///
    /// Held so the failure notice can name the title after the work that produced the error
    /// has dropped its own copy. A job whose identity read failed has none, and the generic
    /// catalogue copy is still the notification the user gets.
    title_name: Option<String>,
}

impl AcquireJob {
    /// A job that acquires one title under one account.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        database: Arc<Database>,
        bus: Arc<EventBus>,
        store: Arc<dyn SecretStore>,
        indexer: Arc<dyn ReleaseIndexer>,
        metadata: Arc<dyn TorrentMetadataFetch>,
        acquisition: Arc<AcquisitionService>,
        clock: Arc<dyn Clock + Send + Sync>,
        account: AccountScope,
        journal_id: i64,
        target: Target,
        min_seeders: u32,
        preferred_audio_language: String,
        notices: Arc<dyn NotifyAcquisition>,
    ) -> Self {
        Self {
            database,
            bus,
            store,
            indexer,
            metadata,
            acquisition,
            clock,
            account,
            journal_id,
            target,
            min_seeders,
            preferred_audio_language,
            notices,
            title_name: None,
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

    /// The work, with the one thing a failure owes the person wrapped around it.
    ///
    /// An acquire that finds no release writes no download row, so the queue's failed line is
    /// the only place its outcome would otherwise appear. The notice is raised here rather than
    /// at each `return Err` so every non-cancelled ending reaches it once, and a cancellation —
    /// the person's own decision — deliberately does not.
    async fn acquire(
        &mut self,
        job: ulid::Ulid,
        attempt: u32,
        cancellation: CancellationToken,
    ) -> peregrine_sync::JobResult {
        let outcome = self.acquire_target(job, attempt, cancellation).await;
        if let Err(error) = &outcome
            && !matches!(error, JobError::Cancelled)
        {
            self.notices
                .failed(self.account, self.target.title_id, self.title_name.clone())
                .await;
        }
        outcome
    }

    /// The work, owning everything it borrows.
    #[allow(clippy::too_many_lines)] // The job is one loop; splitting it hides the order the steps run in.
    async fn acquire_target(
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
        self.title_name = Some(identity.name.clone());

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
            let request = self
                .prepare(&release, &identity.name, self.target.episode)
                .await
                .map_err(PrepareError::into_job_error)?;
            let started = self.start_candidate(request).await?;
            self.phase(job, attempt, 1000).await;
            return Ok(Box::new(started.job.id));
        }

        // A film's year is only evidence for a short title, but reading it for every film keeps
        // the rule out of the search path: a release that names another film with the same word
        // is refused before it costs a metadata fetch. A series has no single year to check, and
        // a manual choice never needs the rule, so both skip the read.
        let year = if identity.kind == TitleKind::Movie {
            self.database
                .title_year(self.account, self.target.title_id)
                .await
                .map_err(|error| {
                    JobError::failed(format!(
                        "the title's release year could not be read: {error}"
                    ))
                })?
        } else {
            None
        };
        let wanted = WantedTitle {
            name: &identity.name,
            kind: identity.kind,
            year,
        };

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

        let releases = self
            .indexer
            .search(
                search_query(wanted.name, self.target.season, self.target.episode),
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
        // The pack side of a season request, held apart until the peer count decides the strategy.
        let mut packs: Vec<Candidate> = Vec::new();
        let mut pack_kept: Vec<&IndexerRelease> = Vec::new();
        // How many of the indexer's answers named a different title. A search whose every answer
        // was one of those is not "nothing at the preferred quality", and the sentence says so.
        let mut title_mismatches = 0usize;
        for release in &releases {
            if release.seeders.unwrap_or(0) < self.min_seeders {
                continue;
            }
            let size_bytes = release.size_bytes.unwrap_or(0);
            if size_bytes > effective_ceiling {
                continue;
            }
            let name = ReleaseName::parse(&release.raw_title);
            if !wanted.names(&name) {
                title_mismatches += 1;
                // The title is the only thing this line carries: a release name is safe to log
                // (it is already in the queue and the offers), while a magnet and a save path are
                // not (AGENTS R31).
                debug!(
                    release = %release.raw_title,
                    reason = "title-mismatch",
                    "a release was skipped because its name does not match the requested title"
                );
                continue;
            }
            if let Some(pick) = self.target.episode {
                // An episode acquire takes only a release that names exactly that episode: a season
                // pack or a multi-episode span is a different download, not a substitute for one.
                if !name.is_single_episode(pick.season, pick.episode) {
                    continue;
                }
                candidates.push(Candidate::new(name, size_bytes));
                kept.push(release);
                continue;
            }
            if let Some(season) = self.target.season {
                match season_candidate(&name, season) {
                    Some(SeasonCandidate::Pack) => {
                        if pack_selection_viable(&name) {
                            packs.push(Candidate::new(name, size_bytes));
                            pack_kept.push(release);
                        } else {
                            info!(
                                season,
                                release = %release.raw_title,
                                "skipped a multi-season pack whose files are numbered absolutely, \
                                 because the selector cannot tell where the season ends"
                            );
                        }
                    }
                    Some(SeasonCandidate::Episode) => {
                        candidates.push(Candidate::new(name, size_bytes));
                        kept.push(release);
                    }
                    None => {}
                }
                continue;
            }
            candidates.push(Candidate::new(name, size_bytes));
            kept.push(release);
        }
        // A title or episode request has no fan-out behind it, so an empty candidate list is the
        // answer. When every answer named another title, say that instead of blaming the quality
        // profile: the person's next move is a different search, not a different profile.
        if self.target.season.is_none() && candidates.is_empty() && title_mismatches > 0 {
            return Err(JobError::failed(format!(
                "no release names the requested title; {title_mismatches} release(s) the indexer \
                 returned name a different title"
            )));
        }
        // Coverage wins, and the two answers have different shapes. A usable pack keeps the
        // one-download path below; only a search with no usable pack queues every missing episode
        // of the season inside this same job, because one episode is not the season the person
        // asked for.
        if self.target.episode.is_none()
            && let Some(season) = self.target.season
        {
            if season_strategy(&pack_kept) == SeasonStrategy::Episode {
                info!(
                    season,
                    "no usable season pack was found; falling back to the per-episode fan-out"
                );
                return self
                    .acquire_season_episodes(
                        job,
                        attempt,
                        wanted,
                        &profile,
                        season,
                        0,
                        None,
                        title_mismatches,
                        &cancellation,
                    )
                    .await;
            }
            candidates = packs;
            kept = pack_kept;
            return match self
                .start_ranked(
                    job,
                    attempt,
                    wanted.name,
                    &profile,
                    &candidates,
                    &kept,
                    &cancellation,
                )
                .await
            {
                Ok(started) => Ok(Box::new(started.job.id)),
                Err(RankedFailure::Unselectable { refused }) => {
                    // The name check could not see the file list; the selector has now refused
                    // the pack. The episodes are the job's remaining way to satisfy the season,
                    // and the refused pack is one failed attempt rather than the job's outcome.
                    info!(
                        season,
                        refused,
                        "no season pack could be narrowed to the season; falling back to its episodes"
                    );
                    self.acquire_season_episodes(
                        job,
                        attempt,
                        wanted,
                        &profile,
                        season,
                        refused,
                        None,
                        title_mismatches,
                        &cancellation,
                    )
                    .await
                }
                Err(RankedFailure::NothingMatched(_)) => {
                    // A season search that ranks nothing is not the season's answer: each
                    // episode does its own search, so the fan-out is the remaining route. The
                    // refusals are carried into that route's sentence, because "no episode could
                    // be queued" alone hides a pack the profile could not rank.
                    let refused = refusal_summary(&candidates, &profile);
                    info!(
                        season,
                        refused = %refused,
                        "no season pack matched the preferred quality; falling back to its episodes"
                    );
                    self.acquire_season_episodes(
                        job,
                        attempt,
                        wanted,
                        &profile,
                        season,
                        0,
                        Some(&refused),
                        title_mismatches,
                        &cancellation,
                    )
                    .await
                }
                Err(failure) => Err(failure.into_job_error()),
            };
        }

        match self
            .start_ranked(
                job,
                attempt,
                wanted.name,
                &profile,
                &candidates,
                &kept,
                &cancellation,
            )
            .await
        {
            Ok(started) => Ok(Box::new(started.job.id)),
            Err(failure) => Err(failure.into_job_error()),
        }
    }

    /// Ranks one search's candidates and starts the best one the gate allows.
    ///
    /// The offer fallback lives here rather than in the caller because it is the same decision for a
    /// title, an episode, and a pack: when the preferred quality found nothing, what did exist is
    /// stored so the person can choose instead of being told a search came back empty.
    #[allow(clippy::too_many_arguments)] // The job's identity plus one search's four inputs; splitting hides the order.
    async fn start_ranked(
        &self,
        job: ulid::Ulid,
        attempt: u32,
        title_name: &str,
        profile: &peregrine_domain::QualityProfile,
        candidates: &[Candidate],
        kept: &[&IndexerRelease],
        cancellation: &CancellationToken,
    ) -> Result<Started, RankedFailure> {
        let mut ranked = ranked_candidates(candidates, profile);
        // Stable: a release that names the preferred language moves ahead of one that names
        // another, and two releases in the same language band keep the quality ranker's order.
        // Nothing is dropped — a release in the wrong language is still a candidate. The tier is
        // the first key, so the language pass cannot lift an undeclared fallback above a declared
        // candidate.
        //
        // A season pack adds the score and the swarm after the language: the ranker stays
        // primary, and the most peers break an exact quality tie between packs. Those keys stay
        // `None` for title and episode searches, where seeders decide nothing.
        let season_pack = self.target.season.is_some() && self.target.episode.is_none();
        ranked.sort_by_key(|index| {
            let ranking = ReleaseRanker::rank(&candidates[*index], profile);
            (
                ranking.is_fallback(),
                language::rank(
                    kept[*index].raw_title.as_str(),
                    &self.preferred_audio_language,
                ),
                season_pack.then(|| Reverse(ranking.score().unwrap_or(0))),
                season_pack.then(|| Reverse(kept[*index].seeders.unwrap_or(0))),
            )
        });
        if ranked.is_empty() {
            // The preferred quality found nothing, but the releases that do exist are worth
            // offering: "nothing at 1080p, but 2160p x3 and 720p x1 exist" is a choice, where "no
            // release matched" is a dead end. They are stored with their sources so the user can
            // pick one without the search running again.
            let alternatives = alternative_candidates(candidates, profile);
            if self
                .persist_offers(candidates, kept, &alternatives, profile)
                .await
            {
                // Once per search that produced offers, not once per offer: the service coalesces a
                // repeat on its dedupe key, and the centre row is written before any toast (R31).
                self.notices
                    .offers_available(
                        self.account,
                        self.target.title_id,
                        Some(title_name.to_owned()),
                    )
                    .await;
            }
            let summary = summarise_alternatives(candidates, &alternatives);
            let refused = refusal_summary(candidates, profile);
            warn!(
                available = %summary,
                refused = %refused,
                "no release matched the preferred quality"
            );
            return Err(RankedFailure::NothingMatched(JobError::failed(format!(
                "nothing matched your preferred quality; other qualities available: {summary}"
            ))));
        }

        self.phase(job, attempt, 500).await;
        let started = self
            .start_from_ranked(
                title_name,
                self.target.episode,
                candidates,
                profile,
                ranked,
                kept,
                cancellation,
            )
            .await?;
        self.phase(job, attempt, 1000).await;
        Ok(started)
    }

    /// Walks the ranked candidates, gating each before `start` persists anything.
    ///
    /// A candidate the gate refuses must not end the attempt while another accepted candidate
    /// exists: the ranker knows a release's name and stated size, and the gate knows the destination
    /// and the volume, so only the gate can refuse what the ranker chose. Every tried candidate
    /// refused is still persisted, so the queue shows one blocked row rather than nothing.
    #[allow(clippy::too_many_arguments)] // The search's candidate slice and profile, plus the walk's own arguments.
    async fn start_from_ranked(
        &self,
        title_name: &str,
        episode: Option<EpisodePick>,
        candidates: &[Candidate],
        profile: &peregrine_domain::QualityProfile,
        ranked: Vec<usize>,
        kept: &[&IndexerRelease],
        cancellation: &CancellationToken,
    ) -> Result<Started, RankedFailure> {
        let mut refused: Option<Prepared> = None;
        let mut last_error: Option<JobError> = None;
        let mut unselectable = 0usize;
        for index in ranked.into_iter().take(MAX_CANDIDATES_TRIED) {
            if cancellation.is_cancelled() {
                return Err(RankedFailure::Failed(JobError::Cancelled));
            }
            let chosen = kept[index];
            if let Ranking::Undeclared { undeclared, .. } =
                ReleaseRanker::rank(&candidates[index], profile)
            {
                // This is the one place a fallback becomes a download, so this is where the person
                // can see why a file whose quality the name does not state was chosen.
                info!(
                    release = %chosen.raw_title,
                    undeclared = %undeclared.fields().join(","),
                    "no declared release is acceptable; using an undeclared-quality release as the fallback"
                );
            }
            let request = match self.prepare(chosen, title_name, episode).await {
                Ok(request) => request,
                Err(PrepareError::Unselectable(error)) => {
                    // The name check could not see the file list; the selector has now refused
                    // this pack. It is one failed pack attempt, and a season job may still have
                    // the episode fan-out to fall back on.
                    unselectable += 1;
                    info!(
                        release = %chosen.raw_title,
                        %error,
                        "a pack could not be narrowed to the requested season and was skipped"
                    );
                    last_error = Some(error);
                    continue;
                }
                Err(PrepareError::Failed(error)) => {
                    // One release whose metadata will not arrive is one candidate lost, not the
                    // attempt: its swarm may be dead while the next release is seeded.
                    debug!(%error, "a ranked candidate could not be prepared");
                    last_error = Some(error);
                    continue;
                }
            };
            if let Prepared::Torrent(torrent) = &request {
                let verdict = self
                    .acquisition
                    .verdict_for(torrent.job.save_path.as_str(), &torrent.description)
                    .map_err(|error| RankedFailure::Failed(JobError::failed(error.to_string())))?;
                if verdict == Verdict::Blocked {
                    refused = Some(request);
                    continue;
                }
            }
            if episode.is_none()
                && let Some(season) = self.target.season
            {
                // The chosen pack covers the whole season. The episode side's exact missing count
                // is not read here: it would cost the fan-out's own database reads to say it, and
                // the reason the pack wins is coverage, not the count.
                info!(
                    season,
                    release = %chosen.raw_title,
                    seeders = chosen.seeders.unwrap_or(0),
                    "the season pack covers every episode of the season, so it was chosen over \
                     the per-episode fan-out"
                );
            }
            return self
                .start_candidate(request)
                .await
                .map_err(RankedFailure::Failed);
        }

        // Every tried candidate was refused: persist the last verdict, so the queue shows one
        // blocked row with its reasons rather than the page showing nothing happened.
        if let Some(request) = refused {
            return self
                .start_candidate(request)
                .await
                .map_err(RankedFailure::Failed);
        }
        if unselectable > 0 {
            return Err(RankedFailure::Unselectable {
                refused: unselectable,
            });
        }
        Err(RankedFailure::Failed(last_error.unwrap_or_else(|| {
            JobError::failed("no release the quality profile accepts could be prepared")
        })))
    }

    /// The season fan-out: every missing episode of `season`, queued inside this one job.
    ///
    /// The episodes come from the same read the title screen uses, so an episode the library has no
    /// row for is not invented here. An episode with a present file is done, and one with a live
    /// queue row for this season is already being fetched; both are skipped rather than duplicated.
    /// A pack row live for the season covers every episode for the same reason.
    ///
    /// One episode's refusal is that episode's: the loop counts it, logs it, and carries on. The job
    /// succeeds when at least one episode started, and also when there was nothing left to queue
    /// because every missing episode is already live or on disk. It fails with the count only when
    /// episodes were missing and none could be queued. `already_refused` carries a pack the
    /// selector refused after its file list arrived, so that lost attempt is counted without
    /// becoming the job's outcome.
    #[allow(clippy::too_many_arguments)] // The job's identity, the search's inputs, and the carried failure count.
    async fn acquire_season_episodes(
        &self,
        job: ulid::Ulid,
        attempt: u32,
        wanted: WantedTitle<'_>,
        profile: &peregrine_domain::QualityProfile,
        season: u32,
        already_refused: usize,
        pack_refusal: Option<&str>,
        season_title_mismatches: usize,
        cancellation: &CancellationToken,
    ) -> peregrine_sync::JobResult {
        let rows = self
            .database
            .list_title_episodes(
                self.account,
                self.target.title_id,
                Some(season),
                SEASON_EPISODE_LIMIT,
            )
            .await
            .map_err(|error| {
                JobError::failed(format!("the season's episodes could not be read: {error}"))
            })?;
        let live = self.live_season(season).await?;

        let mut missing: Vec<u32> = Vec::new();
        for row in &rows {
            // Available or partial: at least one file the library can still see. `items` alone is
            // not enough, because a wanted item with no file at all has none.
            if row.files > 0 && row.missing_files < row.files {
                continue;
            }
            if live.season_covered {
                continue;
            }
            let episode = row.episode_number.get();
            if let Some(id) = self.episode_id(season, episode).await?
                && live.episodes.contains(&id)
            {
                continue;
            }
            missing.push(episode);
        }

        let total = missing.len();
        if total == 0 {
            // Every missing episode already has a live row, or the season is complete. A second
            // "download season" has nothing to do, which is an answer rather than a failure.
            info!(
                season,
                "every missing episode of the season is already queued or on disk"
            );
            return Ok(Box::new(0usize));
        }

        let mut started = 0usize;
        let mut refused = already_refused;
        let mut no_release = 0usize;
        for (processed, episode) in missing.into_iter().enumerate() {
            if cancellation.is_cancelled() {
                return Err(JobError::Cancelled);
            }
            match self
                .acquire_episode(wanted, profile, season, episode, cancellation)
                .await
            {
                Ok(EpisodeAttempt::Queued) => {
                    started += 1;
                    info!(season, episode, "an episode of the season was queued");
                }
                Ok(EpisodeAttempt::Refused { reason }) => {
                    refused += 1;
                    warn!(
                        season,
                        episode,
                        %reason,
                        "an episode of the season had releases but none could be started"
                    );
                }
                Ok(EpisodeAttempt::NoRelease) => {
                    no_release += 1;
                    debug!(
                        season,
                        episode, "the indexer returned no release naming the episode"
                    );
                }
                Err(error) => {
                    refused += 1;
                    debug!(
                        %error,
                        season,
                        episode, "an episode of the season could not be queued"
                    );
                    warn!(season, episode, "an episode of the season was refused");
                }
            }
            // A cancellation that arrived during the episode's search or add is the job's, not a
            // refusal: stop before the next episode rather than queueing past the request.
            if cancellation.is_cancelled() {
                return Err(JobError::Cancelled);
            }
            let permille = 500 + ((processed + 1) * 500 / total);
            self.phase(job, attempt, u16::try_from(permille).unwrap_or(1000))
                .await;
        }

        if started > 0 {
            Ok(Box::new(started))
        } else {
            Err(JobError::failed(no_episode_sentence(
                season,
                already_refused,
                total,
                no_release,
                refused - already_refused,
                pack_refusal,
                season_title_mismatches,
            )))
        }
    }

    /// One episode's own search, filters, ranking, gate, and start.
    ///
    /// The same path a single-episode request takes, reused per episode: the season search chose the
    /// strategy, but the release for an episode is still best found by naming it.
    #[allow(clippy::too_many_lines)] // One search, filter, rank, and start; splitting hides the pass.
    async fn acquire_episode(
        &self,
        wanted: WantedTitle<'_>,
        profile: &peregrine_domain::QualityProfile,
        season: u32,
        episode: u32,
        cancellation: &CancellationToken,
    ) -> Result<EpisodeAttempt, JobError> {
        if cancellation.is_cancelled() {
            return Err(JobError::Cancelled);
        }
        let releases = self
            .indexer
            .search(
                search_query(wanted.name, None, Some(EpisodePick { season, episode })),
                SEARCH_LIMIT,
            )
            .await
            .map_err(|error| match error {
                indexers::IndexerConfigError::Unconfigured => {
                    JobError::failed("no indexer is configured")
                }
                other => JobError::failed(format!("the indexer search failed: {other}")),
            })?;
        let ceiling = profile
            .rules()
            .size_caps()
            .ceiling()
            .unwrap_or(DEFAULT_HD_CEILING_BYTES);
        let mut candidates = Vec::new();
        let mut kept = Vec::new();
        for release in &releases {
            if release.seeders.unwrap_or(0) < self.min_seeders {
                continue;
            }
            let size_bytes = release.size_bytes.unwrap_or(0);
            if size_bytes > ceiling {
                continue;
            }
            let name = ReleaseName::parse(&release.raw_title);
            if !wanted.names(&name) {
                debug!(
                    release = %release.raw_title,
                    reason = "title-mismatch",
                    "a release was skipped because its name does not match the requested title"
                );
                continue;
            }
            if !name.is_single_episode(season, episode) {
                continue;
            }
            candidates.push(Candidate::new(name, size_bytes));
            kept.push(release);
        }
        if candidates.is_empty() {
            return Ok(EpisodeAttempt::NoRelease);
        }
        let mut ranked = ranked_candidates(&candidates, profile);
        // As in the season search: the tier leads, so language can never lift an
        // undeclared fallback above a declared candidate.
        ranked.sort_by_key(|index| {
            (
                ReleaseRanker::rank(&candidates[*index], profile).is_fallback(),
                language::rank(
                    kept[*index].raw_title.as_str(),
                    &self.preferred_audio_language,
                ),
            )
        });
        if ranked.is_empty() {
            let refused = refusal_summary(&candidates, profile);
            debug!(
                season,
                episode,
                candidates = candidates.len(),
                refused = %refused,
                "every release naming the episode was refused by the quality profile"
            );
            return Ok(EpisodeAttempt::Refused {
                reason: format!(
                    "the quality profile refused every release naming the episode ({refused})"
                ),
            });
        }
        match self
            .start_from_ranked(
                wanted.name,
                Some(EpisodePick { season, episode }),
                &candidates,
                profile,
                ranked,
                &kept,
                cancellation,
            )
            .await
        {
            Ok(_) => Ok(EpisodeAttempt::Queued),
            Err(failure) => {
                let error = failure.into_job_error();
                if matches!(error, JobError::Cancelled) {
                    return Err(error);
                }
                debug!(
                    %error,
                    season,
                    episode, "an episode release could not be started"
                );
                Ok(EpisodeAttempt::Refused {
                    reason: error.to_string(),
                })
            }
        }
    }

    /// The live queue rows this title already holds for one season.
    ///
    /// A row that names an episode is that episode and nothing else. A row that names none is only
    /// a pack when its release parses as one: the fan-out writes an episode row per episode, and
    /// treating every row with no `episode_id` as a pack would let the first of them hide every
    /// later episode of the season.
    async fn live_season(&self, season: u32) -> Result<LiveSeason, JobError> {
        let states: Vec<DownloadState> = DownloadState::ALL
            .iter()
            .copied()
            .filter(|state| !state.is_terminal())
            .collect();
        let rows = self
            .database
            .list_download_jobs(
                self.account,
                &states,
                PageRequest::first(peregrine_storage::MAX_PAGE_ROWS),
            )
            .await
            .map_err(|error| {
                JobError::failed(format!("the download queue could not be read: {error}"))
            })?;
        let mut live = LiveSeason::default();
        for row in rows {
            if row.title_id != self.target.title_id || row.season_number != Some(season) {
                continue;
            }
            if let Some(id) = row.episode_id {
                live.episodes.insert(id);
            } else if pack_like(&ReleaseName::parse(&row.release_title)) {
                live.season_covered = true;
            }
        }
        Ok(live)
    }

    /// The database row id of one episode, which is how a live queue row names it.
    async fn episode_id(&self, season: u32, episode: u32) -> Result<Option<i64>, JobError> {
        self.database
            .episode_id_for(self.target.title_id, season, episode)
            .await
            .map_err(|error| JobError::failed(format!("the episode could not be read: {error}")))
    }

    /// The episode count an absolutely numbered single-season pack has to fit.
    ///
    /// The provider's own count on the season row is what a switcher shows before any episode has
    /// been read. A library that has episode rows but no filled season falls back to those rows,
    /// because the fact a pack has to fit is the episodes the database holds. `None` means neither
    /// source has a count, and the selection refuses rather than inventing one.
    async fn season_episode_count(&self, season: u32) -> Result<Option<u32>, JobError> {
        let seasons = self
            .database
            .list_title_seasons(self.account, self.target.title_id, SEASON_LIMIT)
            .await
            .map_err(|error| {
                JobError::failed(format!(
                    "the season's episode count could not be read: {error}"
                ))
            })?;
        if let Some(count) = seasons
            .iter()
            .find(|row| row.season_number.get() == season)
            .and_then(|row| row.episode_count)
            .filter(|count| *count > 0)
        {
            return Ok(Some(count));
        }
        let episodes = self
            .database
            .list_title_episodes(
                self.account,
                self.target.title_id,
                Some(season),
                SEASON_EPISODE_LIMIT,
            )
            .await
            .map_err(|error| {
                JobError::failed(format!("the season's episodes could not be read: {error}"))
            })?;
        Ok(u32::try_from(episodes.len())
            .ok()
            .filter(|count| *count > 0))
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
                protocol: if release.source.is_torrent() {
                    "torrent".to_owned()
                } else {
                    "usenet".to_owned()
                },
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
    #[allow(clippy::too_many_lines)] // One protocol branch per source; splitting hides the order.
    async fn prepare(
        &self,
        chosen: &IndexerRelease,
        title_name: &str,
        episode: Option<EpisodePick>,
    ) -> Result<Prepared, PrepareError> {
        // The season the job row records: the episode's own when the fan-out names one, otherwise
        // the season request's. One job names one season, which is what lets the queue draw a pack
        // and an episode from the same column.
        let season_number = episode.map(|pick| pick.season).or(self.target.season);
        // The episode's own row, so a completed import links the file to the episode the card
        // reads rather than to the title alone. The fan-out passes its own pick because the job's
        // target is the season, and a row built from that alone would name no episode.
        let episode_id = match episode {
            Some(pick) => self
                .database
                .episode_id_for(self.target.title_id, pick.season, pick.episode)
                .await
                .map_err(|error| {
                    JobError::failed(format!("the episode could not be read: {error}"))
                })?,
            None => None,
        };
        // What the release name said about the transfer and the quality, stored beside the title
        // so the queue's row carries the release that was chosen rather than only its name.
        let release_source = chosen.title.source().map(|value| value.as_str().to_owned());
        let release_group = chosen.title.group().map(str::to_owned);
        let resolution = chosen
            .title
            .resolution()
            .map(|value| value.as_str().to_owned());
        let video_codec = chosen
            .title
            .video_codec()
            .map(|value| value.as_str().to_owned());
        let hdr_format = chosen.title.hdr().map(|value| value.as_str().to_owned());
        let audio = chosen.title.audio();
        let audio_summary = if audio.is_empty() {
            None
        } else {
            Some(
                audio
                    .iter()
                    .map(|codec| codec.as_str())
                    .collect::<Vec<_>>()
                    .join("+"),
            )
        };
        // What the engine is handed: a magnet when the indexer gave one, an info hash completed
        // into the same urn:btih form, or the .torrent bytes fetched from the URL Prowlarr serves
        // as its magnetUrl -- a proxy download URL, not a magnet, which the engine would try to GET.
        let mut magnet_uri = String::new();
        let engine_source = match &chosen.source {
            ReleaseSource::Torrent(IndexerSource::Magnet(uri)) => {
                magnet_uri = uri.clone();
                EngineSource::Magnet(NonEmptyText::new(uri).map_err(|error| {
                    JobError::failed(format!("the release's magnet is not usable: {error}"))
                })?)
            }
            ReleaseSource::Torrent(IndexerSource::InfoHash(hash)) => {
                magnet_uri = format!("magnet:?xt=urn:btih:{hash}");
                EngineSource::Magnet(NonEmptyText::new(&magnet_uri).map_err(|error| {
                    JobError::failed(format!("the release's magnet is not usable: {error}"))
                })?)
            }
            ReleaseSource::Torrent(IndexerSource::TorrentUrl(_)) => {
                let fetched = self
                    .indexer
                    .fetch(chosen.source.clone())
                    .await
                    .map_err(|error| {
                        JobError::failed(format!(
                            "the release's torrent could not be fetched: {error}"
                        ))
                    })?;
                match fetched {
                    FetchedRelease::TorrentBytes(bytes) => EngineSource::TorrentFile(bytes),
                    // The proxy answered 301 with a magnet, which is the source the engine wanted
                    // all along; the row keeps it, so nothing has to be re-derived from bytes.
                    FetchedRelease::Magnet(uri) => {
                        magnet_uri.clone_from(&uri);
                        EngineSource::Magnet(NonEmptyText::new(&uri).map_err(|error| {
                            JobError::failed(format!("the release's magnet is not usable: {error}"))
                        })?)
                    }
                    FetchedRelease::NzbBytes(_) => {
                        return Err(PrepareError::Failed(JobError::failed(
                            "a Usenet source answered a torrent fetch".to_owned(),
                        )));
                    }
                }
            }
            ReleaseSource::Usenet(UsenetSource::NzbUrl(_)) => {
                let fetched = self
                    .indexer
                    .fetch(chosen.source.clone())
                    .await
                    .map_err(|error| {
                        JobError::failed(format!("the release's NZB could not be fetched: {error}"))
                    })?;
                let FetchedRelease::NzbBytes(bytes) = fetched else {
                    return Err(PrepareError::Failed(JobError::failed(
                        "a torrent source answered a Usenet fetch".to_owned(),
                    )));
                };
                let document = NzbDocument::parse(&bytes).map_err(|error| {
                    JobError::failed(format!("the release's NZB is not usable: {error}"))
                })?;
                let nzb_sha256 = NzbHash::parse(&sha256_hex(&bytes)).ok_or_else(|| {
                    JobError::failed("the release's NZB has no usable checksum".to_owned())
                })?;
                let save_path = NonEmptyText::new(&folder_for(title_name)).map_err(|error| {
                    JobError::failed(format!("the destination folder is not usable: {error}"))
                })?;
                let servers =
                    crate::usenet::server_configs(&self.database, Arc::clone(&self.store))
                        .await
                        .map_err(|error| JobError::failed(error.to_string()))?;
                let job =
                    NewDownloadJob {
                        title_id: self.target.title_id,
                        episode_id,
                        season_number,
                        release_title: chosen.raw_title.clone(),
                        release_source,
                        release_group,
                        resolution,
                        video_codec,
                        hdr_format,
                        audio_summary,
                        reported_size_bytes: chosen.size_bytes,
                        info_hash: None,
                        protocol: Protocol::Usenet,
                        nzb_sha256: Some(nzb_sha256),
                        source: DownloadSource::NzbFile(NonEmptyText::new("source.nzb").map_err(
                            |_| JobError::failed("the NZB name is not usable".to_owned()),
                        )?),
                        save_path,
                        priority: 100,
                        added_by: AddedBy::Auto,
                        added_at: self.clock.now(),
                    };
                return Ok(Prepared::Usenet(Box::new(UsenetStartRequest {
                    account: self.account,
                    job,
                    document,
                    nzb_bytes: bytes,
                    servers,
                })));
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

        // A pack's own name states the season once; its entries repeat it, and a torrent that
        // bundles several seasons repeats several. Narrow the selection to the entries whose own
        // names place them in the requested season, and refuse a pack that has none before it
        // reaches admission.
        let selection = match (pack_like(&chosen.title), self.target.season) {
            (true, Some(season)) => {
                // Only an absolutely numbered pack needs the season's count, and only it should
                // pay for the read or fail when the count cannot be read: a pack whose own entries
                // name their seasons is decided without either.
                let absolute = fetched
                    .description()
                    .entries()
                    .iter()
                    .all(|entry| ReleaseName::parse(entry.path()).season().is_none());
                let episode_count = if absolute {
                    self.season_episode_count(season).await?
                } else {
                    None
                };
                let (wanted, skipped) = season_pack_selection(
                    fetched.description(),
                    &chosen.title,
                    season,
                    episode_count,
                )
                .map_err(PrepareError::Unselectable)?;
                if skipped > 0 {
                    debug!(
                        season,
                        skipped, "a season pack carried files outside the requested season"
                    );
                }
                Some(wanted)
            }
            _ => None,
        };

        let job_row = NewDownloadJob {
            title_id: self.target.title_id,
            episode_id,
            season_number,
            release_title: chosen.raw_title.clone(),
            release_source,
            release_group,
            resolution,
            video_codec,
            hdr_format,
            audio_summary,
            reported_size_bytes: chosen.size_bytes,
            info_hash: None,
            protocol: peregrine_domain::Protocol::Torrent,
            nzb_sha256: None,
            source: row_source,
            save_path,
            priority: 100,
            added_by: AddedBy::Auto,
            added_at: self.clock.now(),
        };

        Ok(Prepared::Torrent(Box::new(StartRequest {
            account: self.account,
            job: job_row,
            description: fetched.description().clone(),
            selection,
            info_hash: fetched.info_hash(),
        })))
    }

    /// Hands one prepared candidate to the acquisition service, which persists the row and either
    /// starts the transfer or parks a refusal the queue can show.
    async fn start_candidate(&self, request: Prepared) -> Result<Started, JobError> {
        let started = match request {
            Prepared::Torrent(request) => self.acquisition.start(*request).await,
            Prepared::Usenet(request) => self.acquisition.start_usenet(*request).await,
        }
        .map_err(|error| JobError::failed(error.to_string()))?;
        Ok(started)
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

/// The indexer query for a title, narrowed to one episode or one season when the target names
/// one. An episode is the narrower spelling, so it wins if both were somehow set.
fn search_query(name: &str, season: Option<u32>, episode: Option<EpisodePick>) -> String {
    match (episode, season) {
        (Some(pick), _) => format!("{name} S{:02}E{:02}", pick.season, pick.episode),
        (None, Some(season)) => format!("{name} S{season:02}"),
        (None, None) => name.to_owned(),
    }
}

/// The lower-case SHA-256 of a post, the Usenet dedupe key.
fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
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

/// The sentence a season that queued nothing reports.
///
/// It names the one fact a person can act on: whether the indexer answered at all. "No episode
/// of season 1 could be queued (48 tried)" sent the user looking for a quality setting when the
/// real answer was that all 48 per-episode searches came back empty. `pack_refusal` carries the
/// quality rules that refused the season packs the search did find, because a pack nobody could
/// select is exactly the thing that sentence would otherwise hide.
fn no_episode_sentence(
    season: u32,
    already_refused: usize,
    total: usize,
    no_release: usize,
    refused_episodes: usize,
    pack_refusal: Option<&str>,
    season_title_mismatches: usize,
) -> String {
    let mut sentence = format!(
        "no episode of season {season} could be queued ({} tried)",
        already_refused + total
    );
    if no_release == total {
        let _ = write!(
            sentence,
            ": the indexer returned no release naming any of the {total} missing episode(s)"
        );
    } else if no_release > 0 {
        let _ = write!(
            sentence,
            ": the indexer returned no release for {no_release} of the {total} missing episodes, \
             and the other {refused_episodes} were refused"
        );
    } else if refused_episodes > 0 {
        sentence.push_str(": every release the indexer returned was refused");
    }
    if let Some(pack) = pack_refusal {
        let _ = write!(
            sentence,
            "; the season pack(s) the search found were refused ({pack})"
        );
    }
    if already_refused > 0 {
        let _ = write!(
            sentence,
            "; {already_refused} season pack(s) could not be narrowed to the season"
        );
    }
    if season_title_mismatches > 0 {
        let _ = write!(
            sentence,
            "; {season_title_mismatches} release(s) the season search returned name a different title"
        );
    }
    sentence
}

/// The typed rules that refused these candidates, counted and spelled the way the audit trail
/// spells them, for a failure sentence that says *why* nothing could be used.
///
/// Only rejected candidates contribute: one the profile accepts is not a refusal, and a summary
/// that counted it would name a rule that did not run. The order is first-seen, then counted, so
/// the sentence is deterministic for a fixed search answer.
fn refusal_summary(candidates: &[Candidate], profile: &peregrine_domain::QualityProfile) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for candidate in candidates {
        let Ranking::Rejected { rejection } = ReleaseRanker::rank(candidate, profile) else {
            continue;
        };
        let label = rejection.label();
        match counts.iter_mut().find(|(existing, _)| *existing == label) {
            Some((_, count)) => *count += 1,
            None => counts.push((label, 1)),
        }
    }
    if counts.is_empty() {
        return "none".to_owned();
    }
    counts
        .into_iter()
        .map(|(label, count)| format!("{label} x{count}"))
        .collect::<Vec<_>>()
        .join(", ")
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
    use std::path::Path;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU8, Ordering};

    use jiff::Timestamp;
    use peregrine_domain::catalogue::{SeasonDetails, SeasonNumber};
    use peregrine_domain::notify::{
        Consent, Focus, NotificationEvent, NotifyError, OsNotifier, Outgoing, Platform,
    };
    use peregrine_domain::{
        AccountId, EntryKind, InfoHash, MediaFile, MediaItem, MediaItemState, MediaOrigin,
        PathGuard, PayloadEntry, TitleKind, TitleSubtype, Trash, TrashError,
    };
    use peregrine_storage::{
        AddedBy, DownloadSource, MagnetUri, NewDownloadJob, NewJob, NotificationRepository,
        StubEpisode, StubSeason, StubTitle,
    };

    use crate::acquisition::{SpaceError, VolumeSpace};
    use crate::notify::{NotificationService, StoredPreferences};
    use peregrine_testkit::{EngineCall, FakeClock, FakeTorrentEngine, MemorySecretStore};
    use peregrine_torrent::{
        Bytes, EngineFuture, FetchedMetadata, TorrentEngine, TorrentMetadataFetch, TorrentSource,
    };
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    use super::*;

    fn entry(path: &str) -> PayloadEntry {
        PayloadEntry::new(path, 1_000, EntryKind::File)
    }

    fn release(raw_title: &str, seeders: Option<u32>) -> IndexerRelease {
        IndexerRelease {
            raw_title: raw_title.to_owned(),
            title: ReleaseName::parse(raw_title),
            size_bytes: Some(1_000_000),
            seeders,
            leechers: None,
            source: ReleaseSource::Torrent(IndexerSource::Magnet(
                "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567".to_owned(),
            )),
            indexer: None,
            published: None,
            guid: None,
        }
    }

    /// The bytes a test-scoped subscriber wrote, so a test can read the info log a fallback
    /// emits without installing a global subscriber (which the process may already have).
    #[derive(Clone, Default)]
    struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for CapturedLogs {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl tracing_subscriber::fmt::MakeWriter<'_> for CapturedLogs {
        type Writer = Self;

        fn make_writer(&self) -> Self::Writer {
            self.clone()
        }
    }

    impl CapturedLogs {
        fn contents(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    #[test]
    fn a_title_query_is_the_title_name() {
        assert_eq!(search_query("Breaking Bad", None, None), "Breaking Bad");
    }

    #[test]
    fn an_episode_query_pads_its_markers_to_two_digits() {
        assert_eq!(
            search_query(
                "Breaking Bad",
                None,
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
                None,
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
    fn a_season_query_names_the_season_and_no_episode() {
        assert_eq!(
            search_query("Breaking Bad", Some(1), None),
            "Breaking Bad S01"
        );
        assert_eq!(
            search_query("Breaking Bad", Some(12), None),
            "Breaking Bad S12",
            "a season wider than two digits is not truncated"
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
        assert_eq!(dedupe_key(7, None, None), "acquisition.acquire:7");
        assert_eq!(
            dedupe_key(7, None, Some(episode_two)),
            "acquisition.acquire:7:s01e02"
        );
        assert_ne!(
            dedupe_key(7, None, Some(episode_two)),
            dedupe_key(7, None, Some(episode_three))
        );
        assert_ne!(
            dedupe_key(7, None, None),
            dedupe_key(7, None, Some(episode_two))
        );
    }

    #[test]
    fn a_season_gets_its_own_dedupe_key_and_does_not_join_an_episode() {
        let episode = EpisodePick {
            season: 2,
            episode: 1,
        };
        assert_eq!(dedupe_key(7, Some(2), None), "acquire:season:7:2");
        assert_ne!(dedupe_key(7, Some(2), None), dedupe_key(7, Some(3), None));
        assert_ne!(
            dedupe_key(7, Some(2), None),
            dedupe_key(7, None, Some(episode)),
            "a season pack and one of its episodes are different downloads"
        );
    }

    /// The season rule on a parsed name: a pack of the season or one episode of it, and nothing
    /// that names another season.
    #[test]
    fn a_season_candidate_is_that_seasons_pack_or_one_of_its_episodes() {
        let pack = ReleaseName::parse("Breaking.Bad.S02.1080p.BluRay.x264-GROUP");
        assert_eq!(season_candidate(&pack, 2), Some(SeasonCandidate::Pack));
        assert_eq!(
            season_candidate(&pack, 3),
            None,
            "a pack of another season is a different download"
        );

        let episode = ReleaseName::parse("Breaking.Bad.S02E05.1080p.BluRay.x264-GROUP");
        assert_eq!(
            season_candidate(&episode, 2),
            Some(SeasonCandidate::Episode)
        );
        assert_eq!(season_candidate(&episode, 1), None);

        let span = ReleaseName::parse("Breaking.Bad.S02E01E02.1080p.BluRay.x264-GROUP");
        assert_eq!(
            season_candidate(&span, 2),
            None,
            "a multi-episode span is neither a pack nor one episode"
        );
    }

    /// A pack that bundles two seasons writes only the requested one, and the rest is counted so
    /// the job's log says how much it discarded.
    #[test]
    fn a_multi_season_pack_keeps_only_the_wanted_seasons_entries() {
        let description = PayloadDescription::new(vec![
            entry("Show.S01E01.1080p.mkv"),
            entry("Show.S02E01.1080p.mkv"),
            entry("Show.S02E02.1080p.mkv"),
            entry("Show.S02E01.srt"),
            entry("extras/behind-the-scenes.mkv"),
        ]);
        let pack = ReleaseName::parse("Show.S01-S02.Complete.1080p.BluRay.x264-GROUP");
        let (selection, skipped) = season_pack_selection(&description, &pack, 2, None)
            .expect("the pack has files for season two");
        assert_eq!(selection, vec![1, 2, 3]);
        assert_eq!(
            skipped, 2,
            "another season and a nameless extra are skipped"
        );
    }

    /// A pack with nothing for the season is a refusal with a sentence, not an add that writes no
    /// files.
    #[test]
    fn a_pack_with_nothing_for_the_season_is_refused() {
        let description = PayloadDescription::new(vec![
            entry("Show.S01E01.1080p.mkv"),
            entry("Show.S01E02.mkv"),
        ]);
        let pack = ReleaseName::parse("Show.S01-S02.Complete.1080p.BluRay.x264-GROUP");
        let error = season_pack_selection(&description, &pack, 2, None)
            .expect_err("nothing in the pack names season two");
        assert!(error.to_string().contains("season 2"), "{error}");
        assert!(error.to_string().contains("not added"), "{error}");
    }

    /// A single-season pack whose files are numbered absolutely has no season in each name; file
    /// order is the only episode number there is, and the season's own count says whether that
    /// mapping fits.
    #[test]
    fn a_single_season_absolute_pack_maps_files_to_episodes() {
        let description = PayloadDescription::new(vec![
            entry("Peppa Pig - 01 - Muddy Puddles.mkv"),
            entry("Peppa Pig - 02 - Mr Dinosaur.mkv"),
            entry("Peppa Pig - 03 - Best Friend.mkv"),
        ]);
        let pack = ReleaseName::parse("Peppa.Pig.S01.Complete.1080p.WEB-DL.x264-GROUP");

        let (selection, skipped) = season_pack_selection(&description, &pack, 1, Some(3))
            .expect("three files are the season's three episodes");

        assert_eq!(selection, vec![0, 1, 2], "file order is episode order");
        assert_eq!(skipped, 0);
    }

    /// A multi-season pack numbered absolutely cannot say where season one stops, and the refusal
    /// has to say that rather than pretending the pack holds nothing.
    #[test]
    fn a_multi_season_absolute_pack_is_refused_with_where_season_ends() {
        let description = PayloadDescription::new(vec![
            entry("Peppa Pig - 01 - Muddy Puddles.mkv"),
            entry("Peppa Pig - 02 - Mr Dinosaur.mkv"),
            entry("Peppa Pig - 13 - The New House.mkv"),
        ]);
        let pack = ReleaseName::parse("Peppa.Pig.S01-S04.Complete.1080p.WEB-DL.x264-GROUP");

        let error = season_pack_selection(&description, &pack, 1, Some(12))
            .expect_err("the pack spans seasons it cannot divide");

        let message = error.to_string();
        assert!(
            message.contains("cannot tell where season 1 ends"),
            "{message}"
        );
        assert!(
            !message.contains("has no files for season"),
            "the generic no-files sentence is not the reason: {message}"
        );
    }

    /// The per-entry season wins whenever the files state one, even in a multi-season pack.
    #[test]
    fn an_episode_numbered_multi_season_pack_keeps_only_the_wanted_season() {
        let description = PayloadDescription::new(vec![
            entry("Show.S01E01.1080p.mkv"),
            entry("Show.S02E01.1080p.mkv"),
            entry("Show.S02E02.1080p.mkv"),
            entry("Show.S04E01.1080p.mkv"),
        ]);
        let pack = ReleaseName::parse("Show.S01-S04.Complete.1080p.BluRay.x264-GROUP");

        let (selection, skipped) =
            season_pack_selection(&description, &pack, 1, Some(10)).expect("season one is present");

        assert_eq!(selection, vec![0]);
        assert_eq!(skipped, 3);
    }

    /// A season with fewer episodes than the pack has files cannot absorb the mapping, and the
    /// refusal is a sentence rather than an add with the wrong episodes selected.
    #[test]
    fn an_absolute_pack_that_outnumbers_the_season_is_refused() {
        let description = PayloadDescription::new(vec![
            entry("Peppa Pig - 01 - Muddy Puddles.mkv"),
            entry("Peppa Pig - 02 - Mr Dinosaur.mkv"),
            entry("Peppa Pig - 03 - Best Friend.mkv"),
        ]);
        let pack = ReleaseName::parse("Peppa.Pig.S01.Complete.1080p.WEB-DL.x264-GROUP");

        let error = season_pack_selection(&description, &pack, 1, Some(2))
            .expect_err("three files do not fit a two-episode season");

        assert!(error.to_string().contains("3 files"), "{error}");
        assert!(error.to_string().contains("2 episodes"), "{error}");
    }

    /// No count is not a licence to guess: the mapping is refused until the database can say how
    /// many episodes the season has.
    #[test]
    fn an_absolute_pack_for_a_season_with_no_count_is_refused() {
        let description =
            PayloadDescription::new(vec![entry("Peppa Pig - 01 - Muddy Puddles.mkv")]);
        let pack = ReleaseName::parse("Peppa.Pig.S01.Complete.1080p.WEB-DL.x264-GROUP");

        let error =
            season_pack_selection(&description, &pack, 1, None).expect_err("no count, no mapping");

        assert!(error.to_string().contains("no episode count"), "{error}");
    }

    /// **Coverage wins.** A usable pack is the season's route however few peers it has; the
    /// fan-out is only for a search that found no usable pack. The peer count has no say in that
    /// choice, which is the live Peppa shape: a two-seeder pack beside a 24-seeder episode.
    #[test]
    fn a_usable_season_pack_wins_the_strategy_however_small_its_swarm() {
        let quiet_pack = release("Show.S02.1080p.BluRay.x264-GROUP", Some(2));
        assert_eq!(
            season_strategy(&[&quiet_pack]),
            SeasonStrategy::Pack,
            "a two-seeder pack still covers every episode of the season"
        );
        assert_eq!(
            season_strategy(&[]),
            SeasonStrategy::Episode,
            "no usable pack takes the fan-out, which reports the season's count"
        );
    }

    /// The cheap viability check: only a multi-season pack with no per-season episode markers is
    /// known to number its files absolutely, and the selector would refuse it.
    #[test]
    fn a_multi_season_pack_without_episode_markers_is_known_unusable() {
        let absolute = ReleaseName::parse("Peppa.Pig.S01-S04.Complete.1080p.WEB-DL.x264-GROUP");
        assert!(
            !pack_selection_viable(&absolute),
            "an absolute complete-series pack can never be narrowed to one season"
        );

        let numbered = ReleaseName::parse("Show.S01E01-S04E52.Complete.1080p.BluRay.x264-GROUP");
        assert!(
            pack_selection_viable(&numbered),
            "a name that carries SxxEyy can be filtered per entry"
        );
        assert_eq!(
            season_candidate(&numbered, 1),
            Some(SeasonCandidate::Pack),
            "a complete-series name with SxxEyy is still a pack"
        );

        let single = ReleaseName::parse("Peppa.Pig.S01.Complete.1080p.WEB-DL.x264-GROUP");
        assert!(
            pack_selection_viable(&single),
            "a single-season pack is decided by its own entries"
        );
    }

    /// A hash no other fixture in one test holds, spelled from one byte.
    fn hash(byte: u8) -> InfoHash {
        InfoHash::from_bytes([byte; 20])
    }

    /// The episode number a per-episode query names.
    fn queried_episode(query: &str) -> u32 {
        query
            .rsplit('E')
            .next()
            .and_then(|value| value.parse().ok())
            .unwrap_or(1)
    }

    /// One release for an episode of the fixture show.
    fn episode_release(season: u32, episode: u32, seeders: u32) -> IndexerRelease {
        release(
            &format!("Show.S{season:02}E{episode:02}.1080p.WEB-DL.x264.DTS-GROUP"),
            Some(seeders),
        )
    }

    /// One whole-season pack for the fixture show.
    fn pack_release(season: u32, seeders: u32) -> IndexerRelease {
        release(
            &format!("Show.S{season:02}.1080p.BluRay.x264.DTS-GROUP"),
            Some(seeders),
        )
    }

    /// The answer a season fan-out test gives: no usable pack, and a well-seeded release per
    /// episode.
    ///
    /// Coverage decides the strategy, so a usable pack would now be chosen; these tests are about
    /// the fan-out, which only runs when the season search found no usable pack.
    fn episodes_win(query: &str) -> Vec<IndexerRelease> {
        vec![episode_release(1, queried_episode(query), 100)]
    }

    /// The one-file payload a single-episode release resolves to.
    fn episode_description() -> PayloadDescription {
        PayloadDescription::new(vec![entry("Show.S01E01.1080p.mkv")])
    }

    /// The two-file payload a season pack resolves to.
    fn pack_description() -> PayloadDescription {
        PayloadDescription::new(vec![
            entry("Show.S01E01.1080p.mkv"),
            entry("Show.S01E02.1080p.mkv"),
        ])
    }

    /// The answer one scripted search gives.
    type Answer = dyn Fn(&str) -> Vec<IndexerRelease> + Send + Sync;

    /// The indexer port with a scripted answer, so the fan-out runs without a Prowlarr instance.
    struct ScriptedIndexer {
        answer: Box<Answer>,
        queries: Mutex<Vec<String>>,
    }

    impl ScriptedIndexer {
        fn new(answer: impl Fn(&str) -> Vec<IndexerRelease> + Send + Sync + 'static) -> Arc<Self> {
            Arc::new(Self {
                answer: Box::new(answer),
                queries: Mutex::new(Vec::new()),
            })
        }

        fn queries(&self) -> Vec<String> {
            self.queries.lock().unwrap().clone()
        }
    }

    impl ReleaseIndexer for ScriptedIndexer {
        fn search<'a>(
            &'a self,
            query: String,
            _limit: u32,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<Vec<IndexerRelease>, indexers::IndexerConfigError>>
                    + Send
                    + 'a,
            >,
        > {
            Box::pin(async move {
                self.queries.lock().unwrap().push(query.clone());
                Ok((self.answer)(&query))
            })
        }

        fn fetch<'a>(
            &'a self,
            _source: ReleaseSource,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<FetchedRelease, indexers::IndexerConfigError>>
                    + Send
                    + 'a,
            >,
        > {
            Box::pin(async { Err(indexers::IndexerConfigError::Unconfigured) })
        }
    }

    /// A metadata fetch that gives every add its own info hash, because two live rows may not share
    /// one.
    struct SequencedMetadata {
        next: AtomicU8,
        description: PayloadDescription,
    }

    impl SequencedMetadata {
        fn new(description: PayloadDescription) -> Self {
            Self {
                next: AtomicU8::new(1),
                description,
            }
        }

        /// How many metadata fetches this fixture has answered.
        fn fetches(&self) -> u8 {
            self.next.load(Ordering::Relaxed) - 1
        }
    }

    impl TorrentMetadataFetch for SequencedMetadata {
        fn fetch_metadata(&self, _source: &TorrentSource) -> EngineFuture<'_, FetchedMetadata> {
            Box::pin(async move {
                let byte = self.next.fetch_add(1, Ordering::Relaxed);
                Ok(FetchedMetadata::new(hash(byte), self.description.clone()))
            })
        }
    }

    /// The notice port for tests that are not about notifications: it records nothing.
    struct NoNotices;

    impl NotifyAcquisition for NoNotices {
        fn offers_available<'a>(
            &'a self,
            _scope: AccountScope,
            _title_id: i64,
            _label: Option<String>,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
            Box::pin(async {})
        }

        fn failed<'a>(
            &'a self,
            _scope: AccountScope,
            _title_id: i64,
            _label: Option<String>,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
            Box::pin(async {})
        }
    }

    /// The OS seam for the acquire tests: it records every toast it is handed.
    #[derive(Clone, Default)]
    struct RecordingOs {
        sent: Arc<Mutex<Vec<Outgoing>>>,
    }

    impl RecordingOs {
        fn count(&self) -> usize {
            self.sent.lock().unwrap().len()
        }
    }

    impl OsNotifier for RecordingOs {
        fn notify(&self, notification: &Outgoing) -> Result<i32, NotifyError> {
            let mut sent = self.sent.lock().unwrap();
            sent.push(notification.clone());
            Ok(i32::try_from(sent.len()).expect("a handful of toasts"))
        }
    }

    type TestNotifications = NotificationService<
        NotificationRepository,
        StoredPreferences,
        RecordingOs,
        NotificationRepository,
    >;

    /// The shipping service behind the acquire job's narrow port, as the runtime wires it.
    struct CentreNotices {
        service: Arc<TestNotifications>,
    }

    impl NotifyAcquisition for CentreNotices {
        fn offers_available<'a>(
            &'a self,
            _scope: AccountScope,
            _title_id: i64,
            _label: Option<String>,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
            Box::pin(async {})
        }

        fn failed<'a>(
            &'a self,
            scope: AccountScope,
            title_id: i64,
            label: Option<String>,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
            Box::pin(async move {
                self.service
                    .notify(
                        scope,
                        NotificationEvent::AcquisitionFailed,
                        title_id,
                        label.as_deref(),
                        Focus::default(),
                    )
                    .await
                    .expect("the acquisition failure notification is recorded");
            })
        }
    }

    /// A volume with room for every fixture payload.
    struct FreeSpace(u64);

    impl VolumeSpace for FreeSpace {
        fn available_bytes(&self, _path: &Path) -> Result<u64, SpaceError> {
            Ok(self.0)
        }
    }

    /// A trash the fan-out never reaches.
    struct NoTrash;

    impl Trash for NoTrash {
        fn move_to_trash(&self, _path: &Path) -> Result<(), TrashError> {
            Err(TrashError::Refused)
        }
    }

    fn fixture_account() -> AccountScope {
        AccountScope::unrestricted(AccountId::new(1).expect("account one is a positive id"))
    }

    /// A show with one season and the episodes a test names, plus the service and ports the job
    /// needs. Every call is deterministic: a temporary database, a fake engine, a fixed clock.
    struct FanOut {
        directory: TempDir,
        database: Arc<Database>,
        bus: Arc<EventBus>,
        engine: Arc<FakeTorrentEngine>,
        clock: Arc<FakeClock>,
        service: Arc<AcquisitionService>,
        indexer: Arc<ScriptedIndexer>,
        metadata: Arc<SequencedMetadata>,
        notifications: Arc<TestNotifications>,
        toaster: RecordingOs,
        title_id: i64,
        journal_id: i64,
    }

    impl FanOut {
        async fn new(
            episodes: &[u32],
            description: PayloadDescription,
            answer: impl Fn(&str) -> Vec<IndexerRelease> + Send + Sync + 'static,
        ) -> Self {
            Self::new_titled(TitleKind::Show, "Show", None, episodes, description, answer).await
        }

        #[allow(clippy::too_many_lines)] // One ordered fixture setup; splitting it hides the order.
        async fn new_titled(
            kind: TitleKind,
            name: &str,
            released: Option<jiff::civil::Date>,
            episodes: &[u32],
            description: PayloadDescription,
            answer: impl Fn(&str) -> Vec<IndexerRelease> + Send + Sync + 'static,
        ) -> Self {
            let directory = TempDir::new().expect("a temporary directory");
            let database = Arc::new(
                Database::open(directory.path().join("peregrine.sqlite"))
                    .await
                    .expect("the database opens"),
            );
            let now = 1_700_000_000_000;
            database
                .ensure_default_quality_profile(now)
                .await
                .expect("the default quality profile is seeded");
            let title_id = database
                .insert_stub_title(StubTitle {
                    kind,
                    subtype: TitleSubtype::Standard,
                    name: name.to_owned(),
                    original_name: None,
                    sort_name: name.to_owned(),
                    at: now,
                })
                .await
                .expect("the fixture title is inserted");
            if let Some(released) = released {
                database
                    .update_title_metadata(
                        title_id,
                        peregrine_domain::catalogue::TitleDetails {
                            kind,
                            name: NonEmptyText::new(name).expect("the fixture name"),
                            original_name: None,
                            overview: None,
                            tagline: None,
                            released: Some(released),
                            runtime_minutes: None,
                            original_language: None,
                            origin_country: None,
                            rating: None,
                            votes: None,
                            popularity: None,
                            adult: None,
                            in_production: None,
                            season_count: None,
                            episode_count: None,
                            homepage: None,
                            poster_path: None,
                            backdrop_path: None,
                        },
                        name.to_lowercase(),
                        peregrine_storage::MetadataState::Stub,
                        "test".to_owned(),
                        now,
                    )
                    .await
                    .expect("the fixture title's release date is stored");
            }
            if kind == TitleKind::Show {
                let season_id = database
                    .insert_stub_season(StubSeason {
                        title_id,
                        season_number: 1,
                        at: now,
                    })
                    .await
                    .expect("the fixture season is inserted");
                for episode in episodes {
                    database
                        .insert_stub_episode(StubEpisode {
                            title_id,
                            season_id,
                            season_number: 1,
                            episode_number: *episode,
                            at: now,
                        })
                        .await
                        .expect("the fixture episode is inserted");
                }
            }
            let journal_id = database
                .insert_job(NewJob {
                    kind: KIND.to_owned(),
                    priority: PRIORITY,
                    dedupe_key: None,
                    payload_json: "{}".to_owned(),
                    max_attempts: 1,
                    created_at: now,
                })
                .await
                .expect("the journal row is inserted");
            // The fixture's engine describes exactly the files its metadata does, so a selection
            // index the description offered is an index the engine can serve.
            let file_count = description.file_count().max(1);
            let engine =
                Arc::new(FakeTorrentEngine::new().with_files(file_count, Bytes::new(1_000)));
            let clock = Arc::new(FakeClock::default());
            let sink: Arc<dyn crate::events::EventSink> =
                Arc::new(crate::testing::RecordingSink::default());
            let bus = Arc::new(EventBus::new(sink));
            let guard = PathGuard::new(directory.path().join("downloads"))
                .expect("the fixture root is absolute");
            let volume: Arc<dyn VolumeSpace> = Arc::new(FreeSpace(1_000_000));
            let trash: Arc<dyn Trash> = Arc::new(NoTrash);
            let service = Arc::new(AcquisitionService::new(
                Arc::clone(&database),
                Arc::clone(&engine) as Arc<dyn TorrentEngine>,
                Arc::clone(&clock) as Arc<dyn Clock + Send + Sync>,
                guard,
                volume,
                1_000,
                Arc::clone(&bus),
                trash,
            ));
            let indexer = ScriptedIndexer::new(answer);
            let metadata = Arc::new(SequencedMetadata::new(description));
            database
                .set_notification_consent(Consent::granted(), clock.now())
                .await
                .expect("the fixture consents to notifications");
            let repository = NotificationRepository::new(Arc::clone(&database));
            let toaster = RecordingOs::default();
            let notifications = Arc::new(NotificationService::new(
                repository.clone(),
                StoredPreferences::new(Arc::clone(&database), Arc::new(FakeClock::default())),
                toaster.clone(),
                repository,
                Box::new(FakeClock::default()),
                Platform::MacOs,
            ));
            Self {
                directory,
                database,
                bus,
                engine,
                clock,
                service,
                indexer,
                metadata,
                notifications,
                toaster,
                title_id,
                journal_id,
            }
        }

        fn target(&self) -> Target {
            Target {
                title_id: self.title_id,
                season: Some(1),
                episode: None,
                chosen: None,
            }
        }

        async fn run(&self, target: Target) -> peregrine_sync::JobResult {
            self.run_with(target, Arc::new(NoNotices), CancellationToken::new())
                .await
        }

        async fn run_with(
            &self,
            target: Target,
            notices: Arc<dyn NotifyAcquisition>,
            cancellation: CancellationToken,
        ) -> peregrine_sync::JobResult {
            let mut job = AcquireJob::new(
                Arc::clone(&self.database),
                Arc::clone(&self.bus),
                Arc::new(MemorySecretStore::new()),
                Arc::clone(&self.indexer) as Arc<dyn ReleaseIndexer>,
                Arc::clone(&self.metadata) as Arc<dyn TorrentMetadataFetch>,
                Arc::clone(&self.service),
                Arc::clone(&self.clock) as Arc<dyn Clock + Send + Sync>,
                fixture_account(),
                self.journal_id,
                target,
                1,
                "en".to_owned(),
                notices,
            );
            job.acquire(
                ulid::Ulid::from_parts(1_700_000_000_000, 7),
                1,
                cancellation,
            )
            .await
        }

        /// The shipping notification path, as the runtime wires it.
        async fn run_in_centre(
            &self,
            target: Target,
            cancellation: CancellationToken,
        ) -> peregrine_sync::JobResult {
            let notices = Arc::new(CentreNotices {
                service: Arc::clone(&self.notifications),
            });
            self.run_with(target, notices, cancellation).await
        }

        async fn centre_rows(&self) -> Vec<crate::notify::CentreEntry> {
            self.notifications
                .centre_entries(fixture_account(), None, 50)
                .await
                .expect("the notification centre answers")
        }

        fn toasts(&self) -> usize {
            self.toaster.count()
        }

        async fn set_season_episode_count(&self, count: u32) {
            self.database
                .upsert_season_details(
                    self.title_id,
                    SeasonDetails {
                        season_number: SeasonNumber::new(1),
                        name: None,
                        overview: None,
                        air_date: None,
                        poster_path: None,
                        episode_count: Some(count),
                    },
                    "test".to_owned(),
                    self.clock.now().as_millisecond(),
                )
                .await
                .expect("the fixture season's episode count is stored");
        }

        async fn download_rows(&self) -> Vec<peregrine_storage::DownloadJobRow> {
            self.database
                .list_download_jobs(
                    fixture_account(),
                    DownloadState::ALL,
                    PageRequest::first(20),
                )
                .await
                .expect("the queue reads")
        }

        fn adds(&self) -> usize {
            self.engine
                .calls()
                .iter()
                .filter(|call| matches!(call, EngineCall::Add { .. }))
                .count()
        }

        fn metadata_fetches(&self) -> u8 {
            self.metadata.fetches()
        }

        async fn episode_id(&self, episode: u32) -> i64 {
            self.database
                .episode_id_for(self.title_id, 1, episode)
                .await
                .expect("the episode id reads")
                .expect("the fixture inserted the episode")
        }

        async fn set_available(&self, episode: u32) {
            let episode_id = self.episode_id(episode).await;
            let at = Timestamp::from_millisecond(1_700_000_000_000).expect("in range");
            let item_id = self
                .database
                .insert_media_item(MediaItem {
                    id: 0,
                    title_id: self.title_id,
                    episode_id: Some(episode_id),
                    state: MediaItemState::Available,
                    origin: MediaOrigin::Torrent,
                    preferred_file_id: None,
                    quality_label: None,
                    created_at: at,
                    updated_at: at,
                })
                .await
                .expect("the item is inserted");
            self.database
                .insert_media_file(MediaFile {
                    id: 0,
                    media_item_id: item_id,
                    path: NonEmptyText::new(&format!("/tmp/show-s01e{episode:02}.mkv"))
                        .expect("a fixture path"),
                    root_id: None,
                    size_bytes: 1_000,
                    mtime_ms: at,
                    added_at: at,
                    missing_since: None,
                    container: Some("mkv".to_owned()),
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
                    updated_at: at,
                })
                .await
                .expect("the file is inserted");
        }

        async fn create_live_row(&self, episode: u32, byte: u8) {
            let episode_id = self.episode_id(episode).await;
            let release_title = format!("Show.S01E{episode:02}.1080p.WEB-DL.x264-GROUP");
            self.create_row(Some(episode_id), &release_title, byte)
                .await;
        }

        /// A row whose release title and link are the caller's: the shape a pack has, and the
        /// shape the fan-out used to write for every episode before it carried the id.
        async fn create_row(&self, episode_id: Option<i64>, release_title: &str, byte: u8) {
            let at = Timestamp::from_millisecond(1_700_000_000_000).expect("in range");
            let save_path = self
                .directory
                .path()
                .join("downloads")
                .join("Show")
                .to_string_lossy()
                .to_string();
            self.database
                .create_download_job(NewDownloadJob {
                    title_id: self.title_id,
                    episode_id,
                    season_number: Some(1),
                    release_title: release_title.to_owned(),
                    release_source: None,
                    release_group: None,
                    resolution: None,
                    video_codec: None,
                    hdr_format: None,
                    audio_summary: None,
                    reported_size_bytes: Some(1_000),
                    info_hash: Some(hash(byte)),
                    protocol: Protocol::Torrent,
                    nzb_sha256: None,
                    source: DownloadSource::Magnet(MagnetUri::new(
                        NonEmptyText::new(&format!("magnet:?xt=urn:btih:{}", "a".repeat(40)))
                            .expect("a fixture magnet"),
                    )),
                    save_path: NonEmptyText::new(&save_path).expect("a fixture folder"),
                    priority: 100,
                    added_by: AddedBy::Auto,
                    added_at: at,
                })
                .await
                .expect("the live row is inserted");
        }
    }

    /// No usable pack is the fan-out's condition: every missing episode is searched for and
    /// queued inside the one season job.
    #[tokio::test]
    async fn a_season_with_no_usable_pack_fans_out_to_every_missing_episode() {
        let fixture = FanOut::new(&[1, 2, 3], episode_description(), |query| {
            let episode = query
                .rsplit('E')
                .next()
                .and_then(|value| value.parse().ok())
                .unwrap_or(1);
            vec![episode_release(1, episode, 100)]
        })
        .await;

        let result = fixture.run(fixture.target()).await;

        assert!(result.is_ok(), "{result:?}");
        let rows = fixture.download_rows().await;
        assert_eq!(rows.len(), 3);
        assert_eq!(fixture.adds(), 3);
        // The queue row, not just its release title, must name the episode: these are the rows
        // `Runtime::download_jobs` answers, and the DTO draws `episodeNumber` from this id.
        for episode in [1, 2, 3] {
            let id = fixture.episode_id(episode).await;
            let row = rows
                .iter()
                .find(|row| row.episode_id == Some(id))
                .unwrap_or_else(|| {
                    panic!("episode {episode}'s queue row must name its episode id")
                });
            assert!(
                row.release_title.contains(&format!("S01E{episode:02}")),
                "episode {episode} is queued from its own release: {}",
                row.release_title
            );
            assert_eq!(
                row.resolution.as_deref(),
                Some("1080p"),
                "episode {episode}'s row carries its release's quality: {row:?}"
            );
        }
        let queries = fixture.indexer.queries();
        assert_eq!(
            queries
                .iter()
                .filter(|query| query.as_str() == "Show S01")
                .count(),
            1,
            "the strategy is decided by one season search"
        );
        for episode in [1, 2, 3] {
            assert!(
                queries.contains(&format!("Show S01E{episode:02}")),
                "episode {episode} is searched for by name"
            );
        }
    }

    /// A season already queued is a satisfied request, not a failure: a second "download season"
    /// queues nothing and does not turn into the fan-out's no-episode error.
    #[tokio::test]
    async fn a_second_season_acquire_has_nothing_left_to_queue() {
        let fixture = FanOut::new(&[1, 2, 3], episode_description(), episodes_win).await;
        assert!(fixture.run(fixture.target()).await.is_ok());

        let result = fixture.run(fixture.target()).await;

        assert!(
            result.is_ok(),
            "an already queued season is not a failure: {result:?}"
        );
        assert_eq!(fixture.download_rows().await.len(), 3);
        assert_eq!(fixture.adds(), 3, "the second acquire queued nothing");
        let queries = fixture.indexer.queries();
        assert_eq!(
            queries
                .iter()
                .filter(|query| query.as_str() == "Show S01")
                .count(),
            2,
            "each request searches the season once"
        );
        assert_eq!(
            queries.iter().filter(|query| query.contains('E')).count(),
            3,
            "the second acquire did not search for an episode again"
        );
    }

    /// The queue's live set is per episode: a row that is no longer live does not hide its
    /// episode, and the rows that are still live hide only theirs.
    #[tokio::test]
    async fn a_second_season_acquire_queues_only_the_episode_that_is_not_live() {
        let fixture = FanOut::new(&[1, 2, 3], episode_description(), episodes_win).await;
        assert!(fixture.run(fixture.target()).await.is_ok());
        let episode_two = fixture.episode_id(2).await;
        let removed = fixture
            .download_rows()
            .await
            .into_iter()
            .find(|row| row.episode_id == Some(episode_two))
            .expect("episode two's row is queued");
        fixture
            .database
            .transition_download_job(removed.id, peregrine_domain::DownloadEvent::Removed)
            .await
            .expect("the row is removed");

        let result = fixture.run(fixture.target()).await;

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(
            fixture.adds(),
            4,
            "only the episode with no live row is queued again"
        );
        let rows = fixture.download_rows().await;
        assert_eq!(
            rows.len(),
            4,
            "the removed row is history beside a fresh one"
        );
        assert_eq!(
            rows.iter()
                .filter(|row| row.episode_id == Some(episode_two))
                .count(),
            2,
            "the fresh row names the same episode"
        );
        let queries = fixture.indexer.queries();
        let count = |needle: &str| {
            queries
                .iter()
                .filter(|query| query.as_str() == needle)
                .count()
        };
        assert_eq!(
            count("Show S01E01"),
            1,
            "a live episode is not searched again"
        );
        assert_eq!(count("Show S01E02"), 2, "the episode with no live row is");
        assert_eq!(
            count("Show S01E03"),
            1,
            "a live episode is not searched again"
        );
    }

    /// A genuine pack row still covers the season: one transfer is fetching every episode, so a
    /// later season acquire queues nothing.
    #[tokio::test]
    async fn a_live_pack_row_still_covers_the_season() {
        let fixture = FanOut::new(&[1, 2], episode_description(), episodes_win).await;
        fixture
            .create_row(None, "Show.S01.1080p.BluRay.x264.DTS-GROUP", 98)
            .await;

        let result = fixture.run(fixture.target()).await;

        assert!(result.is_ok(), "the pack covers every episode: {result:?}");
        assert_eq!(
            fixture.adds(),
            0,
            "the pack is already the season's download"
        );
        assert_eq!(fixture.download_rows().await.len(), 1);
        assert!(
            !fixture
                .indexer
                .queries()
                .iter()
                .any(|query| query.contains('E')),
            "no episode is searched for while the pack is live"
        );
    }

    /// The regression the fan-out wrote: a row with no episode id whose release names an episode
    /// is not a pack, so it must not hide the season's other episodes.
    #[tokio::test]
    async fn an_unlinked_episode_row_does_not_cover_the_season() {
        let fixture = FanOut::new(&[1, 2], episode_description(), episodes_win).await;
        fixture
            .create_row(None, "Show.S01E01.1080p.WEB-DL.x264.DTS-GROUP", 97)
            .await;

        let result = fixture.run(fixture.target()).await;

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(fixture.adds(), 2, "both missing episodes are still queued");
        let rows = fixture.download_rows().await;
        assert_eq!(rows.len(), 3, "the unlinked row plus one per episode");
        for episode in [1, 2] {
            let id = fixture.episode_id(episode).await;
            assert_eq!(
                rows.iter().filter(|row| row.episode_id == Some(id)).count(),
                1,
                "episode {episode} has its own queue row"
            );
        }
    }

    #[tokio::test]
    async fn an_episode_already_on_disk_is_skipped() {
        let fixture = FanOut::new(&[1, 2, 3], episode_description(), |query| {
            if query.contains('E') {
                let episode = query
                    .rsplit('E')
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(1);
                vec![episode_release(1, episode, 100)]
            } else {
                vec![episode_release(1, 1, 100)]
            }
        })
        .await;
        fixture.set_available(2).await;

        let result = fixture.run(fixture.target()).await;

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(fixture.download_rows().await.len(), 2);
        assert_eq!(fixture.adds(), 2);
        assert!(
            !fixture
                .indexer
                .queries()
                .iter()
                .any(|query| query == "Show S01E02"),
            "an available episode is not searched for"
        );
    }

    #[tokio::test]
    async fn one_refused_episode_does_not_stop_the_others() {
        let fixture = FanOut::new(&[1, 2, 3], episode_description(), |query| {
            if query.contains('E') {
                let episode = query
                    .rsplit('E')
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(1);
                if episode == 2 {
                    Vec::new()
                } else {
                    vec![episode_release(1, episode, 100)]
                }
            } else {
                vec![episode_release(1, 1, 100)]
            }
        })
        .await;

        let result = fixture.run(fixture.target()).await;

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(fixture.download_rows().await.len(), 2);
        assert_eq!(fixture.adds(), 2);
        assert!(
            fixture
                .indexer
                .queries()
                .iter()
                .any(|query| query == "Show S01E02"),
            "the refused episode was tried"
        );
        assert!(
            fixture
                .indexer
                .queries()
                .iter()
                .any(|query| query == "Show S01E03"),
            "the episode after it was still tried"
        );
    }

    /// The fan-out's warning for an episode whose releases were all refused now names
    /// the rule that refused them; the old line said only that none could be started.
    #[tokio::test]
    async fn a_refused_episode_warning_names_the_quality_rule() {
        let fixture = FanOut::new(&[1, 2], episode_description(), |query| {
            if query.contains("E02") {
                vec![release(
                    "Show.S01E02.2160p.WEB-DL.x264.DTS-GROUP",
                    Some(100),
                )]
            } else if query.contains('E') {
                vec![episode_release(1, 1, 100)]
            } else {
                Vec::new()
            }
        })
        .await;
        let logs = CapturedLogs::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_ansi(false)
            .without_time()
            .with_target(false)
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);

        let result = fixture.run(fixture.target()).await;
        drop(guard);

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(fixture.adds(), 1, "the other episode was queued");
        let text = logs.contents();
        assert!(
            text.contains("an episode of the season had releases but none could be started"),
            "{text}"
        );
        assert!(
            text.contains("resolution-not-allowed:2160p"),
            "the warning names the rule that refused the episode: {text}"
        );
    }

    #[tokio::test]
    async fn a_pack_win_still_queues_exactly_one_download() {
        let fixture = FanOut::new(&[1, 2], pack_description(), |query| {
            if query.contains('E') {
                vec![episode_release(1, 1, 5)]
            } else {
                vec![pack_release(1, 100), episode_release(1, 1, 5)]
            }
        })
        .await;

        let result = fixture.run(fixture.target()).await;

        assert!(result.is_ok(), "{result:?}");
        let rows = fixture.download_rows().await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].season_number, Some(1));
        assert_eq!(rows[0].episode_id, None, "the pack is not one episode");
        assert_eq!(fixture.adds(), 1);
        assert!(
            !fixture
                .indexer
                .queries()
                .iter()
                .any(|query| query.contains('E')),
            "a pack win must not search per episode"
        );
    }

    #[tokio::test]
    async fn an_episode_with_a_live_queue_row_is_skipped() {
        let fixture = FanOut::new(&[1, 2, 3], episode_description(), |query| {
            if query.contains('E') {
                let episode = query
                    .rsplit('E')
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(1);
                vec![episode_release(1, episode, 100)]
            } else {
                vec![episode_release(1, 1, 100)]
            }
        })
        .await;
        fixture.create_live_row(2, 99).await;

        let result = fixture.run(fixture.target()).await;

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(
            fixture.download_rows().await.len(),
            3,
            "the live row plus the two new ones"
        );
        assert_eq!(fixture.adds(), 2);
        assert!(
            !fixture
                .indexer
                .queries()
                .iter()
                .any(|query| query == "Show S01E02"),
            "an episode already in the queue is not searched for"
        );
    }

    #[tokio::test]
    async fn a_season_with_no_queueable_episode_fails_with_the_count() {
        let fixture = FanOut::new(&[1, 2], episode_description(), |query| {
            if query.contains('E') {
                Vec::new()
            } else {
                vec![episode_release(1, 1, 100)]
            }
        })
        .await;

        let error = fixture
            .run(fixture.target())
            .await
            .expect_err("nothing was queued");

        let message = error.to_string();
        assert!(
            message.contains("no episode of season 1 could be queued"),
            "{message}"
        );
        assert!(message.contains("2 tried"), "{message}");
        assert_eq!(fixture.adds(), 0);
    }

    /// A single-season pack whose files are numbered absolutely is queued with every entry
    /// selected, because the database says the season has that many episodes.
    #[tokio::test]
    async fn a_single_season_absolute_pack_is_queued_with_its_files_selected() {
        let description = PayloadDescription::new(vec![
            entry("Show - 01 - First.mkv"),
            entry("Show - 02 - Second.mkv"),
            entry("Show - 03 - Third.mkv"),
        ]);
        let fixture = FanOut::new(&[1, 2, 3], description, |query| {
            if query.contains('E') {
                vec![episode_release(1, 1, 5)]
            } else {
                vec![
                    release("Show.S01.Complete.1080p.BluRay.x264.DTS-GROUP", Some(100)),
                    episode_release(1, 1, 5),
                ]
            }
        })
        .await;
        fixture.set_season_episode_count(3).await;

        let result = fixture.run(fixture.target()).await;

        assert!(result.is_ok(), "{result:?}");
        let rows = fixture.download_rows().await;
        assert_eq!(rows.len(), 1);
        let files = fixture
            .database
            .download_files(fixture_account(), rows[0].id)
            .await
            .expect("the file rows read");
        assert_eq!(files.len(), 3);
        assert!(
            files.iter().all(|file| file.selected),
            "file order maps every entry to an episode"
        );
    }

    /// The live-app bug: a complete-series pack's 48 seeders used to beat the season's episodes,
    /// and its absolutely numbered files then made the selector refuse the whole request. The pack
    /// is skipped before the comparison and the season fans out to its episodes instead.
    #[tokio::test]
    async fn a_multi_season_absolute_pack_is_skipped_and_the_episodes_are_queued() {
        let description = PayloadDescription::new(vec![
            entry("Show - 01 - First.mkv"),
            entry("Show - 02 - Second.mkv"),
            entry("Show - 13 - Thirteenth.mkv"),
        ]);
        let fixture = FanOut::new(&[1, 2, 3], description, |query| {
            if query.contains('E') {
                vec![episode_release(1, queried_episode(query), 5)]
            } else {
                vec![
                    release(
                        "Show.S01-S04.Complete.1080p.BluRay.x264.DTS-GROUP",
                        Some(48),
                    ),
                    episode_release(1, 1, 5),
                ]
            }
        })
        .await;
        fixture.set_season_episode_count(12).await;

        let result = fixture.run(fixture.target()).await;

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(fixture.adds(), 3, "one add per episode, never the pack");
        assert_eq!(
            fixture.metadata_fetches(),
            3,
            "the pack was skipped before its file list was fetched"
        );
        let rows = fixture.download_rows().await;
        assert_eq!(rows.len(), 3);
        assert!(
            rows.iter()
                .all(|row| row.release_title.starts_with("Show.S01E")),
            "the season's episodes were queued, not the complete-series pack: {rows:?}"
        );
    }

    /// The live search shape: the 28-seeder title is a trailing run of seasons and is skipped
    /// as unusable for season 1, leaving the 2-seeder WEB pack as the season's route.
    #[tokio::test]
    async fn a_live_season_run_pack_is_skipped_and_the_web_pack_is_queued() {
        let description = PayloadDescription::new(vec![
            entry("Peppa Pig - 01 - First.mkv"),
            entry("Peppa Pig - 02 - Second.mkv"),
        ]);
        let fixture = FanOut::new_titled(
            TitleKind::Show,
            "Peppa Pig",
            None,
            &[1, 2],
            description,
            |query| {
                if query.contains('E') {
                    Vec::new()
                } else {
                    vec![
                        release("Peppa Pig S01 2 3 4 5", Some(28)),
                        release("Peppa Pig Season 1 MULTI 1080p WEB", Some(2)),
                    ]
                }
            },
        )
        .await;
        fixture.set_season_episode_count(2).await;
        let logs = CapturedLogs::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_ansi(false)
            .without_time()
            .with_target(false)
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);

        let result = fixture.run(fixture.target()).await;
        drop(guard);

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(fixture.adds(), 1, "the WEB pack is one download");
        let rows = fixture.download_rows().await;
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].release_title, "Peppa Pig Season 1 MULTI 1080p WEB",
            "the season run was skipped, not ranked"
        );
        assert_eq!(rows[0].season_number, Some(1));
        assert_eq!(rows[0].episode_id, None);
        assert!(
            !fixture
                .indexer
                .queries()
                .iter()
                .any(|query| query.contains('E')),
            "a usable pack means no episode fan-out"
        );
        let text = logs.contents();
        assert!(
            text.contains("Peppa Pig S01 2 3 4 5"),
            "the skipped season run is named: {text}"
        );
        assert!(
            text.contains("multi-season"),
            "the log says why it was skipped: {text}"
        );
    }

    /// Both packs are reachable now that a downloadUrl-only result survives the adapter, and
    /// the fully declared 18-seeder pack beats the 2-seeder WEB fallback.
    #[tokio::test]
    async fn a_fully_declared_live_pack_beats_the_web_fallback() {
        let description = PayloadDescription::new(vec![
            entry("Peppa Pig - 01 - First.mkv"),
            entry("Peppa Pig - 02 - Second.mkv"),
        ]);
        let fixture = FanOut::new_titled(
            TitleKind::Show,
            "Peppa Pig",
            None,
            &[1, 2],
            description,
            |query| {
                if query.contains('E') {
                    Vec::new()
                } else {
                    vec![
                        release(
                            "Peppa Pig (2004) Season 1 S01 (1080p Web x265 HEVC 10bit AAC 2 0 \
                             Tigole)",
                            Some(18),
                        ),
                        release("Peppa Pig Season 1 MULTI 1080p WEB", Some(2)),
                    ]
                }
            },
        )
        .await;
        fixture.set_season_episode_count(2).await;

        let result = fixture.run(fixture.target()).await;

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(fixture.adds(), 1);
        let rows = fixture.download_rows().await;
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0].release_title.starts_with("Peppa Pig (2004)"),
            "the fully declared pack wins over the WEB fallback: {rows:?}"
        );
    }

    /// A pack whose name carries `SxxEyy` markers is selectable per entry, so it is a usable pack
    /// and covers the season; its heavier swarm is not what decides the shape.
    #[tokio::test]
    async fn a_usable_pack_with_more_seeders_still_wins() {
        let fixture = FanOut::new(&[1, 2], pack_description(), |query| {
            if query.contains('E') {
                vec![episode_release(1, 1, 5)]
            } else {
                vec![
                    release(
                        "Show.S01E01-S02E02.Complete.1080p.BluRay.x264.DTS-GROUP",
                        Some(100),
                    ),
                    episode_release(1, 1, 5),
                ]
            }
        })
        .await;

        let result = fixture.run(fixture.target()).await;

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(
            fixture.metadata_fetches(),
            1,
            "the winning pack is the only candidate fetched"
        );
        let rows = fixture.download_rows().await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].episode_id, None, "the pack is not one episode");
        assert_eq!(fixture.adds(), 1);
        assert!(
            !fixture
                .indexer
                .queries()
                .iter()
                .any(|query| query.contains('E')),
            "a pack win must not search per episode"
        );
    }

    /// The live Peppa shape: a two-seeder season pack beside a 24-seeder single episode. Coverage
    /// decides a season, not the swarm, so the pack is acquired and no episode is searched.
    #[tokio::test]
    async fn a_season_pack_with_fewer_peers_than_one_episode_covers_the_season() {
        let fixture = FanOut::new(&[1, 2], pack_description(), |query| {
            if query.contains('E') {
                vec![episode_release(1, queried_episode(query), 24)]
            } else {
                vec![pack_release(1, 2), episode_release(1, 1, 24)]
            }
        })
        .await;
        let logs = CapturedLogs::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_ansi(false)
            .without_time()
            .with_target(false)
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);

        let result = fixture.run(fixture.target()).await;
        drop(guard);

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(fixture.adds(), 1, "the pack is one download");
        assert_eq!(
            fixture.metadata_fetches(),
            1,
            "the pack's file list is the only fetch"
        );
        assert!(
            !fixture
                .indexer
                .queries()
                .iter()
                .any(|query| query.contains('E')),
            "a usable pack means the episode fan-out is never reached"
        );
        let rows = fixture.download_rows().await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].episode_id, None, "the pack is not one episode");
        assert!(
            rows[0].release_title.starts_with("Show.S01.1080p.BluRay"),
            "the two-seeder pack is the season's download: {rows:?}"
        );
        let text = logs.contents();
        assert!(
            text.contains("covers every episode of the season"),
            "the info log says why the pack was chosen: {text}"
        );
        assert!(
            text.contains("Show.S01.1080p.BluRay.x264.DTS-GROUP"),
            "the info log names the chosen pack: {text}"
        );
    }

    /// The undeclared-quality fallback is a usable pack for coverage: a name that declares 1080p
    /// and WEB but no codec or audio is still chosen over a better-seeded episode.
    #[tokio::test]
    async fn a_season_uses_an_undeclared_quality_pack_beside_a_better_seeded_episode() {
        let description = PayloadDescription::new(vec![
            entry("Show - 01 - First.mkv"),
            entry("Show - 02 - Second.mkv"),
        ]);
        let fixture = FanOut::new(&[1, 2], description, |query| {
            if query.contains('E') {
                vec![episode_release(1, queried_episode(query), 24)]
            } else {
                vec![
                    release("Show Season 1 MULTI 1080p WEB", Some(2)),
                    episode_release(1, 1, 24),
                ]
            }
        })
        .await;
        fixture.set_season_episode_count(2).await;

        let result = fixture.run(fixture.target()).await;

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(fixture.adds(), 1, "the fallback pack is one download");
        let rows = fixture.download_rows().await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].release_title, "Show Season 1 MULTI 1080p WEB");
        assert_eq!(rows[0].episode_id, None, "the pack is not one episode");
        assert!(
            !fixture
                .indexer
                .queries()
                .iter()
                .any(|query| query.contains('E')),
            "the fallback pack still covers the season"
        );
    }

    /// Two packs of identical quality: the most peers decide which one is tried first, which is
    /// the only comparison seeders still make.
    #[tokio::test]
    async fn the_best_seeded_pack_wins_a_quality_tie() {
        let fixture = FanOut::new(&[1, 2], pack_description(), |_query| {
            vec![
                release("Show.S01.1080p.BluRay.x264.DTS-LOUD", Some(100)),
                release("Show.S01.1080p.BluRay.x264.DTS-QUIET", Some(2)),
            ]
        })
        .await;

        let result = fixture.run(fixture.target()).await;

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(fixture.adds(), 1);
        let rows = fixture.download_rows().await;
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0].release_title.ends_with("-LOUD"),
            "the most peers break the quality tie: {rows:?}"
        );
    }

    /// An episode request is unchanged: seeders are not a ranking key, so the better quality wins
    /// even when the other release has a far larger swarm.
    #[tokio::test]
    async fn an_episode_request_ranks_by_quality_not_peers() {
        let fixture = FanOut::new(&[1], episode_description(), |_query| {
            vec![
                release("Show.S01E01.1080p.BluRay.x264.DTS-GROUP", Some(2)),
                release("Show.S01E01.720p.BluRay.x264.DTS-GROUP", Some(200)),
            ]
        })
        .await;
        let target = Target {
            title_id: fixture.title_id,
            season: None,
            episode: Some(EpisodePick {
                season: 1,
                episode: 1,
            }),
            chosen: None,
        };

        let result = fixture.run(target).await;

        assert!(result.is_ok(), "{result:?}");
        let rows = fixture.download_rows().await;
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0].release_title.starts_with("Show.S01E01.1080p"),
            "seeders do not lift a lower-quality episode: {rows:?}"
        );
    }
    /// The name check cannot see the file list. A pack whose name carries `SxxEyy` but whose files
    /// are numbered absolutely is refused only after the fetch; that refusal is one failed
    /// candidate, and the season falls back to its episodes instead of failing the job.
    #[tokio::test]
    async fn a_pack_refused_after_the_file_list_falls_back_to_the_episodes() {
        let description = PayloadDescription::new(vec![
            entry("Show - 01 - First.mkv"),
            entry("Show - 02 - Second.mkv"),
            entry("Show - 13 - Thirteenth.mkv"),
        ]);
        let fixture = FanOut::new(&[1, 2, 3], description, |query| {
            if query.contains('E') {
                vec![episode_release(1, queried_episode(query), 5)]
            } else {
                vec![
                    release(
                        "Show.S01E01-S02E02.Complete.1080p.BluRay.x264.DTS-GROUP",
                        Some(100),
                    ),
                    episode_release(1, 1, 5),
                ]
            }
        })
        .await;
        fixture.set_season_episode_count(12).await;

        let result = fixture.run(fixture.target()).await;

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(fixture.adds(), 3, "the fan-out queued every episode");
        assert_eq!(
            fixture.metadata_fetches(),
            4,
            "the pack's file list was fetched before its refusal, then three episodes"
        );
        let rows = fixture.download_rows().await;
        assert_eq!(rows.len(), 3);
        assert!(
            rows.iter()
                .all(|row| row.release_title.starts_with("Show.S01E")),
            "the refused pack made way for the episodes: {rows:?}"
        );
    }

    /// When the pack is refused after the fetch and no episode can be queued either, the job fails
    /// with the season's existing count message, and the refused pack counts as one attempt.
    #[tokio::test]
    async fn a_season_with_no_usable_candidate_fails_with_the_count() {
        let description = PayloadDescription::new(vec![
            entry("Show - 01 - First.mkv"),
            entry("Show - 02 - Second.mkv"),
            entry("Show - 13 - Thirteenth.mkv"),
        ]);
        let fixture = FanOut::new(&[1, 2], description, |query| {
            if query.contains('E') {
                Vec::new()
            } else {
                vec![
                    release(
                        "Show.S01E01-S02E02.Complete.1080p.BluRay.x264.DTS-GROUP",
                        Some(100),
                    ),
                    episode_release(1, 1, 5),
                ]
            }
        })
        .await;
        fixture.set_season_episode_count(12).await;

        let error = fixture
            .run(fixture.target())
            .await
            .expect_err("nothing was queued");

        let message = error.to_string();
        assert!(
            message.contains("no episode of season 1 could be queued"),
            "{message}"
        );
        assert!(
            message.contains("3 tried"),
            "the refused pack is one failed attempt plus two episodes: {message}"
        );
        assert!(
            message.contains("could not be narrowed to the season"),
            "the sentence names the pack the selector refused: {message}"
        );
        assert!(
            message.contains("the indexer returned no release"),
            "the sentence names the empty episode searches: {message}"
        );
        assert_eq!(fixture.adds(), 0);
    }

    /// A season search can answer with packs only, every one of them a multi-season pack the
    /// selector cannot narrow to the season. That is not a failed season: each episode does its
    /// own search, so the fan-out takes over instead of the single-request no-match sentence.
    #[tokio::test]
    async fn a_season_search_with_only_unusable_packs_fans_out_to_the_episodes() {
        let fixture = FanOut::new(&[1, 2, 3], episode_description(), |query| {
            if query.contains('E') {
                vec![episode_release(1, queried_episode(query), 5)]
            } else {
                vec![release(
                    "Peppa.Pig.S01-S04.Complete.1080p.WEB-DL.x264-GROUP",
                    Some(48),
                )]
            }
        })
        .await;

        let result = fixture.run(fixture.target()).await;

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(fixture.adds(), 3, "one add per episode, never the pack");
        assert_eq!(fixture.download_rows().await.len(), 3);
        assert_eq!(
            fixture.metadata_fetches(),
            3,
            "the unusable pack is skipped before its file list is fetched"
        );
        for episode in [1, 2, 3] {
            assert!(
                fixture
                    .indexer
                    .queries()
                    .iter()
                    .any(|query| query == &format!("Show S01E{episode:02}")),
                "episode {episode} is searched for by name"
            );
        }
    }

    /// The same season, with every episode search empty: the fan-out's count is the honest
    /// answer, not the single-request sentence about a quality match.
    #[tokio::test]
    async fn a_season_whose_episode_searches_are_empty_fails_with_the_count() {
        let fixture = FanOut::new(&[1, 2, 3], episode_description(), |query| {
            if query.contains('E') {
                Vec::new()
            } else {
                vec![release(
                    "Peppa.Pig.S01-S04.Complete.1080p.WEB-DL.x264-GROUP",
                    Some(48),
                )]
            }
        })
        .await;

        let error = fixture
            .run(fixture.target())
            .await
            .expect_err("nothing was queued");

        let message = error.to_string();
        assert!(
            message.contains("no episode of season 1 could be queued"),
            "{message}"
        );
        assert!(message.contains("3 tried"), "{message}");
        assert!(
            message
                .contains("the indexer returned no release naming any of the 3 missing episode(s)"),
            "an empty search is not a quality refusal: {message}"
        );
        assert!(
            !message.contains("nothing matched your preferred quality"),
            "a season is not one search's no-match: {message}"
        );
        assert_eq!(fixture.adds(), 0);
    }

    /// A pack the profile refuses still wins the peer comparison, because seeders decide the
    /// strategy and quality decides the ranking. The empty ranking is the same dead end the
    /// fan-out exists to avoid, so the episodes are queued rather than a no-match reported.
    #[tokio::test]
    async fn a_season_pack_the_profile_refuses_falls_back_to_the_episodes() {
        let fixture = FanOut::new(&[1, 2], episode_description(), |query| {
            if query.contains('E') {
                vec![episode_release(1, queried_episode(query), 5)]
            } else {
                vec![
                    release("Show.S01.2160p.WEB-DL.x264.DTS-GROUP", Some(100)),
                    episode_release(1, 1, 5),
                ]
            }
        })
        .await;

        let result = fixture.run(fixture.target()).await;

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(fixture.adds(), 2, "the episodes were queued, not the pack");
        let rows = fixture.download_rows().await;
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter()
                .all(|row| row.release_title.starts_with("Show.S01E")),
            "the refused pack made way for the episodes: {rows:?}"
        );
    }

    /// The live "Her" bug at the acquire layer: a release whose name merely contains another
    /// title's word is not a candidate, so the right release wins even though the impostor is far
    /// better seeded.
    #[tokio::test]
    async fn a_wrong_title_release_does_not_beat_the_right_one() {
        let fixture = FanOut::new(&[1, 2], pack_description(), |query| {
            if query.contains('E') {
                vec![episode_release(1, queried_episode(query), 100)]
            } else {
                vec![
                    release("Peppa.Pig.S01.1080p.BluRay.x264.DTS-GROUP", Some(100)),
                    release("Show.S01.1080p.BluRay.x264.DTS-GROUP", Some(5)),
                    episode_release(1, 1, 1),
                ]
            }
        })
        .await;

        let result = fixture.run(fixture.target()).await;

        assert!(result.is_ok(), "{result:?}");
        let rows = fixture.download_rows().await;
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0].release_title.starts_with("Show.S01"),
            "the better-seeded release named another title: {rows:?}"
        );
        assert_eq!(fixture.adds(), 1);
        assert_eq!(fixture.metadata_fetches(), 1);
    }

    /// A one-word film title is ambiguous, so the catalogue year decides: the better-quality
    /// release that states another year is not a candidate, even though its name contains "Her".
    #[tokio::test]
    async fn a_film_short_title_uses_the_catalogue_year() {
        let fixture = FanOut::new_titled(
            TitleKind::Movie,
            "Her",
            Some("2013-12-18".parse().expect("a date")),
            &[],
            episode_description(),
            |_| {
                vec![
                    release("Her.2014.1080p.BluRay.x264.DTS-GROUP", Some(100)),
                    release("Her.2013.720p.BluRay.x264.DTS-GROUP", Some(5)),
                ]
            },
        )
        .await;

        let result = fixture
            .run(Target {
                title_id: fixture.title_id,
                season: None,
                episode: None,
                chosen: None,
            })
            .await;

        assert!(result.is_ok(), "{result:?}");
        let rows = fixture.download_rows().await;
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0].release_title.starts_with("Her.2013"),
            "the release with the wrong year was a candidate: {rows:?}"
        );
    }

    /// The exact live "Her" failure: a film request's search returned an anime episode whose
    /// name contains the word, and it must not be a candidate however well seeded it is.
    #[tokio::test]
    async fn an_anime_episode_is_not_a_film_candidate() {
        let fixture = FanOut::new_titled(
            TitleKind::Movie,
            "Her",
            Some("2013-12-18".parse().expect("a date")),
            &[],
            episode_description(),
            |_| {
                vec![release(
                    "The Forsaken Saintess and Her Foodie Roadtrip in Another World S01E10 1080p \
                     AMZN WEB-DL DUAL DDP2.0 H.264-VARYG (Dual-Audio)",
                    Some(100),
                )]
            },
        )
        .await;

        let error = fixture
            .run(Target {
                title_id: fixture.title_id,
                season: None,
                episode: None,
                chosen: None,
            })
            .await
            .expect_err("the anime episode is not the film");

        let message = error.to_string();
        assert!(
            message.contains("no release names the requested title"),
            "{message}"
        );
        assert_eq!(fixture.adds(), 0);
    }

    /// A title request whose every answer names another title says so: the person's next move is
    /// a different search, not a different quality profile.
    #[tokio::test]
    async fn a_search_with_only_wrong_title_releases_names_the_mismatch() {
        let fixture = FanOut::new(&[1], episode_description(), |_query| {
            vec![release(
                "Peppa.Pig.S01E01.1080p.BluRay.x264.DTS-GROUP",
                Some(100),
            )]
        })
        .await;

        let error = fixture
            .run(Target {
                title_id: fixture.title_id,
                season: None,
                episode: None,
                chosen: None,
            })
            .await
            .expect_err("no release named the title");

        let message = error.to_string();
        assert!(
            message.contains("no release names the requested title"),
            "{message}"
        );
        assert!(message.contains("different title"), "{message}");
        assert_eq!(fixture.adds(), 0);
    }

    /// The live Peppa Pig case: the season search answers with one two-seeder pack whose name
    /// declares 1080p and WEB but no video codec and no audio, and every per-episode search is
    /// empty. That pack is an undeclared-quality fallback, not a refusal, so the season is queued
    /// as the pack and its absolutely numbered files are narrowed to the season.
    #[tokio::test]
    async fn a_season_whose_only_pack_declares_no_codec_or_audio_is_queued_as_that_season() {
        let description = PayloadDescription::new(vec![
            entry("Show - 01 - First.mkv"),
            entry("Show - 02 - Second.mkv"),
        ]);
        let fixture = FanOut::new(&[1, 2], description, |query| {
            if query.contains('E') {
                Vec::new()
            } else {
                vec![release("Show Season 1 MULTI 1080p WEB", Some(2))]
            }
        })
        .await;
        fixture.set_season_episode_count(2).await;

        let result = fixture.run(fixture.target()).await;

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(fixture.adds(), 1, "the pack is one download");
        assert_eq!(
            fixture.metadata_fetches(),
            1,
            "the pack's file list is the only fetch"
        );
        assert!(
            !fixture
                .indexer
                .queries()
                .iter()
                .any(|query| query.contains('E')),
            "a pack win must not search per episode"
        );
        let rows = fixture.download_rows().await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].release_title, "Show Season 1 MULTI 1080p WEB");
        assert_eq!(rows[0].season_number, Some(1));
        assert_eq!(rows[0].episode_id, None, "the pack is not one episode");
        let files = fixture
            .database
            .download_files(fixture_account(), rows[0].id)
            .await
            .expect("the file rows read");
        assert_eq!(files.len(), 2, "only the season's two files are selected");
        assert!(
            files.iter().all(|file| file.selected),
            "file order maps every entry to an episode of season one"
        );
    }

    /// The live log's refusal: the only season pack declares 1080p but no source, no
    /// video codec, and no audio. A missing source is unknown like the other two, so
    /// the pack is the season's route instead of a `source-not-declared` refusal.
    #[tokio::test]
    async fn a_season_whose_only_pack_declares_no_source_is_queued_as_that_season() {
        let description = PayloadDescription::new(vec![
            entry("Show - 01 - First.mkv"),
            entry("Show - 02 - Second.mkv"),
        ]);
        let fixture = FanOut::new(&[1, 2], description, |query| {
            if query.contains('E') {
                Vec::new()
            } else {
                vec![release("Show Season 1 MULTI 1080p", Some(2))]
            }
        })
        .await;
        fixture.set_season_episode_count(2).await;
        let logs = CapturedLogs::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_ansi(false)
            .without_time()
            .with_target(false)
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);

        let result = fixture.run(fixture.target()).await;
        drop(guard);

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(fixture.adds(), 1, "the pack is one download");
        assert_eq!(
            fixture.metadata_fetches(),
            1,
            "the pack's file list is the only fetch"
        );
        assert!(
            !fixture
                .indexer
                .queries()
                .iter()
                .any(|query| query.contains('E')),
            "a pack win must not search per episode"
        );
        let rows = fixture.download_rows().await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].release_title, "Show Season 1 MULTI 1080p");
        assert_eq!(rows[0].season_number, Some(1));
        assert_eq!(rows[0].episode_id, None, "the pack is not one episode");
        let files = fixture
            .database
            .download_files(fixture_account(), rows[0].id)
            .await
            .expect("the file rows read");
        assert_eq!(files.len(), 2, "only the season's two files are selected");
        assert!(
            files.iter().all(|file| file.selected),
            "file order maps every entry to an episode of season one"
        );
        let text = logs.contents();
        assert!(
            text.contains("undeclared=source,video_codec,audio"),
            "the info log names the source the name omits: {text}"
        );
    }

    /// A value the profile disallows is still a hard refusal: a 2160p pack is never the fallback,
    /// however much of the rest of its name is undeclared, and the empty fan-out names that rule.
    #[tokio::test]
    async fn a_season_pack_the_profile_refuses_is_named_in_the_failure() {
        let fixture = FanOut::new(&[1, 2], episode_description(), |query| {
            if query.contains('E') {
                Vec::new()
            } else {
                vec![release("Show.S01.2160p.WEB-DL-GROUP", Some(100))]
            }
        })
        .await;

        let error = fixture
            .run(fixture.target())
            .await
            .expect_err("nothing was queued");

        let message = error.to_string();
        assert!(
            message.contains("no episode of season 1 could be queued"),
            "{message}"
        );
        assert!(
            message.contains("resolution-not-allowed"),
            "the sentence must name the rule that refused the pack: {message}"
        );
        assert!(
            message.contains("the indexer returned no release"),
            "the sentence must name the empty episode searches: {message}"
        );
        assert_eq!(fixture.adds(), 0);
        assert_eq!(fixture.metadata_fetches(), 0);
    }

    /// The live pack's exact quality shape on a title request: 1080p and WEB-DL are declared, the
    /// video codec and the audio are not. The release is a fallback, it is chosen when nothing
    /// declared exists, and the info log names the release and both undeclared fields.
    #[tokio::test]
    async fn a_title_with_only_an_undeclared_quality_release_queues_it_and_logs_the_fallback() {
        let fixture = FanOut::new(&[1], episode_description(), |_query| {
            vec![release("Show.1080p.WEB-DL-GROUP", Some(40))]
        })
        .await;
        let logs = CapturedLogs::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_ansi(false)
            .without_time()
            .with_target(false)
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);

        let result = fixture
            .run(Target {
                title_id: fixture.title_id,
                season: None,
                episode: None,
                chosen: None,
            })
            .await;
        drop(guard);

        assert!(result.is_ok(), "{result:?}");
        let rows = fixture.download_rows().await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].release_title, "Show.1080p.WEB-DL-GROUP");
        let text = logs.contents();
        assert!(
            text.contains("Show.1080p.WEB-DL-GROUP"),
            "the log names the release: {text}"
        );
        assert!(
            text.contains("undeclared=video_codec,audio"),
            "the log names the fields the name does not declare: {text}"
        );
        assert!(
            text.contains("fallback"),
            "the log says this was the fallback, not the preference: {text}"
        );
    }

    /// A declared candidate always wins, even when the undeclared one has more seeders and is in
    /// the preferred language: neither seeders nor the language pass may lift a fallback.
    #[tokio::test]
    async fn a_declared_release_beats_a_better_seeded_undeclared_one() {
        let fixture = FanOut::new(&[1], episode_description(), |_query| {
            vec![
                release("Show.1080p.WEB-DL-MULTi", Some(500)),
                release("Show.720p.WEB-DL.x264.AAC-FRENCH", Some(5)),
            ]
        })
        .await;

        let result = fixture
            .run(Target {
                title_id: fixture.title_id,
                season: None,
                episode: None,
                chosen: None,
            })
            .await;

        assert!(result.is_ok(), "{result:?}");
        let rows = fixture.download_rows().await;
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0].release_title.starts_with("Show.720p"),
            "the declared release is the only acceptable preference: {rows:?}"
        );
    }

    /// An episode request has no fan-out behind it, so an empty ranking keeps the original
    /// sentence that says a quality match was what failed.
    #[tokio::test]
    async fn an_episode_search_with_no_candidates_keeps_the_no_match_sentence() {
        let fixture = FanOut::new(&[1], episode_description(), |_query| Vec::new()).await;
        let target = Target {
            title_id: fixture.title_id,
            season: None,
            episode: Some(EpisodePick {
                season: 1,
                episode: 1,
            }),
            chosen: None,
        };

        let error = fixture
            .run(target)
            .await
            .expect_err("no release was available");

        let message = error.to_string();
        assert!(
            message.contains("nothing matched your preferred quality"),
            "{message}"
        );
        assert!(
            !message.contains("no episode of season"),
            "an episode request is not a season fan-out: {message}"
        );
        assert_eq!(fixture.adds(), 0);
    }

    /// An acquire that finds nothing is a notification: it wrote no download row, so the centre
    /// is the only place the person can read what happened.
    #[tokio::test]
    async fn a_failed_season_fan_out_raises_one_notification_naming_the_title() {
        let fixture = FanOut::new(&[1, 2], episode_description(), |_query| Vec::new()).await;

        let result = fixture
            .run_in_centre(fixture.target(), CancellationToken::new())
            .await;

        assert!(result.is_err(), "nothing was queued");
        let rows = fixture.centre_rows().await;
        assert_eq!(rows.len(), 1, "one job raises one entry");
        assert_eq!(rows[0].event, "acquisition.failed");
        assert_eq!(
            rows[0].title, "Show",
            "the title is what the person asked for"
        );
        assert_eq!(
            rows[0].deep_link.as_deref(),
            Some("peregrine://downloads"),
            "the entry opens the queue that can retry it"
        );
        assert_eq!(fixture.toasts(), 1, "the OS is offered the same event");
    }

    /// The single-episode path ends in the same failure and owes the same notice.
    #[tokio::test]
    async fn a_failed_single_episode_acquire_raises_one_notification() {
        let fixture = FanOut::new(&[1], episode_description(), |_query| Vec::new()).await;
        let target = Target {
            title_id: fixture.title_id,
            season: None,
            episode: Some(EpisodePick {
                season: 1,
                episode: 1,
            }),
            chosen: None,
        };

        let result = fixture
            .run_in_centre(target, CancellationToken::new())
            .await;

        assert!(result.is_err(), "no release named the episode");
        let rows = fixture.centre_rows().await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].event, "acquisition.failed");
        assert_eq!(rows[0].title, "Show");
        assert_eq!(fixture.toasts(), 1);
    }

    /// The person's own stop is not a failure to report: a cancelled job raises nothing.
    #[tokio::test]
    async fn a_cancelled_acquire_raises_no_notification() {
        let fixture = FanOut::new(&[1, 2], episode_description(), |_query| Vec::new()).await;
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let result = fixture.run_in_centre(fixture.target(), cancellation).await;

        assert!(matches!(result, Err(JobError::Cancelled)), "{result:?}");
        assert_eq!(
            fixture.centre_rows().await,
            Vec::new(),
            "a cancelled job raises no notification"
        );
        assert_eq!(fixture.toasts(), 0);
    }
}
