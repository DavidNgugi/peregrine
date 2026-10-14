//! Filling the Discover screen's rails, and fetching the artwork that makes them worth drawing.
//!
//! A job rather than a command because it leaves the machine seven times per provider — one request
//! per rail — and `ARCHITECTURE §7` rule 2 puts anything that slow behind the scheduler. It is also
//! the job that exists so the *read* does not have to: [`HomeRails`] is a cache the runtime answers
//! from, and this is the only writer (`FLOWS F2`: the screen paints from what is here, and the
//! network work is a job whose completion re-reads).
//!
//! # The two phases, and why the second needs no budget
//!
//! A rail answer is a list of *candidates* — an id, a name, a kind, a date, and, from a provider
//! whose list endpoints send them, the poster's path and the title's score. The screen this exists
//! for is a wall of posters, so phase one syncs every candidate into the catalogue, artwork and
//! score included, and publishes the rails; phase two fetches each title's poster into the blob
//! cache, publishing again with what landed.
//!
//! **Phase two used to stop after a fixed number of titles, and that bound was the defect.** It
//! existed because a candidate carried no artwork, so learning a poster's path cost a *details*
//! request per title — two requests per tile, and a budget of twenty-four kept the wall four-fifths
//! grey placeholders forever, because there was never a later run to fill the rest in. A list answer
//! already carries the path (`PG-E7-08`), so the second request is gone, every rail item is filled
//! in one run, and what paces the requests is the per-host token bucket the provider client already
//! enforces (`AGENTS R24`) rather than a constant here. A title whose poster the cache already holds
//! costs a `stat` and no request, which is what keeps a second visit cheap.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;
use std::sync::{Arc, RwLock};

use peregrine_domain::Clock;
use peregrine_domain::catalogue::{ImageKind, TitleKind};
use peregrine_domain::matching::{Candidate, MatchDecision, match_candidate, sort_name};
use peregrine_domain::text::NonEmptyText;
use peregrine_providers::error::ProviderError;
use peregrine_providers::provider::{MetadataProvider, RailKind, SearchCandidate};
use peregrine_storage::{
    ArtworkPath, Database, JobChange, JobPhase, ProviderTitle, StorageError, TitleExternalId,
};
use peregrine_sync::{Job, JobContext, JobError, JobFuture, JobOutput};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

use crate::events::{Event, EventBus, JOB_PROGRESS, JOB_STATE};
use crate::providers::ProviderRegistry;

/// The kind this job registers under, in the scheduler and in the journal.
pub const KIND: &str = "discover.rails";

/// Below the user-asked work and above background maintenance, for the reason a title refresh is:
/// a screen is waiting, and nothing is on fire.
pub const PRIORITY: i32 = 10;

/// One title as a rail shows it.
///
/// The row id is the stable key a screen navigates with, and the name and year are what the
/// catalogue holds. Mapping a row into this is the core's business rather than a screen's
/// (`AGENTS R7`), which is why the DTO the IPC layer answers with is built from it.
///
/// **Everything a list answer already gave is here, and nothing is invented.** A provider's rail
/// answer carries a poster, a backdrop, a score, a synopsis, and a vote count for each item, and the
/// hover preview (`PG-E7-05`) draws four of them while Discover's hero draws the wide one — so all
/// five ride the item rather than costing a second request per tile. What it does *not* carry is a
/// runtime or a genre: TMDB's list endpoints send neither, and a panel that guessed one would be
/// stating something no provider said.
///
/// **Which of the three artwork fields is for whom** is worth stating, because two of them are
/// plumbing: [`RailItem::backdrop_source`] is what the artwork phase fetches *from*,
/// [`RailItem::poster`] and [`RailItem::backdrop`] are where the cache put them, and only the last
/// two plus [`RailItem::playable`] cross IPC.
#[derive(Debug, Clone, PartialEq)]
pub struct RailItem {
    /// The catalogue row, which is what opening the title needs.
    pub title_id: i64,
    /// Film or series.
    pub kind: TitleKind,
    /// The name to draw.
    pub name: String,
    /// The release year, when the provider or the catalogue stated one.
    pub year: Option<i32>,
    /// Where the cached poster is on this machine, or `None` while it has not been fetched.
    pub poster: Option<std::path::PathBuf>,
    /// The backdrop a list answer named — the provider that serves it and the path it spells.
    ///
    /// An input to the artwork phase rather than something a screen reads: the screen is given
    /// [`RailItem::backdrop`], the cached blob, for the reason `AGENTS R11` gives about bytes and
    /// paths. It is kept on the item because the same run that learns the path is the run that
    /// fetches it, and a second lookup of a stored column the title row does not expose would be a
    /// read for something already in hand.
    pub backdrop_source: Option<peregrine_storage::ArtworkPath>,
    /// Where the cached backdrop is on this machine, or `None` while it has not been fetched.
    ///
    /// The wide image Discover's hero draws behind a title's copy. `None` is a state the hero draws
    /// — the poster cropped to the band, or the reserved surface when there is no poster either —
    /// rather than a reason to stretch a portrait image across it.
    pub backdrop: Option<std::path::PathBuf>,
    /// Whether the library holds a file for this title that is not known to be missing.
    ///
    /// **A fact about the library and not a promise about playback**: it says a blob is on disk, not
    /// that it decodes or that a player exists (`PG-E15` owns playing). It is here because the one
    /// control that must not lie is Play, and a screen that decided this for itself would be reading
    /// the library from the wrong layer (`AGENTS R7`).
    pub playable: bool,
    /// The provider's own score for the title, when the list answer carried one, so the card a
    /// screen draws can show its badge without a second read.
    pub rating: Option<f64>,
    /// The synopsis the list answer carried, when it carried one, for the preview's prose.
    pub overview: Option<String>,
    /// How many people voted on it, when the list answer counted them.
    pub votes: Option<u64>,
}

/// How many of the first rail's titles a run fetches a backdrop for.
///
/// **A backdrop is a whole-screen image and four times a poster's bytes** (`PG-E6-09`'s variants,
/// `w780` against `w342`), and only the hero's rotation draws one — so fetching one for every item of
/// every rail would spend tens of megabytes on a strip that shows one at a time. Eight is the hero's
/// rotation with room for the one that failed: the screen rotates through at most five
/// (`hero-rotation.ts`'s cap), and a title missing its backdrop is a reserved box the hero draws
/// anyway.
const HERO_BACKDROP_LIMIT: usize = 8;

/// What the Discover screen reads: every rail, as the catalogue and the cache last answered.
///
/// **A `RwLock` in the runtime rather than a table, and the cost is stated plainly: a cold start
/// shows every rail empty until the first fetch lands.** What that buys is a read that cannot fail,
/// cannot block, and cannot touch the network — which is what `FLOWS F2` requires of the first
/// paint — and a rail that is *known* to exist even when its membership is not, so the screen's
/// geometry is defined from the first frame (`DESIGN_SYSTEM §11` rule 2: no spinner, ever).
///
/// The follow-up is a persisted manifest: a `discover_rail` table keyed by rail and position, an
/// append-only numbered migration, and a repository with its own round-trip and scope tests. That
/// is a slice of its own — nothing here pretends to have done it halfway.
#[derive(Debug, Default)]
pub struct HomeRails {
    /// The rails as last published. A `BTreeMap` keyed by the enum, so the read order is the
    /// enum's — which is the order a home screen shows them — rather than an insertion order that
    /// would depend on which provider answered first.
    rails: RwLock<BTreeMap<RailKind, Vec<RailItem>>>,
}

impl HomeRails {
    /// Rails nothing has been published for yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every rail, in the order a screen shows them, with the ones nobody has fetched yet as empty
    /// lists.
    ///
    /// **A rail is reported whether or not it has membership**, and that is the whole of the
    /// "defined geometry" rule: a screen that omitted a rail until it had content would move every
    /// heading below it the moment the fetch landed.
    ///
    /// # Panics
    ///
    /// Panics when the lock is poisoned, which means a writer panicked while holding it — a bug in
    /// this type rather than anything a caller can cause.
    #[must_use]
    pub fn read(&self) -> Vec<(RailKind, Vec<RailItem>)> {
        let rails = self.rails.read().expect("the lock is not poisoned");
        RailKind::ALL
            .into_iter()
            .map(|rail| (rail, rails.get(&rail).cloned().unwrap_or_default()))
            .collect()
    }

    /// Replaces what is published.
    ///
    /// A whole-map write rather than a per-rail one: a run fetches a set of rails, and a screen
    /// that read a half-updated map would draw two runs' answers side by side.
    ///
    /// # Panics
    ///
    /// Panics when the lock is poisoned, for the reason [`HomeRails::read`] does.
    fn publish(&self, rails: &BTreeMap<RailKind, Vec<RailItem>>) {
        *self.rails.write().expect("the lock is not poisoned") = rails.clone();
    }
}

/// Fetches the rails the configured providers publish and fills the catalogue and the cache.
pub struct DiscoverRailsJob {
    database: Arc<Database>,
    bus: Arc<EventBus>,
    clock: Arc<dyn Clock + Send + Sync>,
    providers: Arc<ProviderRegistry>,
    artwork: Arc<peregrine_sync::ImageCache>,
    rails: Arc<HomeRails>,
    journal_id: i64,
}

impl DiscoverRailsJob {
    /// A job that fills every rail this installation's providers publish.
    #[must_use]
    pub fn new(
        database: Arc<Database>,
        bus: Arc<EventBus>,
        clock: Arc<dyn Clock + Send + Sync>,
        providers: Arc<ProviderRegistry>,
        artwork: Arc<peregrine_sync::ImageCache>,
        rails: Arc<HomeRails>,
        journal_id: i64,
    ) -> Self {
        Self {
            database,
            bus,
            clock,
            providers,
            artwork,
            rails,
            journal_id,
        }
    }

    /// Reports how far along the run is, so a screen following it can say so.
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
}

impl Job for DiscoverRailsJob {
    fn run(&mut self, context: &JobContext<'_>) -> JobFuture<'_> {
        // Copied out of the context before the future is built, because a future that held the
        // context borrow would have to live as long as this job does.
        let job = context.id();
        let attempt = context.attempt();
        let cancellation = context.cancellation().clone();
        Box::pin(self.fetch(job, attempt, cancellation))
    }
}

impl DiscoverRailsJob {
    /// The work, owning everything it borrows so the returned future borrows only `self`.
    async fn fetch(
        &mut self,
        job: ulid::Ulid,
        attempt: u32,
        cancellation: CancellationToken,
    ) -> peregrine_sync::JobResult {
        let adapters = self.providers.adapters();
        if adapters.is_empty() {
            return Err(JobError::failed("no provider is configured"));
        }
        self.run(job, attempt, &adapters, cancellation).await
    }

    /// The work over an explicit set of providers.
    ///
    /// Split out from [`DiscoverRailsJob::fetch`] so the job's body can be driven with a scripted
    /// provider: the registry is built from a keyring this crate's tests may not read (`AGENTS R14`),
    /// and a job whose only tested path was "no provider is configured" would be a job nobody had run.
    async fn run(
        &mut self,
        job: ulid::Ulid,
        attempt: u32,
        adapters: &[Arc<dyn MetadataProvider>],
        cancellation: CancellationToken,
    ) -> peregrine_sync::JobResult {
        let at = self.clock.now().as_millisecond();

        self.phase(job, attempt, 50).await;
        // **A rail one provider cannot serve is not a failure of the run.** Providers differ in
        // what they publish — that is what `Capability::Trending` and the rail vocabulary exist
        // for — so a refusal is skipped and the loop moves on.
        let mut rails: BTreeMap<RailKind, Vec<RailItem>> = BTreeMap::new();
        let mut answered = 0_usize;
        let mut failures = Vec::new();
        for adapter in adapters {
            for rail in RailKind::ALL {
                if cancellation.is_cancelled() {
                    return Err(JobError::Cancelled);
                }
                match self
                    .sync_rail(adapter.as_ref(), rail, at, &cancellation)
                    .await
                {
                    Ok(items) => {
                        answered += 1;
                        rails.entry(rail).or_default().extend(items);
                    }
                    Err(RailFailure::Refused) => {
                        debug!(
                            rail = rail.as_str(),
                            "a provider does not publish this rail"
                        );
                    }
                    Err(RailFailure::Unavailable(error)) => {
                        failures.push(format!("{}: {error}", rail.as_str()));
                    }
                    Err(RailFailure::Cancelled) => return Err(JobError::Cancelled),
                }
            }
        }

        // Published before any artwork is fetched, so the names and years a rail already knows are
        // on screen while the posters are still arriving. What this publish cannot know is the
        // library's answer about each title — whether a file is there, and where the cached artwork
        // went — so it says `None` and "nothing to play", and the publish below corrects both. A read
        // that lands in the window between them sees the conservative answer rather than an invented
        // one; a screen that reads after the run, which is when the follow re-reads, sees the truth.
        self.rails.publish(&rails);
        self.phase(job, attempt, 500).await;

        let artwork = self
            .prefetch_artwork(job, attempt, &rails, adapters, &cancellation)
            .await?;
        if !artwork.is_empty() {
            let mut with_artwork = rails.clone();
            for items in with_artwork.values_mut() {
                for item in items.iter_mut() {
                    if let Some(found) = artwork.get(&item.title_id) {
                        item.poster = found.poster.clone();
                        item.backdrop = found.backdrop.clone();
                        item.playable = found.playable;
                    }
                }
            }
            self.rails.publish(&with_artwork);
        }

        // Every provider refused every rail: there is nothing this run can do, and a screen that
        // pressed Refresh deserves to know why rather than to see the same empty rows again.
        if answered == 0 && !failures.is_empty() {
            return Err(JobError::failed(failures.join("; ")));
        }

        self.succeeded(job, attempt, rails.values().map(Vec::len).sum())
            .await
    }

    /// Reads one rail from one provider and writes what it answered into the catalogue.
    ///
    /// The upsert is the shape `peregrine_sync::sync_search` uses — identity first, then the
    /// matching rule, then a stub — because a rail item *is* a
    /// [`SearchCandidate`], and there is no second title shape to invent. The sync service's
    /// `sync_search` takes a [`SearchQuery`](peregrine_providers::SearchQuery) and a rail has none,
    /// and it leaves a title it already holds alone rather than writing the artwork and score a
    /// list answer carries, which is the one thing this path has to do; `record_list_answer` is
    /// that write, so the rule it follows (`COALESCE`, so an absent field never clears a stored
    /// one) is the sync's rather than this job's.
    async fn sync_rail(
        &self,
        provider: &dyn MetadataProvider,
        rail: RailKind,
        at: i64,
        cancellation: &CancellationToken,
    ) -> Result<Vec<RailItem>, RailFailure> {
        let page = NonZeroU32::new(1).expect("one is not zero");
        let candidates = match provider.rail(rail, page).await {
            Ok(candidates) => candidates,
            Err(ProviderError::Unsupported { .. }) => return Err(RailFailure::Refused),
            Err(error) => return Err(RailFailure::Unavailable(error.to_string())),
        };

        let mut items = Vec::new();
        for candidate in candidates {
            // A page is at most twenty titles and each one is a read and possibly a write, so the
            // token is checked per candidate rather than per rail: a run cancelled here stops
            // within one title rather than after a whole page.
            if cancellation.is_cancelled() {
                return Err(RailFailure::Cancelled);
            }
            match self.upsert(candidate, at).await {
                Ok(Some(item)) => items.push(item),
                // A candidate the matching rule would not attribute, or one another writer claimed
                // mid-loop: the title is not drawn in *this* rail, and `PG-E5-13` owns what a
                // person does about the first.
                Ok(None) => {}
                Err(error) => return Err(RailFailure::Unavailable(error.to_string())),
            }
        }
        Ok(items)
    }

    /// One candidate, in the catalogue, as a rail item — or `None` when it cannot be one.
    ///
    /// The write is [`peregrine_sync::record_list_answer`] rather than a details read: a list answer
    /// already carries the poster's path, the score, the synopsis, and the vote count, so the
    /// catalogue is given all four here and the artwork phase below reads the path back out. That is
    /// what turns two requests per title into one (`PG-E7-08`), and it is where the hover preview's
    /// prose comes from without a second request either (`PG-E7-05`). The item the caller draws is
    /// built from what was written, so the screen and the catalogue cannot disagree about what the
    /// provider said.
    async fn upsert(
        &self,
        candidate: SearchCandidate,
        at: i64,
    ) -> Result<Option<RailItem>, peregrine_sync::SyncError> {
        let title_id = match self
            .database
            .title_for_external_id(candidate.external_id())
            .await?
        {
            Some(id) => id,
            None => match self.match_or_create(&candidate, at).await? {
                Some(id) => id,
                None => return Ok(None),
            },
        };
        let written =
            peregrine_sync::record_list_answer(&self.database, title_id, &candidate, at).await?;
        Ok(Some(RailItem {
            title_id,
            kind: written.kind,
            name: written.name.as_str().to_owned(),
            // The date a *candidate* carries is what the provider listed it under; a title the
            // catalogue already holds and a provider dated differently keeps the candidate's,
            // because the rail's own answer is the more recent statement of the two.
            year: written.released.map(|date| i32::from(date.year())),
            poster: None,
            // **The provider that serves the backdrop is the one that listed it**, which is a fact
            // about the candidate rather than about the stored row: the path came from this
            // adapter's answer, so asking any other configured provider for it would be asking for a
            // path it never sent.
            backdrop_source: written.backdrop_path.as_ref().map(|path| ArtworkPath {
                provider: candidate.external_id().provider(),
                path: path.clone(),
            }),
            backdrop: None,
            // Filled by the artwork phase, whose library read knows the two counts; a title this run
            // has just written has no file until a scan finds one.
            playable: false,
            rating: written.rating,
            overview: written.overview,
            votes: written.votes,
        }))
    }

    /// The matching rule, and the stub it decides to write.
    async fn match_or_create(
        &self,
        candidate: &SearchCandidate,
        at: i64,
    ) -> Result<Option<i64>, StorageError> {
        let Some(name) = NonEmptyText::new(candidate.name().as_str()).ok() else {
            return Ok(None);
        };
        let input = Candidate {
            external_id: candidate.external_id().clone(),
            name,
            kind: candidate.kind(),
            released: candidate.released(),
        };
        let locals = self
            .database
            .local_titles_for_sort_name(
                candidate.kind().as_str(),
                &sort_name(candidate.name().as_str()),
            )
            .await?;

        match match_candidate(&input, &locals) {
            MatchDecision::Matched { local_id, .. } => match self
                .database
                .attach_external_ids(
                    local_id,
                    &[TitleExternalId {
                        id: candidate.external_id().clone(),
                        is_primary: true,
                        fetched_at: at,
                    }],
                )
                .await
            {
                // A concurrent writer attaching the same id to the same title is the outcome this
                // call was trying to produce, so it is the same arm and not a silent failure.
                Ok(()) | Err(StorageError::Conflict { .. }) => Ok(Some(local_id)),
                Err(error) => Err(error),
            },
            MatchDecision::New => self
                .database
                .upsert_provider_title(ProviderTitle {
                    external_id: candidate.external_id().clone(),
                    kind: candidate.kind(),
                    subtype: peregrine_domain::catalogue::TitleSubtype::Standard,
                    name: candidate.name().as_str().to_owned(),
                    sort_name: sort_name(candidate.name().as_str()),
                    is_primary: true,
                    at,
                })
                .await
                .map(Some),
            // A question, not an answer: a screen cannot draw a guess, so the candidate is left
            // out of the rail rather than merged into one of two titles.
            MatchDecision::Ambiguous { .. } => Ok(None),
        }
    }

    /// Fetches every rail title's poster, and the hero's backdrops, into the blob cache.
    ///
    /// **Every title's poster, in one run, and no budget.** The path came from the list answer the
    /// candidate already was, so filling a tile costs one request rather than two, and what paces
    /// those requests is the provider client's per-host token bucket (`AGENTS R24`) rather than a
    /// constant here. [`artwork_order`] still decides *which* poster is worth having first when the
    /// network is slow; it no longer decides how many are asked for.
    ///
    /// **The backdrops are the hero's, and they are bounded** ([`hero_backdrops`]): the wide image is
    /// four times a poster's bytes and only the rotation draws one, so a backdrop per rail item would
    /// be tens of megabytes for a strip that shows one at a time.
    ///
    /// **This is also where the library's answer about each title is read**, because the title row a
    /// poster's provider and path come from is the same read: [`RailItem::playable`] is that row's
    /// file counts, and the alternative — a second query per title — would be a second read of the
    /// same row.
    ///
    /// Returns what the cache now holds for each title, and the one library fact, keyed by title. A
    /// failure for one title is logged and does not end the run: a rail whose artwork did not arrive
    /// is a rail of reserved boxes, which is a state the screen draws, and failing here would throw
    /// away the rails that did arrive.
    async fn prefetch_artwork(
        &self,
        job: ulid::Ulid,
        attempt: u32,
        rails: &BTreeMap<RailKind, Vec<RailItem>>,
        adapters: &[Arc<dyn MetadataProvider>],
        cancellation: &CancellationToken,
    ) -> Result<BTreeMap<i64, RailArtwork>, JobError> {
        let scope = crate::runtime::installation_scope();
        let ordered = artwork_order(rails);
        let heroes = hero_backdrops(rails);
        let total = ordered.len();
        let mut artwork = BTreeMap::new();
        for (done, item) in ordered.into_iter().enumerate() {
            // Checked between fetches, so a cancelled run stops within one title rather than after
            // the whole wall. What phase one wrote is committed and published, so the catalogue the
            // caller is left with is a whole number of titles rather than a half-written one.
            if cancellation.is_cancelled() {
                return Err(JobError::Cancelled);
            }
            // Phase one owns the first half of the band and the artwork the second, and the bus
            // coalesces a burst of these to the ≤4 Hz the IPC budget allows (`ARCHITECTURE §7`
            // rule 7) — so reporting one per title costs no extra event. A missing divisor is an
            // empty set of rail items, which has no progress to report.
            if let Some(share) = done.saturating_mul(500).checked_div(total) {
                let permille = 500 + u16::try_from(share).unwrap_or(0);
                self.phase(job, attempt, permille).await;
            }
            let stored = match self.database.library_title(scope, item.title_id).await {
                Ok(row) => row,
                Err(error) => {
                    debug!(title = item.title_id, %error, "a rail title could not be looked up");
                    None
                }
            };
            let mut found = RailArtwork::default();
            if let Some(row) = &stored {
                // A blob that is known to be gone is not something to play, so what counts is a file
                // that is not missing — not the number of rows.
                found.playable = row.library.files > row.library.missing_files;
            }
            // No list answer ever gave this title a poster path, so there is nothing to fetch and a
            // details request would be asking the provider to repeat itself.
            if let Some(poster) = stored.and_then(|row| row.poster) {
                found.poster = self
                    .cache_artwork(item.title_id, &poster, adapters, ImageKind::Poster)
                    .await;
            }
            if heroes.contains(&item.title_id)
                && let Some(source) = &item.backdrop_source
            {
                found.backdrop = self
                    .cache_artwork(item.title_id, source, adapters, ImageKind::Backdrop)
                    .await;
            }
            artwork.insert(item.title_id, found);
        }
        Ok(artwork)
    }

    /// One image, in the cache or fetched into it, or `None` when it cannot be had.
    ///
    /// **The provider that serves the artwork is the one that listed it**, which the path's own row
    /// records: a second configured provider must not be asked for a path it never sent. A blob the
    /// cache already holds costs a `stat` and no request, which is what makes a second visit cheap,
    /// and a fetch that fails is a debug line rather than a failed run — a reserved box is a state
    /// the screen draws.
    async fn cache_artwork(
        &self,
        title: i64,
        source: &peregrine_storage::ArtworkPath,
        adapters: &[Arc<dyn MetadataProvider>],
        kind: ImageKind,
    ) -> Option<std::path::PathBuf> {
        if let Some(path) =
            crate::artwork::cached_image(&self.artwork, source.provider, &source.path, kind)
        {
            return Some(path);
        }
        let Some(provider) = adapters
            .iter()
            .find(|adapter| adapter.descriptor().provider() == source.provider)
        else {
            debug!(
                title,
                provider = source.provider.as_str(),
                "no configured provider serves this title's artwork"
            );
            return None;
        };
        match peregrine_sync::cache_image(provider.as_ref(), &self.artwork, &source.path, kind)
            .await
        {
            Ok(path) => Some(path),
            Err(error) => {
                debug!(title, ?kind, %error, "a rail image did not arrive");
                None
            }
        }
    }

    /// Records that the run succeeded and publishes the phase, which is the last thing every
    /// successful path does.
    async fn succeeded(
        &self,
        job: ulid::Ulid,
        attempt: u32,
        items: usize,
    ) -> peregrine_sync::JobResult {
        let at = self.clock.now().as_millisecond();
        for change in [
            JobChange::Progress { permille: 1_000 },
            JobChange::Finished {
                at,
                phase: JobPhase::Succeeded,
                error_code: None,
                error: None,
            },
        ] {
            if let Err(error) = self.database.update_job(self.journal_id, change).await {
                debug!(%error, "job completion could not be journalled");
            }
        }
        self.bus.publish(Event::new(
            JOB_STATE,
            job.to_string(),
            crate::jobs::state(job, self.journal_id, KIND, JobPhase::Succeeded, attempt),
        ));
        info!(items, "the discover rails were refreshed");
        Ok(Box::new(items) as JobOutput)
    }
}

/// What one artwork pass found for one title: where the cache put each image, and the one library
/// fact the same read answered.
///
/// A value rather than a tuple of three, because the three are read together at the publish and a
/// bare `(Option<PathBuf>, Option<PathBuf>, bool)` at a call site says nothing about which is which.
#[derive(Debug, Default)]
struct RailArtwork {
    /// The cached poster, when the catalogue records a path for one and the fetch succeeded.
    poster: Option<std::path::PathBuf>,
    /// The cached backdrop, for a title the hero can show and whose answer carried a path.
    backdrop: Option<std::path::PathBuf>,
    /// Whether the library holds a file for this title that is not known to be missing.
    playable: bool,
}

/// The titles a run fetches a backdrop for: the first rail that has items, capped at
/// [`HERO_BACKDROP_LIMIT`].
///
/// **The first rail, because that is the one a screen's hero rotates through.** `DiscoverView` picks
/// its hero from the first rail with an item and rotates through that rail's first few, so a
/// backdrop for any other rail would be a wide image nothing draws. The order is `RailKind::ALL`'s,
/// which is the order the runtime reads the rails in and the order the screen draws them.
fn hero_backdrops(rails: &BTreeMap<RailKind, Vec<RailItem>>) -> BTreeSet<i64> {
    let first = RailKind::ALL
        .into_iter()
        .find_map(|rail| rails.get(&rail).filter(|items| !items.is_empty()));
    match first {
        Some(items) => items
            .iter()
            .take(HERO_BACKDROP_LIMIT)
            .map(|item| item.title_id)
            .collect(),
        None => BTreeSet::new(),
    }
}

/// The order artwork is fetched in: interleaved by position, one entry per title.
///
/// **Interleaved, so every rail's first row is reached before any rail's third.** Walking one rail
/// to its end first would put the rail the enum lists first in front of the network while the other
/// six walls stayed grey, which is a worse first frame for the same number of requests. The order
/// is therefore a statement about *what a person sees first*, and it no longer implies a stop:
/// every entry is fetched in one run. **De-duplicated, because the list entries outnumber the
/// titles**: the same film appears in `/trending/all/week` and `/movie/popular`, and asking for its
/// poster twice spends two requests on one answer.
fn artwork_order(rails: &BTreeMap<RailKind, Vec<RailItem>>) -> Vec<RailItem> {
    let deepest = rails.values().map(Vec::len).max().unwrap_or(0);
    let mut ordered = Vec::new();
    let mut seen: BTreeSet<i64> = BTreeSet::new();
    for index in 0..deepest {
        // `RailKind::ALL` rather than the map's own order: a `BTreeMap` iterates by the enum's
        // declaration order, and the screen draws `RailKind::ALL` — so anything else would fill the
        // *last* rail the screen shows before the first one it shows.
        for rail in RailKind::ALL {
            let Some(items) = rails.get(&rail) else {
                continue;
            };
            let Some(item) = items.get(index) else {
                continue;
            };
            if seen.insert(item.title_id) {
                ordered.push(item.clone());
            }
        }
    }
    ordered
}

/// Why one rail produced nothing.
enum RailFailure {
    /// The provider does not publish this rail, which is its business and not a fault.
    Refused,
    /// The provider was asked and failed.
    Unavailable(String),
    /// The run was cancelled while the rail was being fetched.
    Cancelled,
}

/// The dedupe key for the whole set of rails, so two visits to Discover coalesce onto one fetch.
///
/// One key for the screen rather than one per rail: a person pressing Refresh twice is asking for
/// *the screen*, and seven keys would let a second press start six of the seven requests again.
#[must_use]
pub fn dedupe_key() -> String {
    KIND.to_owned()
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use peregrine_domain::catalogue::{ImageKind, TitleDetails, TitleKind};
    use peregrine_domain::id::{ExternalId, Provider};
    use peregrine_domain::text::NonEmptyText;
    use peregrine_providers::descriptor::ProviderDescriptor;
    use peregrine_providers::provider::{ProviderFuture, SearchQuery};
    use peregrine_testkit::FakeMetadataProvider;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::events::EventSink;
    use crate::testing::RecordingSink;

    /// A fake clock so nothing here reads the wall clock (`AGENTS R14`).
    fn clock() -> Arc<dyn Clock + Send + Sync> {
        Arc::new(peregrine_testkit::FakeClock::at(1_700_000_000_000))
    }

    fn candidate(id: &str, name: &str, released: &str) -> SearchCandidate {
        SearchCandidate::new(
            ExternalId::new(Provider::Tmdb, id).expect("a valid id"),
            NonEmptyText::new(name).expect("real text"),
            TitleKind::Movie,
            Some(released.parse().expect("a date")),
        )
    }

    /// A candidate as a *list* endpoint answers one, which is what the artwork phase consumes.
    fn listed(id: &str, name: &str, artwork: &str, rating: Option<f64>) -> SearchCandidate {
        candidate(id, name, "1999-03-31")
            .with_artwork_path(Some(artwork.to_owned()))
            .with_rating(rating)
    }

    /// A candidate as a list endpoint that also sends a backdrop answers one.
    fn listed_wide(
        id: &str,
        name: &str,
        artwork: &str,
        backdrop: &str,
        rating: Option<f64>,
    ) -> SearchCandidate {
        listed(id, name, artwork, rating).with_backdrop_path(Some(backdrop.to_owned()))
    }

    /// One file the library holds for a title, so the row's count is a number this test set.
    ///
    /// Every optional probe field is `None`, because the only thing the field under test reads is
    /// whether a file row exists and whether it is known to be gone.
    async fn shelve(database: &Database, title_id: i64, missing: bool) {
        let at = jiff::Timestamp::from_millisecond(1_700_000_000_000).expect("a real instant");
        let item_id = database
            .insert_media_item(peregrine_domain::MediaItem {
                id: 0,
                title_id,
                episode_id: None,
                state: peregrine_domain::MediaItemState::Available,
                origin: peregrine_domain::MediaOrigin::Scan,
                preferred_file_id: None,
                quality_label: None,
                created_at: at,
                updated_at: at,
            })
            .await
            .expect("the item is written");
        database
            .insert_media_file(peregrine_domain::MediaFile {
                id: 0,
                media_item_id: item_id,
                path: NonEmptyText::new(&format!("/library/{title_id}.mkv")).expect("a path"),
                root_id: None,
                size_bytes: 1,
                mtime_ms: at,
                added_at: at,
                missing_since: missing.then_some(at),
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
                updated_at: at,
            })
            .await
            .expect("the file is written");
    }

    /// A provider that counts the image requests it is asked for, and can cancel the run on one.
    ///
    /// The count is what makes "every rail item is fetched in one run" and "an already-cached poster
    /// causes zero requests" assertions rather than intentions: a number no test can read is a
    /// number nobody is holding the job to (`AGENTS R4`). It wraps a `dyn` provider because that is
    /// what the job takes, and it delegates every other operation so the fake's own answers are what
    /// the run sees.
    struct CountingProvider {
        inner: Arc<dyn MetadataProvider>,
        requests: Arc<AtomicUsize>,
        /// Cancel the run once this many image requests have been *started*.
        cancel_after: Option<(usize, CancellationToken)>,
    }

    impl CountingProvider {
        fn new(inner: Arc<dyn MetadataProvider>, requests: Arc<AtomicUsize>) -> Self {
            Self {
                inner,
                requests,
                cancel_after: None,
            }
        }

        /// The same, cancelling `token` once `after` image requests have been started.
        fn cancelling(
            inner: Arc<dyn MetadataProvider>,
            requests: Arc<AtomicUsize>,
            token: CancellationToken,
            after: usize,
        ) -> Self {
            Self {
                inner,
                requests,
                cancel_after: Some((after, token)),
            }
        }
    }

    impl MetadataProvider for CountingProvider {
        fn descriptor(&self) -> &ProviderDescriptor {
            self.inner.descriptor()
        }

        fn check(&self) -> ProviderFuture<'_, ()> {
            self.inner.check()
        }

        fn search(&self, query: &SearchQuery) -> ProviderFuture<'_, Vec<SearchCandidate>> {
            self.inner.search(query)
        }

        fn title(&self, id: &ExternalId, kind: TitleKind) -> ProviderFuture<'_, TitleDetails> {
            self.inner.title(id, kind)
        }

        fn rail(
            &self,
            rail: RailKind,
            page: NonZeroU32,
        ) -> ProviderFuture<'_, Vec<SearchCandidate>> {
            self.inner.rail(rail, page)
        }

        fn image(&self, path: &str, kind: ImageKind) -> ProviderFuture<'_, Vec<u8>> {
            let started = self.requests.fetch_add(1, Ordering::SeqCst) + 1;
            if let Some((after, token)) = &self.cancel_after
                && started >= *after
            {
                token.cancel();
            }
            self.inner.image(path, kind)
        }
    }

    fn details(name: &str, poster: Option<&str>) -> TitleDetails {
        TitleDetails {
            kind: TitleKind::Movie,
            name: NonEmptyText::new(name).expect("real text"),
            original_name: None,
            overview: None,
            tagline: None,
            released: None,
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
            poster_path: poster.map(str::to_owned),
            backdrop_path: None,
        }
    }

    #[test]
    fn a_rail_that_has_never_been_fetched_reads_as_empty_rather_than_missing() {
        // The geometry rule, at the layer that decides it: a screen asks for the rails and draws a
        // heading and an empty state for each one, from the first frame of a cold start.
        let rails = HomeRails::new();

        let read = rails.read();

        assert_eq!(read.len(), RailKind::ALL.len());
        assert!(read.iter().all(|(_, items)| items.is_empty()));
        assert_eq!(
            read.iter().map(|(rail, _)| *rail).collect::<Vec<_>>(),
            RailKind::ALL.to_vec(),
            "the read order is the order a home screen shows them"
        );
    }

    #[test]
    fn publishing_replaces_the_whole_read_model() {
        let rails = HomeRails::new();
        let mut published = BTreeMap::new();
        published.insert(
            RailKind::PopularMovies,
            vec![RailItem {
                title_id: 7,
                kind: TitleKind::Movie,
                name: "The Matrix".to_owned(),
                year: Some(1999),
                poster: None,
                backdrop_source: None,
                backdrop: None,
                playable: false,
                rating: None,
                overview: None,
                votes: None,
            }],
        );

        rails.publish(&published);

        let read = rails.read();
        let popular = read
            .iter()
            .find(|(rail, _)| *rail == RailKind::PopularMovies)
            .expect("every rail is reported");
        assert_eq!(popular.1.len(), 1);
        assert_eq!(popular.1[0].name, "The Matrix");
        assert_eq!(popular.1[0].year, Some(1999));
        assert_eq!(
            read.iter()
                .find(|(rail, _)| *rail == RailKind::TopRatedShows)
                .expect("every rail is reported")
                .1
                .len(),
            0,
            "a rail nobody published is empty rather than absent"
        );
    }

    #[test]
    fn a_candidate_date_becomes_the_year_the_screen_draws() {
        assert_eq!(
            candidate("603", "The Matrix", "1999-03-31")
                .released()
                .map(|date| i32::from(date.year())),
            Some(1999)
        );
    }

    #[test]
    fn the_dedupe_key_names_the_screen_and_not_a_rail() {
        // One key for the screen: two presses of Refresh must wait on one fetch rather than on one
        // fetch per rail.
        assert_eq!(dedupe_key(), "discover.rails");
    }

    #[test]
    fn artwork_is_ordered_across_rails_and_counts_each_title_once() {
        // The order decides what a person sees *first*: the whole set is fetched in one run now, so
        // this is about which poster is worth having while the network catches up, and the
        // de-duplication decides how many requests the run spends — one title in two rails is one
        // poster.
        let item = |title_id: i64| RailItem {
            title_id,
            kind: TitleKind::Movie,
            name: format!("Title {title_id}"),
            year: None,
            poster: None,
            backdrop_source: None,
            backdrop: None,
            playable: false,
            rating: None,
            overview: None,
            votes: None,
        };
        let mut rails = BTreeMap::new();
        rails.insert(RailKind::TrendingWeek, vec![item(1), item(2), item(3)]);
        rails.insert(RailKind::PopularMovies, vec![item(2), item(4)]);

        let ordered: Vec<i64> = artwork_order(&rails)
            .into_iter()
            .map(|item| item.title_id)
            .collect();

        // Interleaved by position and walked in the order the screen draws the rails — trending
        // before popular — so both rails' first entries come before either rail's second. Title 2,
        // which is trending's second entry and popular's first, appears once.
        assert_eq!(ordered, vec![1, 2, 4, 3]);
    }

    #[test]
    fn the_backdrops_a_run_fetches_are_the_first_rails_and_no_more_than_it_can_show() {
        // The bound, as arithmetic: a backdrop is four times a poster's bytes and only the hero's
        // rotation draws one, so the set is the first rail that has items — the rail the screen's
        // hero reads (`heroItem`) — capped. A rail the enum lists earlier but which the provider left
        // empty does not own the hero, which is the case a "always the first rail" rule would get
        // wrong.
        let item = |title_id: i64| RailItem {
            title_id,
            kind: TitleKind::Movie,
            name: format!("Title {title_id}"),
            year: None,
            poster: None,
            backdrop_source: None,
            backdrop: None,
            playable: false,
            rating: None,
            overview: None,
            votes: None,
        };
        let mut rails = BTreeMap::new();
        rails.insert(RailKind::PopularMovies, (1..=20).map(item).collect());

        let heroes = hero_backdrops(&rails);
        assert_eq!(heroes.len(), HERO_BACKDROP_LIMIT);
        assert!(heroes.contains(&1), "the first item is the hero's lead");
        assert!(
            !heroes.contains(&20),
            "the cap is a cap, not a hope that the wall is short"
        );

        // An empty first rail does not hold the hero back: the screen's own rule takes the first
        // *non-empty* rail, and so does this.
        let mut with_empty_first = rails.clone();
        with_empty_first.insert(RailKind::TrendingWeek, Vec::new());
        assert_eq!(hero_backdrops(&with_empty_first), heroes);

        assert!(hero_backdrops(&BTreeMap::new()).is_empty());
    }

    /// One run of the job over a scripted provider, against a real database and a real cache.
    struct Fixture {
        directory: TempDir,
        database: Arc<Database>,
        bus: Arc<EventBus>,
        rails: Arc<HomeRails>,
        artwork: Arc<peregrine_sync::ImageCache>,
        journal_id: i64,
    }

    impl Fixture {
        async fn new() -> Self {
            let directory = TempDir::new().expect("a temporary directory");
            let database = Arc::new(
                Database::open(directory.path().join("peregrine.sqlite"))
                    .await
                    .expect("the database opens"),
            );
            let sink = Arc::new(RecordingSink::default());
            let bus = Arc::new(EventBus::new(Arc::clone(&sink) as Arc<dyn EventSink>));
            let journal_id = database
                .insert_job(peregrine_storage::NewJob {
                    kind: KIND.to_owned(),
                    priority: PRIORITY,
                    dedupe_key: Some(dedupe_key()),
                    payload_json: "{}".to_owned(),
                    max_attempts: 2,
                    created_at: 1_700_000_000_000,
                })
                .await
                .expect("the journal row is written");
            Self {
                database,
                bus,
                rails: Arc::new(HomeRails::new()),
                artwork: Arc::new(peregrine_sync::ImageCache::new(
                    directory.path().join("images"),
                )),
                journal_id,
                directory,
            }
        }

        /// Runs one attempt over `provider`, which is the job's body without the registry the
        /// crate's tests may not build (`AGENTS R14`: a registry reads a keyring).
        async fn run(&self, provider: Arc<dyn MetadataProvider>) -> peregrine_sync::JobResult {
            self.run_cancellable(provider, CancellationToken::new())
                .await
        }

        /// The same, with a token the caller holds — which is how a run is cancelled from inside a
        /// provider's own image call rather than from a sleep (`AGENTS R14`).
        async fn run_cancellable(
            &self,
            provider: Arc<dyn MetadataProvider>,
            cancellation: CancellationToken,
        ) -> peregrine_sync::JobResult {
            let mut job = DiscoverRailsJob::new(
                Arc::clone(&self.database),
                Arc::clone(&self.bus),
                clock(),
                Arc::new(ProviderRegistry::new()),
                Arc::clone(&self.artwork),
                Arc::clone(&self.rails),
                self.journal_id,
            );
            job.run(
                ulid::Ulid::from_parts(1_700_000_000_000, 1),
                1,
                &[provider],
                cancellation,
            )
            .await
        }
    }

    #[tokio::test]
    async fn a_run_fills_the_rails_the_catalogue_and_the_artwork_cache() {
        // The whole path of one run: a rail answer becomes catalogue rows, the read model answers
        // with them, and the title's poster is on disk so the screen has something to draw. **One
        // request per title**, which is what this pins: the candidate already carried the path, so
        // nothing asked the provider for the title's details.
        let fixture = Fixture::new().await;
        let fake = FakeMetadataProvider::publishing_rails_and_serving_images(
            Provider::Tmdb,
            details("The Matrix", None),
            b"jpeg".to_vec(),
            vec![listed("603", "The Matrix", "/poster.jpg", Some(8.2))],
        );

        fixture.run(Arc::new(fake)).await.expect("the run succeeds");

        let read = fixture.rails.read();
        let popular = read
            .iter()
            .find(|(rail, _)| *rail == RailKind::PopularMovies)
            .expect("every rail is reported");
        assert_eq!(
            popular.1.len(),
            1,
            "the rail holds what the provider listed"
        );
        assert_eq!(popular.1[0].name, "The Matrix");
        assert_eq!(popular.1[0].year, Some(1999));
        assert_eq!(
            popular.1[0].rating,
            Some(8.2),
            "the list answer's score reaches the card"
        );
        let poster = popular.1[0]
            .poster
            .as_ref()
            .expect("the poster was fetched and the path published");
        assert!(
            poster.is_file(),
            "the published path must be a blob the webview can be served"
        );
        assert_eq!(
            std::fs::read(poster).expect("the blob is readable"),
            b"jpeg"
        );

        // And it is in the catalogue, which is what opening it needs — with the artwork path a
        // later run reads the blob's address from.
        let titles = fixture
            .database
            .list_titles(
                peregrine_domain::AccountScope::unrestricted(
                    peregrine_domain::AccountId::new(1).expect("account one"),
                ),
                peregrine_storage::PageRequest::first(10),
            )
            .await
            .expect("the catalogue is readable");
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].id, popular.1[0].title_id);
        let stored = fixture
            .database
            .library_title(
                peregrine_domain::AccountScope::unrestricted(
                    peregrine_domain::AccountId::new(1).expect("account one"),
                ),
                titles[0].id,
            )
            .await
            .expect("the title is readable")
            .expect("the title exists");
        assert_eq!(
            stored.poster.as_ref().map(|artwork| artwork.path.as_str()),
            Some("/poster.jpg"),
            "the list answer's path is what the catalogue keeps"
        );

        // The row reached a terminal phase, which is what a screen following the job waits for.
        let row = fixture
            .database
            .job(fixture.journal_id)
            .await
            .expect("the journal is readable")
            .expect("the row exists");
        assert!(
            row.state.is_terminal(),
            "a finished run must close its journal row"
        );
    }

    #[tokio::test]
    async fn a_rail_item_carries_the_synopsis_and_the_vote_count_the_list_answer_gave() {
        // The hover preview draws both inside the enlarged card (`PG-E7-05`), and the rail item is
        // where it gets them: a candidate whose list answer carried prose and a count keeps them,
        // and one whose answer carried neither says `None` rather than inventing a summary or a
        // zero. The write is asserted separately, against `peregrine-sync`'s own suite.
        let fixture = Fixture::new().await;
        let described = listed("603", "The Matrix", "/poster.jpg", Some(8.2))
            .with_overview(Some("A hacker learns the truth.".to_owned()))
            .with_votes(Some(24_000));
        let bare = listed("604", "A Bare Listing", "/bare.jpg", None);
        let fake = FakeMetadataProvider::publishing_rails_and_serving_images(
            Provider::Tmdb,
            details("unused", None),
            b"jpeg".to_vec(),
            vec![described, bare],
        );
        fixture.run(Arc::new(fake)).await.expect("the run succeeds");

        let read = fixture.rails.read();
        let popular = read
            .iter()
            .find(|(rail, _)| *rail == RailKind::PopularMovies)
            .expect("every rail is reported");
        assert_eq!(popular.1.len(), 2);
        let matrix = popular
            .1
            .iter()
            .find(|item| item.name == "The Matrix")
            .expect("the described title is listed");
        assert_eq!(
            matrix.overview.as_deref(),
            Some("A hacker learns the truth.")
        );
        assert_eq!(matrix.votes, Some(24_000));
        let bare = popular
            .1
            .iter()
            .find(|item| item.name == "A Bare Listing")
            .expect("the bare title is listed");
        assert_eq!(
            bare.overview, None,
            "a provider that wrote no prose said none"
        );
        assert_eq!(bare.votes, None, "an absent count is not a count of zero");
    }

    #[tokio::test]
    async fn one_run_fetches_every_rail_item_rather_than_a_budget_of_them() {
        // The defect this pins: a fixed budget left every title past the twenty-fourth as a grey
        // placeholder forever. Thirty titles, every one of them in the rail the fake answers, and
        // thirty image requests — a number asserted rather than a constant compared, so raising or
        // reinstating a cap fails here.
        let fixture = Fixture::new().await;
        let candidates: Vec<SearchCandidate> = (0..30)
            .map(|index| {
                listed(
                    &index.to_string(),
                    &format!("Title {index}"),
                    &format!("/poster-{index}.jpg"),
                    None,
                )
            })
            .collect();
        let fake = Arc::new(FakeMetadataProvider::publishing_rails_and_serving_images(
            Provider::Tmdb,
            details("unused", None),
            b"jpeg".to_vec(),
            candidates,
        ));
        let requests = Arc::new(AtomicUsize::new(0));

        fixture
            .run(Arc::new(CountingProvider::new(fake, Arc::clone(&requests))))
            .await
            .expect("the run succeeds");

        assert_eq!(
            requests.load(Ordering::SeqCst),
            30,
            "one request per title, whatever a budget used to say"
        );
        let read = fixture.rails.read();
        let popular = read
            .iter()
            .find(|(rail, _)| *rail == RailKind::PopularMovies)
            .expect("every rail is reported");
        assert_eq!(popular.1.len(), 30);
        assert!(
            popular
                .1
                .iter()
                .all(|item| item.poster.as_ref().is_some_and(|path| path.is_file())),
            "every tile is a poster rather than a reserved box"
        );
    }

    #[tokio::test]
    async fn a_poster_the_cache_already_holds_costs_no_request() {
        // The second visit's whole economy: the catalogue records the path, the blob is on disk, and
        // the run asks the provider for nothing. Without the cache check this is one request per
        // title on every visit to the screen.
        let fixture = Fixture::new().await;
        let fake = Arc::new(FakeMetadataProvider::publishing_rails_and_serving_images(
            Provider::Tmdb,
            details("The Matrix", None),
            b"jpeg".to_vec(),
            vec![listed("603", "The Matrix", "/poster.jpg", Some(8.2))],
        ));
        let requests = Arc::new(AtomicUsize::new(0));
        let provider: Arc<dyn MetadataProvider> =
            Arc::new(CountingProvider::new(fake, Arc::clone(&requests)));

        fixture
            .run(Arc::clone(&provider))
            .await
            .expect("the first run");
        assert_eq!(requests.load(Ordering::SeqCst), 1, "one blob to fetch");
        fixture
            .run(Arc::clone(&provider))
            .await
            .expect("the second run");

        assert_eq!(
            requests.load(Ordering::SeqCst),
            1,
            "a poster the cache holds must not be fetched twice"
        );
    }

    #[tokio::test]
    async fn a_run_fetches_the_heros_backdrop_alongside_its_poster() {
        // The wide image the hero draws (`PG-E7-08`): the list answer already carried the path, so
        // filling it costs one more request rather than a details read — and the path that reaches
        // the cache is the *backdrop* variant, which the cache keys separately from the poster's, so
        // the two cannot be served for each other.
        let fixture = Fixture::new().await;
        let fake = FakeMetadataProvider::publishing_rails_and_serving_images(
            Provider::Tmdb,
            details("unused", None),
            b"jpeg".to_vec(),
            vec![listed_wide(
                "603",
                "The Matrix",
                "/poster.jpg",
                "/wide.jpg",
                Some(8.2),
            )],
        );
        let requests = Arc::new(AtomicUsize::new(0));

        fixture
            .run(Arc::new(CountingProvider::new(
                Arc::new(fake),
                Arc::clone(&requests),
            )))
            .await
            .expect("the run succeeds");

        assert_eq!(
            requests.load(Ordering::SeqCst),
            2,
            "one request for the poster and one for the wide image, and no third for the wall"
        );
        let read = fixture.rails.read();
        let trending = read
            .iter()
            .find(|(rail, _)| *rail == RailKind::TrendingWeek)
            .expect("every rail is reported");
        let item = &trending.1[0];
        let backdrop = item
            .backdrop
            .as_ref()
            .expect("the hero's rail carries the wide image it named");
        assert!(
            backdrop.is_file(),
            "the published path must be a blob the webview can be served"
        );
        assert_eq!(
            std::fs::read(backdrop).expect("the blob is readable"),
            b"jpeg"
        );
        assert!(
            item.poster.as_ref().is_some_and(|path| path.is_file()),
            "the poster is still fetched for the tile"
        );
        assert_ne!(
            item.poster.as_ref(),
            item.backdrop.as_ref(),
            "the two variants are different blobs and cannot stand in for each other"
        );
    }

    #[tokio::test]
    async fn a_rail_item_carries_the_library_s_answer_about_whether_there_is_a_file() {
        // The fact the hero's Play is enabled from, and the reason it is a core answer rather than a
        // screen's guess (`AGENTS R7`): a title the catalogue merely lists is not something to play.
        // A file that is *known to be gone* is not either, which is the arm a `files > 0` rule would
        // get wrong.
        let fixture = Fixture::new().await;
        let candidates = vec![
            listed("603", "The Matrix", "/matrix.jpg", None),
            listed("604", "Inception", "/inception.jpg", None),
            listed("605", "Interstellar", "/interstellar.jpg", None),
        ];
        let fake = FakeMetadataProvider::publishing_rails_and_serving_images(
            Provider::Tmdb,
            details("unused", None),
            b"jpeg".to_vec(),
            candidates,
        );

        // One title the run will find before it writes anything, which is the order a second visit
        // has: the rails are written, and the file was shelved by an earlier scan.
        fixture
            .run(Arc::new(fake))
            .await
            .expect("the first run writes the titles");
        let read = fixture.rails.read();
        let popular = read
            .iter()
            .find(|(rail, _)| *rail == RailKind::PopularMovies)
            .expect("every rail is reported")
            .1
            .clone();
        let matrix = popular
            .iter()
            .find(|item| item.name == "The Matrix")
            .expect("the title is listed")
            .title_id;
        shelve(&fixture.database, matrix, false).await;
        let inception = popular
            .iter()
            .find(|item| item.name == "Inception")
            .expect("the title is listed")
            .title_id;
        shelve(&fixture.database, inception, true).await;

        // The second run, over a provider answering the same list — the shape a Refresh has.
        let fake = FakeMetadataProvider::publishing_rails_and_serving_images(
            Provider::Tmdb,
            details("unused", None),
            b"jpeg".to_vec(),
            vec![
                listed("603", "The Matrix", "/matrix.jpg", None),
                listed("604", "Inception", "/inception.jpg", None),
                listed("605", "Interstellar", "/interstellar.jpg", None),
            ],
        );
        fixture.run(Arc::new(fake)).await.expect("the run succeeds");

        let read = fixture.rails.read();
        let popular = read
            .iter()
            .find(|(rail, _)| *rail == RailKind::PopularMovies)
            .expect("every rail is reported");
        let playable = |name: &str| {
            popular
                .1
                .iter()
                .find(|item| item.name == name)
                .unwrap_or_else(|| panic!("{name} is listed"))
                .playable
        };

        assert!(
            playable("The Matrix"),
            "a file on disk is something to play"
        );
        assert!(
            !playable("Inception"),
            "a file the library knows is gone is not"
        );
        assert!(
            !playable("Interstellar"),
            "a title nobody has shelved is not, and that is not the same as an error"
        );
    }

    #[tokio::test]
    async fn cancelling_a_run_mid_artwork_leaves_it_cancelled_and_the_catalogue_whole() {
        // Cancellation is checked between fetches, so a cancelled run stops within one title. What
        // it must leave behind is *consistent* rather than empty: every title phase one wrote is in
        // the catalogue with its artwork path, and the read model still holds the items it published
        // — so the next run picks the posters up rather than starting the screen over.
        let fixture = Fixture::new().await;
        let token = CancellationToken::new();
        let candidates = vec![
            listed("603", "The Matrix", "/one.jpg", None),
            listed("27205", "Inception", "/two.jpg", None),
            listed("157336", "Interstellar", "/three.jpg", None),
        ];
        let fake = Arc::new(FakeMetadataProvider::publishing_rails_and_serving_images(
            Provider::Tmdb,
            details("unused", None),
            b"jpeg".to_vec(),
            candidates,
        ));
        let requests = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(CountingProvider::cancelling(
            fake,
            Arc::clone(&requests),
            token.clone(),
            1,
        ));

        let outcome = fixture.run_cancellable(provider, token).await;

        assert!(
            matches!(outcome, Err(JobError::Cancelled)),
            "a cancelled token must come back as a cancellation"
        );
        assert_eq!(
            requests.load(Ordering::SeqCst),
            1,
            "the fetch after the cancellation is never started"
        );

        let scope = peregrine_domain::AccountScope::unrestricted(
            peregrine_domain::AccountId::new(1).expect("account one"),
        );
        assert_eq!(
            fixture
                .database
                .title_count()
                .await
                .expect("the count is readable"),
            3,
            "phase one wrote every title before any artwork was attempted"
        );
        let titles = fixture
            .database
            .list_titles(scope, peregrine_storage::PageRequest::first(10))
            .await
            .expect("the catalogue is readable");
        for title in &titles {
            let stored = fixture
                .database
                .library_title(scope, title.id)
                .await
                .expect("the title is readable")
                .expect("the title exists");
            assert!(
                stored.poster.is_some(),
                "every written title keeps the path the list answer gave it"
            );
        }
        let read = fixture.rails.read();
        assert!(
            read.iter()
                .all(|(_, items)| items.iter().all(|item| item.poster.is_none())),
            "the artwork phase was cancelled before it could publish, so the items keep their \
             reserved geometry rather than a half-held set of posters"
        );
    }

    #[tokio::test]
    async fn a_provider_that_publishes_nothing_leaves_every_rail_empty_and_still_succeeds() {
        // The cold-catalogue case: a provider with a trending capability and nothing in its lists.
        // Every rail reads empty rather than absent, and the run is a success — there is nothing
        // here to fail, and a red journal row would send someone looking for a fault.
        let fixture = Fixture::new().await;
        let fake = FakeMetadataProvider::publishing_rails(Provider::Tmdb, Vec::new());

        fixture.run(Arc::new(fake)).await.expect("the run succeeds");

        let read = fixture.rails.read();
        assert_eq!(read.len(), RailKind::ALL.len());
        assert!(read.iter().all(|(_, items)| items.is_empty()));
        // Nothing was fetched, so the cache directory holds no blob: the empty rail costs no
        // request and writes nothing.
        let blobs = std::fs::read_dir(fixture.directory.path().join("images"))
            .map_or(0, std::iter::Iterator::count);
        assert_eq!(blobs, 0, "an empty rail fetches no artwork");
    }

    #[tokio::test]
    async fn a_provider_that_cannot_trend_leaves_every_rail_empty_and_still_succeeds() {
        // A provider that declares only `search` refuses every rail. That is its descriptor being
        // honest rather than a fault — rails are a capability not every provider has — so the run
        // succeeds and every rail reads empty, which is the state the screen has a designed empty
        // state for. A red journal row here would send someone looking for a fault that is not
        // there.
        let fixture = Fixture::new().await;
        let fake = FakeMetadataProvider::searching(Provider::Tmdb, Vec::new());

        fixture.run(Arc::new(fake)).await.expect("the run succeeds");

        assert_eq!(fixture.rails.read().len(), RailKind::ALL.len());
        assert!(
            fixture
                .rails
                .read()
                .iter()
                .all(|(_, items)| items.is_empty())
        );
    }

    #[tokio::test]
    async fn a_run_where_every_rail_failed_reports_why() {
        // The other half: a provider that *claims* the capability and then fails every request is
        // a provider having a bad day, and a screen that pressed Refresh deserves the reason
        // rather than seven silently empty rails.
        let fixture = Fixture::new().await;
        let fake = FakeMetadataProvider::publishing_rails(Provider::Tmdb, Vec::new())
            .failing_rails(ProviderError::Transport {
                provider: Provider::Tmdb,
                kind: peregrine_providers::TransportFailure::Timeout,
            });

        let outcome = fixture.run(Arc::new(fake)).await;

        assert!(outcome.is_err(), "every rail failed, so the run did too");
        assert!(
            fixture
                .rails
                .read()
                .iter()
                .all(|(_, items)| items.is_empty())
        );
    }
}
