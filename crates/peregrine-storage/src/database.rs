//! The database handle: one writer task, a reader pool, and the open path.

use std::path::Path;

use sqlx::ConnectOptions;
use sqlx::Connection as _;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection, SqlitePool, SqlitePoolOptions};
use tracing::{info, warn};

use peregrine_domain::{
    AccountScope, DownloadEvent, DownloadState, LibraryRoot, MediaFile, MediaItem, ScanMode,
    SubtitleTrack, TorrentDescription, UnmatchedFile, WatchProgress, WatchlistEntry,
};

use crate::catalogue::{StubTitle, TitleExternalId, TitleIdentity};
use crate::download::{
    DownloadFileRow, DownloadJobRow, DownloadStatistics, DownloadTransitionError, NewDownloadJob,
};
use crate::error::StorageError;
use crate::integrity::Recovery;
use crate::jobs::{JobChange, JobRecord, NewJob};
use crate::maintenance::{MaintenanceOutcome, MaintenancePlan, MaintenanceRequest};
use crate::page::PageRequest;
use crate::pragmas;
use crate::retention::{RetentionOutcome, RetentionPlan};
use crate::usage::{DiskUsage, LibraryUsageRow};
use crate::writer::{NewEvent, WriteRequest, WriteResponse, WriterHandle};

/// ARCHITECTURE §10: readers use a pool sized to `min(4, cores)`.
const READER_POOL_MAX: u32 = 4;

/// A handle to the one `SQLite` database.
///
/// Writes are applied by a task that owns the only write connection; reads are served
/// by a small pool, because WAL lets readers run while a writer holds the write lock.
#[derive(Debug)]
pub struct Database {
    writer: WriterHandle,
    readers: SqlitePool,
    recovery: Recovery,
}

impl Database {
    /// Opens the database at `path`, creating it when it does not exist, checks the file
    /// for damage, applies the embedded migrations on the write connection, and starts the
    /// writer task.
    ///
    /// The order is the recovery path of ARCHITECTURE §15: a damaged file is moved aside and
    /// replaced with a clean schema before the writer opens it, and an upgrade copies an
    /// existing database before it applies the first pending migration.
    ///
    /// # Errors
    ///
    /// [`StorageError::Pragma`] means the write connection could not be brought to the
    /// documented PRAGMA set, so the database was left untouched and the caller should
    /// report the pragma rather than retry blindly. [`StorageError::MigrationFailed`] and
    /// [`StorageError::Backup`] mean the upgrade did not run; [`StorageError::Quarantine`]
    /// means a damaged file is still in the way and nothing was deleted.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let path = path.as_ref();
        // The path is the first thing a support question needs and is not a secret; this
        // crate never logs a setting or a credential.
        info!(path = %path.display(), "opening the database");

        let recovery = recover(path).await?;
        let mut connection = open_writer_connection(path).await?;
        crate::migrations::apply(&mut connection, path).await?;
        // No job survives the process that was running it, and a row left non-terminal holds its
        // dedupe key against every later submission (`crate::jobs::fail_unfinished`). Done before the
        // writer task starts, so the first write of this process already sees a settled journal.
        let interrupted = crate::jobs::fail_unfinished(&mut connection)
            .await
            .map_err(StorageError::from_sqlx)?;
        if interrupted > 0 {
            warn!(
                jobs = interrupted,
                "closed out the jobs a previous process left unfinished"
            );
        }

        let writer = WriterHandle::spawn(connection);
        let readers = open_reader_pool(path)
            .await
            .map_err(|error| StorageError::open(path, error))?;

        Ok(Self {
            writer,
            readers,
            recovery,
        })
    }

    /// What the boot-time integrity check found.
    ///
    /// [`Recovery::Rebuilt`] means the rows that were in the file are not in the database
    /// any more, so the library has to be rebuilt by scanning (ARCHITECTURE §15).
    #[must_use]
    pub fn recovery(&self) -> &Recovery {
        &self.recovery
    }

    /// The single-writer handle, for a repository that lives in its own module.
    ///
    /// A repository needs the writer and a reader, not the handle's whole surface; handing
    /// it the two ports is what keeps the writer the only write path (`ARCHITECTURE` §5).
    pub(crate) const fn writer(&self) -> &WriterHandle {
        &self.writer
    }

    /// The reader pool, for a repository that lives in its own module.
    pub(crate) const fn readers(&self) -> &SqlitePool {
        &self.readers
    }

    /// Appends one row to `event_log` through the single writer.
    ///
    /// Returns the rowid `SQLite` assigned: only the writer task ever holds the write
    /// connection, so it is the only place that can observe it.
    pub async fn append_event(&self, event: NewEvent) -> Result<i64, StorageError> {
        match self.writer.send(WriteRequest::AppendEvent(event)).await? {
            WriteResponse::EventAppended { id } => Ok(id),
            other => Err(unexpected_response("append_event", &other)),
        }
    }

    /// Adds one job to the journal through the single writer.
    ///
    /// The row starts in `queued`, which is the phase the runtime reports before the
    /// scheduler has an attempt to run.
    pub async fn insert_job(&self, job: NewJob) -> Result<i64, StorageError> {
        match self
            .writer
            .send(WriteRequest::InsertJob(Box::new(job)))
            .await?
        {
            WriteResponse::JobInserted { id } => Ok(id),
            other => Err(unexpected_response("insert_job", &other)),
        }
    }

    /// Applies one change to a journal row, reporting whether the row was still there.
    ///
    /// `false` is not an error: a job that finished while a late progress report was in
    /// flight is the ordinary case, and the guards in `jobs::update` are what stop that
    /// report from resurrecting a terminal row.
    pub async fn update_job(&self, id: i64, change: JobChange) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::UpdateJob {
                id,
                change: Box::new(change),
            })
            .await?
        {
            WriteResponse::JobUpdated { changed } => Ok(changed),
            other => Err(unexpected_response("update_job", &other)),
        }
    }

    /// One journal row, or `None` when no such row exists.
    pub async fn job(&self, id: i64) -> Result<Option<JobRecord>, StorageError> {
        crate::jobs::get(&self.readers, id).await
    }

    /// The profile new work uses when the caller names none, or None when none is marked.
    ///
    /// # Errors
    ///
    /// StorageError::InvalidJsonField or StorageError::UnexpectedValue when the stored row is
    /// not a profile this build wrote.
    pub async fn default_quality_profile(
        &self,
    ) -> Result<Option<peregrine_domain::QualityProfile>, StorageError> {
        crate::quality::default_profile(&self.readers).await
    }

    /// Rewrites the built-in default profile's resolution list.
    ///
    /// Answers whether a default profile was there to write. The caller turns a preference into
    /// the list; this only persists it.
    pub async fn set_default_quality_resolutions(
        &self,
        resolutions_json: &str,
        at: i64,
    ) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::SetDefaultQualityResolutions {
                resolutions_json: resolutions_json.to_owned(),
                at,
            })
            .await?
        {
            WriteResponse::Applied => Ok(true),
            WriteResponse::RowIgnored => Ok(false),
            other => Err(unexpected_response(
                "set_default_quality_resolutions",
                &other,
            )),
        }
    }

    /// The profile new work uses, seeding the built-in default the first time it is needed.
    ///
    /// A fresh installation has no profile, and ranking cannot run without one — the acquire job
    /// failed at the profile step on exactly that. The insert is conditional, so a second caller or
    /// a second process never adds a second default.
    pub async fn ensure_default_quality_profile(
        &self,
        at: i64,
    ) -> Result<peregrine_domain::QualityProfile, StorageError> {
        if let Some(profile) = self.default_quality_profile().await? {
            return Ok(profile);
        }
        let profile = peregrine_domain::QualityProfile::new(
            "Default",
            crate::quality::default_rules(),
            peregrine_domain::QualityRank::new(u16::MAX),
            true,
            true,
        )
        .map_err(|error| StorageError::UnexpectedValue {
            column: "quality_profile",
            found: error.to_string(),
        })?;
        let rules_json = serde_json::to_string(profile.rules()).map_err(|error| {
            StorageError::InvalidJsonField {
                column: "rules_json",
                detail: error.to_string(),
            }
        })?;
        match self
            .writer
            .send(WriteRequest::InsertDefaultQualityProfile {
                name: profile.name().to_owned(),
                rules_json,
                cutoff_rank: i64::from(profile.cutoff_rank().position()),
                upgrade_allowed: profile.upgrade_allowed(),
                at,
            })
            .await?
        {
            WriteResponse::Applied | WriteResponse::RowIgnored => Ok(profile),
            other => Err(unexpected_response(
                "ensure_default_quality_profile",
                &other,
            )),
        }
    }

    /// A page of journal rows, newest first.
    pub async fn list_jobs(&self, request: PageRequest) -> Result<Vec<JobRecord>, StorageError> {
        crate::jobs::list(&self.readers, request).await
    }

    /// Creates one `download_job` row in `queued` and returns the row the database holds.
    ///
    /// `queued` is a literal in the statement: it is the one state `FLOWS` F6 admits a job in,
    /// and no caller may name a state. Every later move goes through
    /// [`Database::transition_download_job`].
    ///
    /// # Errors
    ///
    /// [`StorageError::Conflict`] when a live job already holds this torrent's info hash (I36,
    /// `download_job_hash_uq`).
    pub async fn create_download_job(
        &self,
        job: NewDownloadJob,
    ) -> Result<DownloadJobRow, StorageError> {
        match self
            .writer
            .send(WriteRequest::CreateDownloadJob(Box::new(job)))
            .await?
        {
            WriteResponse::DownloadJobCreated(row) => Ok(*row),
            other => Err(unexpected_response("create_download_job", &other)),
        }
    }

    /// Applies one event to a job's state, writing the safety report the event carries.
    ///
    /// `Ok(None)` means no job holds that id. A refusal is the domain's answer and not a storage
    /// failure, and nothing is written for one.
    pub async fn transition_download_job(
        &self,
        id: i64,
        event: DownloadEvent,
    ) -> Result<Option<DownloadState>, DownloadTransitionError> {
        let response = self
            .writer
            .send(WriteRequest::TransitionDownloadJob {
                id,
                event: Box::new(event),
            })
            .await?;
        match response {
            WriteResponse::DownloadJobTransitioned { state } => Ok(state),
            WriteResponse::DownloadJobRefused { rejection } => Err(rejection.into()),
            other => Err(unexpected_response("transition_download_job", &other).into()),
        }
    }

    /// Deletes one finished queue entry, answering whether it went.
    ///
    /// Only `failed` and `removed` are deletable; the statement scopes the state in SQL. Files
    /// already written are untouched: this removes the record, not the bytes.
    pub async fn delete_download_job(&self, id: i64) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::DeleteDownloadJob { id })
            .await?
        {
            WriteResponse::DownloadJobDeleted { removed } => Ok(removed),
            other => Err(unexpected_response("delete_download_job", &other)),
        }
    }

    /// Deletes every finished queue entry, answering how many went.
    pub async fn clear_finished_download_jobs(&self) -> Result<u64, StorageError> {
        match self
            .writer
            .send(WriteRequest::DeleteFinishedDownloadJobs)
            .await?
        {
            WriteResponse::DownloadJobsCleared { removed } => Ok(removed),
            other => Err(unexpected_response("clear_finished_download_jobs", &other)),
        }
    }

    /// Replaces the offers a search found for one title, answering how many were stored.
    ///
    /// Anything the user already acted on is kept, so a dismissed offer does not come back.
    ///
    /// # Errors
    ///
    /// StorageError when the statement fails.
    pub async fn replace_release_offers(
        &self,
        title_id: i64,
        offers: &[crate::offers::NewOffer],
        at: i64,
    ) -> Result<u64, StorageError> {
        match self
            .writer
            .send(WriteRequest::ReplaceReleaseOffers {
                title_id,
                offers: offers.to_vec(),
                at,
            })
            .await?
        {
            WriteResponse::ReleaseOffersStored { written } => Ok(written),
            other => Err(unexpected_response("replace_release_offers", &other)),
        }
    }

    /// Marks one offer chosen and answers its source, which never crosses IPC.
    ///
    /// # Errors
    ///
    /// StorageError when the statement fails.
    pub async fn choose_release_offer(
        &self,
        id: i64,
    ) -> Result<Option<crate::offers::ChosenOfferRow>, StorageError> {
        match self
            .writer
            .send(WriteRequest::ChooseReleaseOffer { id })
            .await?
        {
            WriteResponse::ReleaseOfferChosen { chosen } => Ok(chosen),
            other => Err(unexpected_response("choose_release_offer", &other)),
        }
    }

    /// Marks one offer dismissed, answering whether a row changed.
    ///
    /// # Errors
    ///
    /// StorageError when the statement fails.
    pub async fn dismiss_release_offer(&self, id: i64) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::DismissReleaseOffer { id })
            .await?
        {
            WriteResponse::RowChanged { changed } => Ok(changed),
            other => Err(unexpected_response("dismiss_release_offer", &other)),
        }
    }

    /// The offers still on the table for one title.
    ///
    /// # Errors
    ///
    /// StorageError when the read fails.
    pub async fn list_release_offers(
        &self,
        title_id: i64,
    ) -> Result<Vec<crate::offers::OfferRow>, StorageError> {
        crate::offers::list_offers(&self.readers, title_id).await
    }

    /// Writes one coalesced statistics tick, reporting whether the job was there.
    pub async fn record_download_statistics(
        &self,
        id: i64,
        statistics: DownloadStatistics,
    ) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::RecordDownloadStatistics {
                id,
                statistics: Box::new(statistics),
            })
            .await?
        {
            WriteResponse::RowChanged { changed } => Ok(changed),
            other => Err(unexpected_response("record_download_statistics", &other)),
        }
    }

    /// Writes a job's `download_file` rows from the metadata's list and the engine's selection.
    pub async fn select_download_files(
        &self,
        job_id: i64,
        description: TorrentDescription,
        selection: &[usize],
    ) -> Result<(), StorageError> {
        match self
            .writer
            .send(WriteRequest::SelectDownloadFiles {
                job_id,
                description: Box::new(description),
                selection: selection.to_vec(),
            })
            .await?
        {
            WriteResponse::Applied => Ok(()),
            other => Err(unexpected_response("select_download_files", &other)),
        }
    }

    /// Persists the engine's own state for one job, reporting whether the job was there.
    pub async fn record_download_engine_state(
        &self,
        id: i64,
        state: &[u8],
    ) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::RecordEngineState {
                id,
                state: state.to_vec(),
            })
            .await?
        {
            WriteResponse::RowChanged { changed } => Ok(changed),
            other => Err(unexpected_response("record_download_engine_state", &other)),
        }
    }

    /// One download job, or `None` when no such row exists or `scope` may not see its title.
    pub async fn download_job(
        &self,
        scope: AccountScope,
        id: i64,
    ) -> Result<Option<DownloadJobRow>, StorageError> {
        crate::download::job(&self.readers, scope, id).await
    }

    /// A page of download jobs in `states`, newest first.
    pub async fn list_download_jobs(
        &self,
        scope: AccountScope,
        states: &[DownloadState],
        request: PageRequest,
    ) -> Result<Vec<DownloadJobRow>, StorageError> {
        crate::download::list_jobs(&self.readers, scope, states, request).await
    }

    /// One job's file rows, in the metadata's own order.
    pub async fn download_files(
        &self,
        scope: AccountScope,
        job_id: i64,
    ) -> Result<Vec<DownloadFileRow>, StorageError> {
        crate::download::files(&self.readers, scope, job_id).await
    }

    /// The engine's persisted state for one job, when `scope` may see its title.
    pub async fn download_engine_state(
        &self,
        scope: AccountScope,
        id: i64,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        crate::download::engine_state(&self.readers, scope, id).await
    }

    /// Inserts one title in its `stub` state and reports its row id.
    ///
    /// A stub is the identity a provider sync hangs its data off: the row exists, the name
    /// is known, and every field a provider fills is still NULL until that sync runs
    /// (PG-E5-06).
    pub async fn insert_stub_title(&self, stub: StubTitle) -> Result<i64, StorageError> {
        match self
            .writer
            .send(WriteRequest::InsertStubTitle(Box::new(stub)))
            .await?
        {
            WriteResponse::TitleInserted { id } => Ok(id),
            other => Err(unexpected_response("insert_stub_title", &other)),
        }
    }

    /// Attaches external ids to a title, refusing any that another title owns.
    ///
    /// # Errors
    ///
    /// [`StorageError::Conflict`] when an id in `ids` is already attached elsewhere; nothing
    /// in the call is applied in that case.
    pub async fn attach_external_ids(
        &self,
        title_id: i64,
        ids: &[TitleExternalId],
    ) -> Result<(), StorageError> {
        match self
            .writer
            .send(WriteRequest::AttachExternalIds {
                title_id,
                ids: ids.to_vec(),
            })
            .await?
        {
            WriteResponse::ExternalIdsAttached => Ok(()),
            other => Err(unexpected_response("attach_external_ids", &other)),
        }
    }

    /// One title's identity, when `scope` may see it.
    ///
    /// A title outside the scope reads as `None` rather than as a refusal: an account must
    /// not be able to tell a title it may not see from one that does not exist (AGENTS R34).
    pub async fn title_identity(
        &self,
        scope: AccountScope,
        id: i64,
    ) -> Result<Option<TitleIdentity>, StorageError> {
        crate::catalogue::identity(&self.readers, scope, id).await
    }

    /// The newest page of titles `scope` may see.
    pub async fn list_titles(
        &self,
        scope: AccountScope,
        request: PageRequest,
    ) -> Result<Vec<TitleIdentity>, StorageError> {
        crate::catalogue::list_titles(&self.readers, scope, request).await
    }

    /// Finds the title one provider id names, or creates it as a stub and attaches the id.
    ///
    /// One write request rather than a read-then-insert, because the two are one fact: see
    /// `catalogue::upsert_provider_title` for the window that splitting them leaves open.
    ///
    /// # Errors
    ///
    /// [`StorageError`] when the statement fails.
    pub async fn upsert_provider_title(
        &self,
        title: crate::catalogue::ProviderTitle,
    ) -> Result<i64, StorageError> {
        match self
            .writer
            .send(WriteRequest::UpsertProviderTitle(Box::new(title)))
            .await?
        {
            WriteResponse::TitleInserted { id } => Ok(id),
            other => Err(unexpected_response("upsert_provider_title", &other)),
        }
    }

    /// Writes what a provider said onto an existing title, and reindexes it.
    ///
    /// # Errors
    ///
    /// [`StorageError`] when the statement fails.
    pub async fn update_title_metadata(
        &self,
        id: i64,
        details: peregrine_domain::catalogue::TitleDetails,
        sort_name: String,
        state: crate::MetadataState,
        provider: String,
        at: i64,
    ) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::UpdateTitleMetadata {
                id,
                details: Box::new(details),
                sort_name,
                state,
                provider,
                at,
            })
            .await?
        {
            WriteResponse::RowChanged { changed } => Ok(changed),
            other => Err(unexpected_response("update_title_metadata", &other)),
        }
    }

    /// Every title the Discover rails held when they were last published, in rail and then in
    /// that rail's own order.
    ///
    /// The manifest is membership: what a rail draws comes from the `title` row and the library,
    /// which [`crate::DiscoverRailRow`] documents. A title outside the scope is absent from the
    /// answer rather than reported, for the reason every scoped read gives (AGENTS R34).
    pub async fn discover_rails(
        &self,
        scope: peregrine_domain::AccountScope,
    ) -> Result<Vec<crate::DiscoverRailRow>, StorageError> {
        crate::discover_rail::read(&self.readers, scope).await
    }

    /// Replaces the whole rails manifest with what one fetch published.
    ///
    /// Whole, and not per rail, because that is what a publish is: the caller has one run's
    /// answer, and a manifest that kept a rail this run did not answer would hydrate a screen
    /// with membership the last publish does not have.
    ///
    /// # Errors
    ///
    /// [`StorageError`] when a statement fails, including a title the catalogue does not hold —
    /// the manifest names catalogue rows, and the transaction leaves the previous manifest in
    /// place rather than writing a partial one.
    pub async fn replace_discover_rails(
        &self,
        rails: &[(String, Vec<i64>)],
    ) -> Result<(), StorageError> {
        match self
            .writer
            .send(WriteRequest::ReplaceDiscoverRails {
                rails: rails.to_vec(),
            })
            .await?
        {
            WriteResponse::Applied => Ok(()),
            other => Err(unexpected_response("replace_discover_rails", &other)),
        }
    }

    /// The local titles a candidate name could be, with everything the matching rule reads.
    ///
    /// Unscoped on purpose, like `title_for_external_id`: this is the sync's own view for deciding
    /// whether an answer is a title it already has, not a read that shows anything to an account.
    pub async fn local_titles_for_sort_name(
        &self,
        kind: &str,
        sort_name: &str,
    ) -> Result<Vec<peregrine_domain::matching::LocalTitle>, StorageError> {
        crate::catalogue::local_titles_for_sort_name(&self.readers, kind, sort_name).await
    }

    /// Every alias of one title.
    pub async fn aliases_of(&self, title_id: i64) -> Result<Vec<String>, StorageError> {
        crate::catalogue::aliases(&self.readers, title_id).await
    }

    /// The title an external id belongs to, or `None` when no title has it.
    pub async fn title_for_external_id(
        &self,
        id: &peregrine_domain::ExternalId,
    ) -> Result<Option<i64>, StorageError> {
        crate::catalogue::title_for(&self.readers, id).await
    }

    /// Every external id attached to a title, in provider order.
    pub async fn external_ids_of(
        &self,
        title_id: i64,
    ) -> Result<Vec<TitleExternalId>, StorageError> {
        crate::catalogue::external_ids(&self.readers, title_id).await
    }

    /// How many titles the catalogue holds.
    pub async fn title_count(&self) -> Result<i64, StorageError> {
        crate::catalogue::title_count(&self.readers).await
    }

    /// One page of titles `scope` may see that match `query`, the highest id first.
    ///
    /// `query` is the user's own text, and this is the only place it reaches `FTS5`: it is
    /// turned into a `MATCH` expression of quoted terms, so a query the grammar would refuse
    /// is answered with an empty page instead of an error (AGENTS R34 and §4's hostile row).
    /// An empty or whitespace-only query is not a search and reads nothing.
    pub async fn search(
        &self,
        scope: peregrine_domain::AccountScope,
        query: &str,
        request: PageRequest,
    ) -> Result<Vec<TitleIdentity>, StorageError> {
        crate::fts::search(&self.readers, scope, query, request).await
    }

    /// One account-scoped page matching the query and optional title kind, filtered before its limit.
    pub async fn search_kind(
        &self,
        scope: peregrine_domain::AccountScope,
        query: &str,
        request: PageRequest,
        kind: Option<peregrine_domain::catalogue::TitleKind>,
    ) -> Result<Vec<TitleIdentity>, StorageError> {
        crate::fts::search_kind(&self.readers, scope, query, request, kind).await
    }

    /// Rewrites one title's search index row from the source tables.
    ///
    /// Returns `false` when no title holds `title_id`, in which case nothing was indexed.
    /// The caller is the provider sync that changed a title's name or its cast; nothing in
    /// the repository layer updates a `title` row yet (PG-E5-06), so this is exercised by
    /// the tests and is the call that path will make.
    pub async fn reindex_title(&self, title_id: i64) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::ReindexTitle { title_id })
            .await?
        {
            WriteResponse::TitleReindexed { indexed } => Ok(indexed),
            other => Err(unexpected_response("reindex_title", &other)),
        }
    }

    /// Removes one title's search index row, reporting whether there was one.
    ///
    /// The title itself is not touched: this is the index half of a removal, and the row is
    /// what is left when the catalogue still holds the title.
    pub async fn remove_title_index(&self, title_id: i64) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::RemoveTitleIndex { title_id })
            .await?
        {
            WriteResponse::TitleIndexRemoved { removed } => Ok(removed),
            other => Err(unexpected_response("remove_title_index", &other)),
        }
    }

    /// Rebuilds the whole search index from the source tables, reporting how many titles it
    /// indexed.
    ///
    /// It runs through the single writer, so it cannot interleave with a title being indexed,
    /// and it is the recovery path `DATA_MODEL` §6 names after a dataset reimport or a change
    /// to what the index reads.
    pub async fn rebuild_search_index(&self) -> Result<u64, StorageError> {
        match self.writer.send(WriteRequest::RebuildSearchIndex).await? {
            WriteResponse::SearchIndexRebuilt { indexed } => Ok(indexed),
            other => Err(unexpected_response("rebuild_search_index", &other)),
        }
    }

    /// What a retention run at `at_millis` would remove, before anything is removed.
    ///
    /// The instant arrives as a value (AGENTS R14) and the answer is a plan the caller can log
    /// or show (AGENTS R22); nothing is deleted until [`Database::apply_retention`] is handed
    /// that plan.
    pub async fn plan_retention(&self, at_millis: i64) -> Result<RetentionPlan, StorageError> {
        crate::retention::plan(&self.readers, at_millis).await
    }

    /// Applies a retention plan through the single writer.
    ///
    /// Best-effort per table, like the pre-migration copy rotation: a table whose statement
    /// fails is reported in the outcome and the remaining tables still run. The one error
    /// returned is a stopped writer, when nothing can be applied at all.
    pub async fn apply_retention(
        &self,
        plan: &RetentionPlan,
    ) -> Result<RetentionOutcome, StorageError> {
        crate::retention::apply(&self.writer, plan).await
    }

    /// What a maintenance run at `request.at_millis` would do, before any of it happens.
    ///
    /// The instant and §6's two intervals arrive as values (AGENTS R14); the `VACUUM` half of
    /// the fragmentation trigger is measured here, from the file. Planning writes nothing, so
    /// it is allowed while playback is active — the gate belongs to
    /// [`Database::apply_maintenance`], which is what runs the plan.
    pub async fn plan_maintenance(
        &self,
        request: MaintenanceRequest,
    ) -> Result<MaintenancePlan, StorageError> {
        crate::maintenance::plan(&self.readers, request).await
    }

    /// Applies a maintenance plan through the single writer.
    ///
    /// `is_playing` is the caller's answer to "is something playing right now?", asked before
    /// every step: this adapter may not name the player (AGENTS R6), so the answer is a
    /// predicate owned by whoever can see both crates — `peregrine-app`'s maintenance job. A
    /// step that finds playback running is declined; a step that fails is reported and the
    /// remaining steps still run. The one error returned is a stopped writer, when nothing can
    /// run at all.
    pub async fn apply_maintenance<F>(
        &self,
        plan: &MaintenancePlan,
        is_playing: F,
    ) -> Result<MaintenanceOutcome, StorageError>
    where
        F: Fn() -> bool + Send,
    {
        crate::maintenance::apply(&self.writer, plan, &is_playing).await
    }

    /// The bytes the database can account for, per `DATA_MODEL` §7 tier.
    ///
    /// Every number is the database's own, and each entry names the side of the
    /// database/filesystem line it came from ([`crate::MeasuredFrom`]); the filesystem's answer
    /// is a directory walk this crate does not do.
    pub async fn disk_usage(&self) -> Result<DiskUsage, StorageError> {
        crate::usage::disk_usage(&self.readers).await
    }

    /// The library's recorded bytes by root and quality label, for the `scope` that may see them.
    ///
    /// This is the §4 query the storage screen and the free-space floor guard read.
    pub async fn library_storage_usage(
        &self,
        scope: AccountScope,
    ) -> Result<Vec<LibraryUsageRow>, StorageError> {
        crate::usage::library_storage_usage(&self.readers, scope).await
    }

    /// Inserts one library root and reports the id `SQLite` assigned.
    pub async fn insert_library_root(&self, root: LibraryRoot) -> Result<i64, StorageError> {
        match self
            .writer
            .send(WriteRequest::InsertLibraryRoot(Box::new(root)))
            .await?
        {
            WriteResponse::RowInserted { id } => Ok(id),
            other => Err(unexpected_response("insert_library_root", &other)),
        }
    }

    /// One library root, or `None` when no such row exists.
    ///
    /// No [`AccountScope`] applies and none is asked for: a root is a directory the user
    /// configured, not content.
    pub async fn library_root(&self, id: i64) -> Result<Option<LibraryRoot>, StorageError> {
        crate::library::root(&self.readers, id).await
    }

    /// The root at `path`, or `None`.
    ///
    /// `path` is `UNIQUE`, so this is an index lookup. It is how a caller decides whether a folder
    /// is already a root before inserting it; the constraint, not this check, is what stops a
    /// duplicate that two callers race into.
    pub async fn library_root_by_path(
        &self,
        path: &str,
    ) -> Result<Option<LibraryRoot>, StorageError> {
        crate::library::root_by_path(&self.readers, path).await
    }

    /// Removes one library root, reporting whether the row was there.
    ///
    /// Only the `library_root` row goes. Media items, files, watch progress, and subtitles are
    /// untouched (PG-E14-01), so removing a folder stops it being scanned and forgets nothing the
    /// user watched.
    pub async fn remove_library_root(&self, id: i64) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::RemoveLibraryRoot { id })
            .await?
        {
            WriteResponse::RowChanged { changed } => Ok(changed),
            other => Err(unexpected_response("remove_library_root", &other)),
        }
    }

    /// Changes when one root is scanned, reporting whether the row was there.
    pub async fn set_root_scan_mode(&self, id: i64, mode: ScanMode) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::SetRootScanMode { id, mode })
            .await?
        {
            WriteResponse::RowChanged { changed } => Ok(changed),
            other => Err(unexpected_response("set_root_scan_mode", &other)),
        }
    }

    /// A page of library roots, newest first.
    pub async fn list_library_roots(
        &self,
        request: PageRequest,
    ) -> Result<Vec<crate::library::LibraryRootRow>, StorageError> {
        crate::library::list_roots(&self.readers, request).await
    }

    /// Records that a root was scanned at `at`, reporting whether the row was there.
    pub async fn record_root_scan(&self, id: i64, at: i64) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::RecordRootScan { id, at })
            .await?
        {
            WriteResponse::RowChanged { changed } => Ok(changed),
            other => Err(unexpected_response("record_root_scan", &other)),
        }
    }

    /// Inserts one media item and reports the id `SQLite` assigned.
    ///
    /// The two partial unique indices refuse a second film item for one title or a second item
    /// for one episode (I2); that arrives as a statement failure rather than a silent second
    /// row.
    pub async fn insert_media_item(&self, item: MediaItem) -> Result<i64, StorageError> {
        match self
            .writer
            .send(WriteRequest::InsertMediaItem(Box::new(item)))
            .await?
        {
            WriteResponse::RowInserted { id } => Ok(id),
            other => Err(unexpected_response("insert_media_item", &other)),
        }
    }

    /// One media item, when `scope` may see the title it is for.
    pub async fn media_item(
        &self,
        scope: AccountScope,
        id: i64,
    ) -> Result<Option<MediaItem>, StorageError> {
        crate::library::item(&self.readers, scope, id).await
    }

    /// The item for one title or one episode, when `scope` may see it.
    pub async fn media_item_for(
        &self,
        scope: AccountScope,
        title_id: i64,
        episode_id: Option<i64>,
    ) -> Result<Option<MediaItem>, StorageError> {
        crate::library::item_for(&self.readers, scope, title_id, episode_id).await
    }

    /// A page of one title's items, newest first, when `scope` may see the title.
    pub async fn list_media_items(
        &self,
        scope: AccountScope,
        title_id: i64,
        request: PageRequest,
    ) -> Result<Vec<crate::library::MediaItemRow>, StorageError> {
        crate::library::list_items_for_title(&self.readers, scope, title_id, request).await
    }

    /// Every item of one title with the files and jobs its availability is derived from.
    ///
    /// One read rather than three per screen: `peregrine_domain::availability` and
    /// `peregrine_domain::TitleOffer` read the whole evidence set, and a caller that assembled it
    /// from the paged reads would truncate a long series or issue two queries per episode.
    pub async fn title_evidence(
        &self,
        scope: AccountScope,
        title_id: i64,
    ) -> Result<crate::library::TitleEvidence, StorageError> {
        crate::library::title_evidence(&self.readers, scope, title_id).await
    }

    /// Inserts one media file and reports the id `SQLite` assigned.
    ///
    /// `media_file.path` is `UNIQUE`, so two items can never claim one file (I3): the second
    /// insert fails rather than producing a second row for the same bytes.
    pub async fn insert_media_file(&self, file: MediaFile) -> Result<i64, StorageError> {
        match self
            .writer
            .send(WriteRequest::InsertMediaFile(Box::new(file)))
            .await?
        {
            WriteResponse::RowInserted { id } => Ok(id),
            other => Err(unexpected_response("insert_media_file", &other)),
        }
    }

    /// Deletes one `media_file` row by its path, reporting whether the row was there.
    ///
    /// The deletion path (`AGENTS` R22) calls this once per file it removed, so the library stops
    /// claiming a copy that no longer exists. The item is kept: it is the history that makes the
    /// title `Availability::Removed` rather than a title that was never acquired.
    pub async fn delete_media_file_by_path(&self, path: &str) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::DeleteMediaFileByPath {
                path: path.to_owned(),
            })
            .await?
        {
            WriteResponse::RowChanged { changed } => Ok(changed),
            other => Err(unexpected_response("delete_media_file_by_path", &other)),
        }
    }

    /// One file, when `scope` may see the title its item is for.
    pub async fn media_file(
        &self,
        scope: AccountScope,
        id: i64,
    ) -> Result<Option<MediaFile>, StorageError> {
        crate::library::file(&self.readers, scope, id).await
    }

    /// The file a path names, when `scope` may see it.
    pub async fn media_file_by_path(
        &self,
        scope: AccountScope,
        path: &str,
    ) -> Result<Option<MediaFile>, StorageError> {
        crate::library::file_by_path(&self.readers, scope, path).await
    }

    /// A page of one item's files, newest first, when `scope` may see the item.
    pub async fn list_media_files(
        &self,
        scope: AccountScope,
        media_item_id: i64,
        request: PageRequest,
    ) -> Result<Vec<crate::library::MediaFileRow>, StorageError> {
        crate::library::list_files_for_item(&self.readers, scope, media_item_id, request).await
    }

    /// The present files of one root, with the two numbers a scan compares.
    ///
    /// Served by the partial index migration 0011 adds, so this is an index read per library
    /// rather than a walk of every `media_file` row — which matters because the table's pages hold
    /// `probe_json`.
    pub async fn present_files(
        &self,
        root_id: i64,
    ) -> Result<Vec<crate::library::RecordedFile>, StorageError> {
        crate::library::present_files(&self.readers, root_id).await
    }

    /// The files of one root that are marked missing.
    ///
    /// What a restore is decided against: a path in this set that a scan sees again is a file that
    /// came back, and PG-E14-13's rule is that its row — and the watch progress hanging off its
    /// item — was never removed in the first place.
    pub async fn missing_files(
        &self,
        root_id: i64,
    ) -> Result<Vec<crate::library::RecordedFile>, StorageError> {
        crate::library::missing_files(&self.readers, root_id).await
    }

    /// Records that one image is cached, or refreshes where it is and how big it is.
    ///
    /// Written *after* the bytes are published, never before: a row that outlives its file is a row
    /// an eviction tolerates, and a file that outlives its row is a leak only a cache wipe reaps
    /// (PG-E6-18). The instants and the hit count of a row that already exists are kept — see
    /// [`ImageCacheEntry`].
    pub async fn record_image(
        &self,
        entry: crate::image_cache::ImageCacheEntry,
    ) -> Result<(), StorageError> {
        match self
            .writer
            .send(WriteRequest::RecordImageCache(Box::new(entry)))
            .await?
        {
            WriteResponse::Applied => Ok(()),
            other => Err(unexpected_response("record_image", &other)),
        }
    }

    /// One cached image, or `None` when no row holds that key.
    pub async fn image_cache_entry(
        &self,
        key: &str,
    ) -> Result<Option<crate::image_cache::ImageCacheEntry>, StorageError> {
        crate::image_cache::entry(&self.readers, key).await
    }

    /// How many images are cached and how many bytes they account for, which is the number a size
    /// cap is compared against (PG-E6-10) and what the artwork tier reports.
    /// Sets or clears one cached image's pin.
    ///
    /// What "in use" means to the eviction policy: whoever displays or is fetching an image pins it
    /// and clears the pin when they are done, and the policy never touches a pinned row.
    pub async fn pin_image(&self, key: &str, pinned: bool) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::PinImage {
                key: key.to_owned(),
                pinned,
            })
            .await?
        {
            WriteResponse::RowChanged { changed } => Ok(changed),
            other => Err(unexpected_response("pin_image", &other)),
        }
    }

    /// Evicts least-recently-used entries until the cache fits `cap_bytes`.
    ///
    /// The whole policy is `peregrine_domain`'s: this reads the cache, asks it what to forget, and
    /// removes that. **Pinned rows are never chosen** — they are in use — and a cap that cannot be met
    /// while they are held is reported rather than broken.
    ///
    /// Deletes the rows only. The blobs on disk are `peregrine-sync`'s store, and nothing writes one
    /// yet: `PG-E6-09`'s fetch path is what will, and deleting the file alongside the row belongs with
    /// it.
    ///
    /// # Errors
    ///
    /// [`StorageError`] when a read or a write fails.
    pub async fn evict_image_cache(
        &self,
        cap_bytes: i64,
    ) -> Result<crate::image_cache::Evicted, StorageError> {
        let entries = crate::image_cache::entries(&self.readers).await?;
        let plan = peregrine_domain::cache::plan_eviction(&entries, cap_bytes);
        if plan.keys.is_empty() {
            return Ok(crate::image_cache::Evicted {
                cap_unreachable: plan.cap_unreachable,
                ..crate::image_cache::Evicted::default()
            });
        }
        let removed = match self
            .writer
            .send(WriteRequest::RemoveImages {
                keys: plan.keys.clone(),
            })
            .await?
        {
            WriteResponse::ImagesRemoved { removed } => removed,
            other => return Err(unexpected_response("evict_image_cache", &other)),
        };
        Ok(crate::image_cache::Evicted {
            keys: plan.keys,
            bytes: plan.bytes,
            removed,
            // The plan's answer stands: the cap is unreachable if what is *left* is still over it,
            // whatever the removal did. A row that vanished between the read and the delete makes the
            // removal smaller than the plan, not the cap any more reachable.
            cap_unreachable: plan.cap_unreachable,
        })
    }

    /// How many rows the cache holds and what they weigh together.
    pub async fn image_cache_totals(&self) -> Result<crate::image_cache::Totals, StorageError> {
        crate::image_cache::totals(&self.readers).await
    }

    /// Reports that one cached image was read at `at`, answering whether the row was there.
    ///
    /// A hit is the caller's answer rather than this one's: only the caller knows whether the file
    /// it found was the one it was looking for.
    pub async fn touch_image(&self, key: &str, at: i64) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::TouchImageCache {
                key: key.to_owned(),
                at,
            })
            .await?
        {
            WriteResponse::RowChanged { changed } => Ok(changed),
            other => Err(unexpected_response("touch_image", &other)),
        }
    }

    /// Removes one `image_cache` row, answering whether it was there.
    ///
    /// The blob is not touched: this layer knows only the path the row held, and the layer that
    /// owns the bytes removes the file first and the row second (PG-E6-10).
    pub async fn remove_image(&self, key: &str) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::RemoveImageCache {
                key: key.to_owned(),
            })
            .await?
        {
            WriteResponse::RowChanged { changed } => Ok(changed),
            other => Err(unexpected_response("remove_image", &other)),
        }
    }

    /// A page of the titles the library holds, newest first, with what it holds of each.
    ///
    /// The library screen's read, and a scoped one: this is a caller asking what *it* has, so the
    /// scope applies (I25) and a title outside it is indistinguishable from one that is not there
    /// (I37).
    pub async fn list_library_titles(
        &self,
        scope: AccountScope,
        request: PageRequest,
    ) -> Result<Vec<crate::library::LibraryTitleRow>, StorageError> {
        crate::library::list_library_titles(&self.readers, scope, request).await
    }

    /// One title, with the counts a screen shows and the artwork a provider fill recorded, or `None`
    /// when `scope` may not see it.
    ///
    /// Scoped for the reason the page above is, and the `None` is the whole of what a hidden title
    /// and an absent one have in common (I37).
    pub async fn library_title(
        &self,
        scope: AccountScope,
        id: i64,
    ) -> Result<Option<crate::library::TitleDetailRow>, StorageError> {
        crate::library::library_title(&self.readers, scope, id).await
    }

    /// A title's episodes, in the order a person watches them, optionally one season of them.
    ///
    /// Bounded rather than paged: a title's episodes are a few hundred rows at most, and a keyset
    /// cursor over a `(season, episode)` pair would need a two-column cursor for no gain.
    ///
    /// `season_number` is the switcher's filter and `None` is every season, because the two questions
    /// differ by one predicate and a second read would be the one that drifts.
    pub async fn list_title_episodes(
        &self,
        scope: AccountScope,
        title_id: i64,
        season_number: Option<u32>,
        limit: u32,
    ) -> Result<Vec<crate::library::TitleEpisodeRow>, StorageError> {
        crate::library::list_title_episodes(&self.readers, scope, title_id, season_number, limit)
            .await
    }

    /// A title's seasons, in the order a switcher lists them.
    ///
    /// Scoped like the episode read above, and bounded for the same reason: a show's seasons are a
    /// handful of rows.
    pub async fn list_title_seasons(
        &self,
        scope: AccountScope,
        title_id: i64,
        limit: u32,
    ) -> Result<Vec<crate::library::TitleSeasonRow>, StorageError> {
        crate::library::list_title_seasons(&self.readers, scope, title_id, limit).await
    }

    /// Inserts one season and answers the id `SQLite` assigned.
    ///
    /// A stub, like [`Database::insert_stub_title`]: the library creates the row so episodes have
    /// somewhere to go, and a provider sync fills in its name, overview, and artwork later.
    pub async fn insert_stub_season(
        &self,
        season: crate::catalogue::StubSeason,
    ) -> Result<i64, StorageError> {
        match self
            .writer
            .send(WriteRequest::InsertStubSeason(Box::new(season)))
            .await?
        {
            WriteResponse::RowInserted { id } => Ok(id),
            other => Err(unexpected_response("insert_stub_season", &other)),
        }
    }

    /// Inserts one episode and answers the id `SQLite` assigned.
    pub async fn insert_stub_episode(
        &self,
        episode: crate::catalogue::StubEpisode,
    ) -> Result<i64, StorageError> {
        match self
            .writer
            .send(WriteRequest::InsertStubEpisode(Box::new(episode)))
            .await?
        {
            WriteResponse::RowInserted { id } => Ok(id),
            other => Err(unexpected_response("insert_stub_episode", &other)),
        }
    }

    /// Writes what a provider said about one season, and answers its row id with whether anything
    /// moved.
    ///
    /// An upsert on the season's own unique key, so a stub the library created is filled rather than
    /// duplicated; `changed` is `false` when the statement stored the values that were already there,
    /// which is what makes a re-run's idempotence observable (see
    /// `catalogue::upsert_season_details` for the `COALESCE` and ordering rules it obeys).
    ///
    /// # Errors
    ///
    /// [`StorageError`] when the statement fails.
    pub async fn upsert_season_details(
        &self,
        title_id: i64,
        details: peregrine_domain::catalogue::SeasonDetails,
        provider: String,
        at: i64,
    ) -> Result<(i64, bool), StorageError> {
        match self
            .writer
            .send(WriteRequest::UpsertSeasonDetails {
                title_id,
                details: Box::new(details),
                provider,
                at,
            })
            .await?
        {
            WriteResponse::RowUpserted { id, changed } => Ok((id, changed)),
            other => Err(unexpected_response("upsert_season_details", &other)),
        }
    }

    /// Writes what a provider said about one episode, and answers whether anything moved.
    ///
    /// Keyed on the canonical numbers, and descriptive columns only: enrichment never renumbers an
    /// episode and never moves it between seasons (`catalogue::upsert_episode_details`).
    ///
    /// # Errors
    ///
    /// [`StorageError`] when the statement fails.
    pub async fn upsert_episode_details(
        &self,
        title_id: i64,
        season_id: i64,
        details: peregrine_domain::catalogue::EpisodeDetails,
        provider: String,
        at: i64,
    ) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::UpsertEpisodeDetails {
                title_id,
                season_id,
                details: Box::new(details),
                provider,
                at,
            })
            .await?
        {
            WriteResponse::RowChanged { changed } => Ok(changed),
            other => Err(unexpected_response("upsert_episode_details", &other)),
        }
    }

    /// Records that a provider has answered this title's season step.
    ///
    /// A fact of its own, not a side effect of the season writes: a show whose provider lists no
    /// seasons writes no row and is still read, and a season read that fails must leave the title due
    /// (`catalogue::mark_title_seasons_synced`).
    ///
    /// # Errors
    ///
    /// [`StorageError`] when the statement fails.
    pub async fn mark_title_seasons_synced(
        &self,
        title_id: i64,
        at: i64,
    ) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::MarkTitleSeasonsSynced { id: title_id, at })
            .await?
        {
            WriteResponse::RowChanged { changed } => Ok(changed),
            other => Err(unexpected_response("mark_title_seasons_synced", &other)),
        }
    }

    /// The id of the season a title and number already name, or `None`.
    ///
    /// Part of the **scanner's** surface: a season row is created by a scan, so the scan is what has
    /// to find it again (`crate::library`'s module doc states why the scanner's reads take no
    /// `AccountScope`).
    pub async fn season_id_for(
        &self,
        title_id: i64,
        season_number: u32,
    ) -> Result<Option<i64>, StorageError> {
        crate::catalogue::season_id_for(&self.readers, title_id, season_number).await
    }

    /// The id of the episode a title, season, and number already name, or `None`.
    ///
    /// The scanner's, for the reason above, and the lookup that keeps a re-scan from creating a
    /// second episode for one number.
    pub async fn episode_id_for(
        &self,
        title_id: i64,
        season_number: u32,
        episode_number: u32,
    ) -> Result<Option<i64>, StorageError> {
        crate::catalogue::episode_id_for(&self.readers, title_id, season_number, episode_number)
            .await
    }

    /// The file at `path` and the numbers a scan compares, whatever account may see it.
    ///
    /// Part of the **scanner's** surface rather than a general read: the module doc in
    /// `crate::library` says why it takes no `AccountScope`. A caller acting for an account wants
    /// [`Database::media_file_by_path`].
    pub async fn recorded_file_at_path(
        &self,
        path: &str,
    ) -> Result<Option<crate::library::RecordedFile>, StorageError> {
        crate::library::recorded_file_at_path(&self.readers, path).await
    }

    /// The item a title and episode pair already has, whatever account may see it.
    ///
    /// The scanner's, and the lookup that keeps a re-scan from creating a second item for a title
    /// it already holds.
    pub async fn item_id_for(
        &self,
        title_id: i64,
        episode_id: Option<i64>,
    ) -> Result<Option<i64>, StorageError> {
        crate::library::item_id_for(&self.readers, title_id, episode_id).await
    }

    /// The title a kind and sort name already name, whatever account may see it.
    ///
    /// The scanner's, and the lookup that keeps a show's episodes under one title.
    pub async fn title_id_for_sort_name(
        &self,
        kind: &str,
        sort_name: &str,
    ) -> Result<Option<i64>, StorageError> {
        crate::library::title_id_for_sort_name(&self.readers, kind, sort_name).await
    }

    /// Refreshes one file's stamp and root, reporting whether the row was there.
    ///
    /// What a scan does with a file it has seen before: the bytes it read are the same file, and
    /// what changed is what the library knows about it.
    pub async fn refresh_file_stamp(
        &self,
        id: i64,
        size_bytes: i64,
        mtime_ms: i64,
        root_id: i64,
        updated_at: i64,
    ) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::RefreshFileStamp {
                id,
                size_bytes,
                mtime_ms,
                root_id,
                updated_at,
            })
            .await?
        {
            WriteResponse::RowChanged { changed } => Ok(changed),
            other => Err(unexpected_response("refresh_file_stamp", &other)),
        }
    }

    /// Removes one unmatched file, reporting whether the row was there.
    pub async fn remove_unmatched_file(&self, id: i64) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::RemoveUnmatchedFile { id })
            .await?
        {
            WriteResponse::RowChanged { changed } => Ok(changed),
            other => Err(unexpected_response("remove_unmatched_file", &other)),
        }
    }

    /// Marks one file absent at scan time, or present again.
    ///
    /// The instant is the caller's — [`MediaFile::mark_missing`] keeps the *first* one, so a
    /// file that stays gone keeps saying when it was first noticed (I11) — and `false` means
    /// the row was not there, which is not an error.
    pub async fn set_media_file_missing(
        &self,
        id: i64,
        missing_since: Option<i64>,
        updated_at: i64,
    ) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::SetFileMissing {
                id,
                missing_since,
                updated_at,
            })
            .await?
        {
            WriteResponse::RowChanged { changed } => Ok(changed),
            other => Err(unexpected_response("set_media_file_missing", &other)),
        }
    }

    /// Inserts one unmatched file and reports its id, or `None` when the path was already
    /// there.
    ///
    /// `ignore_conflict` is the scan's answer to seeing the same unplaceable file twice: with
    /// it set the second sighting leaves the first row — and its `first_seen_at` — alone.
    pub async fn insert_unmatched_file(
        &self,
        file: UnmatchedFile,
        ignore_conflict: bool,
    ) -> Result<Option<i64>, StorageError> {
        match self
            .writer
            .send(WriteRequest::InsertUnmatchedFile {
                file: Box::new(file),
                ignore_conflict,
            })
            .await?
        {
            WriteResponse::RowInserted { id } => Ok(Some(id)),
            WriteResponse::RowIgnored => Ok(None),
            other => Err(unexpected_response("insert_unmatched_file", &other)),
        }
    }

    /// Records a match rule, or moves the existing rule for that pattern, and answers its id.
    pub async fn insert_match_rule(
        &self,
        rule: crate::catalogue::NewMatchRule,
    ) -> Result<i64, StorageError> {
        match self
            .writer
            .send(WriteRequest::InsertMatchRule(Box::new(rule)))
            .await?
        {
            WriteResponse::RowInserted { id } => Ok(id),
            other => Err(unexpected_response("insert_match_rule", &other)),
        }
    }

    /// One title's name, or `None` when the id names nothing.
    pub async fn title_name(&self, id: i64) -> Result<Option<String>, StorageError> {
        crate::catalogue::title_name(&self.readers, id).await
    }

    /// The titles whose sort names fall between `lower` and `upper`, for a suggestion lookup.
    ///
    /// Part of the **scanner's** surface, like the placement lookups beside it: the indexer asks this
    /// while parking a file, and a *suggestion* is filtered where a person reads it.
    pub async fn title_candidates_between(
        &self,
        lower: &str,
        upper: &str,
    ) -> Result<Vec<peregrine_domain::library::TitleCandidate>, StorageError> {
        crate::catalogue::title_candidates_between(&self.readers, lower, upper).await
    }

    /// A page of the rules a user has made, newest first, under `scope`.
    pub async fn list_match_rules(
        &self,
        scope: AccountScope,
        request: PageRequest,
    ) -> Result<Vec<crate::catalogue::MatchRuleRow>, StorageError> {
        crate::catalogue::list_match_rules(&self.readers, scope, request).await
    }

    /// Removes one rule, reporting whether the row was there.
    pub async fn remove_match_rule(&self, id: i64) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::RemoveMatchRule { id })
            .await?
        {
            WriteResponse::RowChanged { changed } => Ok(changed),
            other => Err(unexpected_response("remove_match_rule", &other)),
        }
    }

    /// The title a user's rule places files under for one pattern, or `None` when they made none.
    pub async fn rule_title_for(&self, pattern: &str) -> Result<Option<String>, StorageError> {
        crate::catalogue::rule_title_for(&self.readers, pattern).await
    }

    /// One unmatched file, when `scope` may be offered its suggestion.
    pub async fn unmatched_file(
        &self,
        scope: AccountScope,
        id: i64,
    ) -> Result<Option<UnmatchedFile>, StorageError> {
        crate::library::unmatched(&self.readers, scope, id).await
    }

    /// A page of unmatched files, newest first.
    pub async fn list_unmatched_files(
        &self,
        scope: AccountScope,
        request: PageRequest,
    ) -> Result<Vec<crate::library::UnmatchedFileRow>, StorageError> {
        crate::library::list_unmatched(&self.readers, scope, request).await
    }

    /// How many rows `unmatched_file` holds.
    pub async fn unmatched_count(&self) -> Result<i64, StorageError> {
        crate::library::unmatched_count(&self.readers).await
    }

    /// Inserts one subtitle track and reports the id `SQLite` assigned.
    pub async fn insert_subtitle_track(&self, track: SubtitleTrack) -> Result<i64, StorageError> {
        match self
            .writer
            .send(WriteRequest::InsertSubtitleTrack(Box::new(track)))
            .await?
        {
            WriteResponse::RowInserted { id } => Ok(id),
            other => Err(unexpected_response("insert_subtitle_track", &other)),
        }
    }

    /// One subtitle track, when `scope` may see the media it belongs to.
    pub async fn subtitle_track(
        &self,
        scope: AccountScope,
        id: i64,
    ) -> Result<Option<SubtitleTrack>, StorageError> {
        crate::library::subtitle(&self.readers, scope, id).await
    }

    /// A page of one file's subtitle tracks, newest first.
    pub async fn list_subtitle_tracks(
        &self,
        scope: AccountScope,
        media_file_id: i64,
        request: PageRequest,
    ) -> Result<Vec<crate::library::SubtitleTrackRow>, StorageError> {
        crate::library::list_subtitles_for_file(&self.readers, scope, media_file_id, request).await
    }

    /// Writes one account's progress for one title or episode.
    ///
    /// The value is keyed on `(account_id, entity_key)`, so a player reporting a position twice
    /// updates one row. The percentage is the domain's derivation, never this layer's (I6).
    pub async fn record_watch_progress(&self, progress: WatchProgress) -> Result<(), StorageError> {
        match self
            .writer
            .send(WriteRequest::UpsertWatchProgress(Box::new(progress)))
            .await?
        {
            WriteResponse::Applied => Ok(()),
            other => Err(unexpected_response("record_watch_progress", &other)),
        }
    }

    /// One account's progress for one title or episode, when `scope` may see the title.
    pub async fn watch_progress(
        &self,
        scope: AccountScope,
        title_id: i64,
        episode_id: Option<i64>,
    ) -> Result<Option<WatchProgress>, StorageError> {
        crate::watch::progress_for(&self.readers, scope, title_id, episode_id).await
    }

    /// A page of what one account is part-way through, most recently updated first.
    pub async fn list_continue_watching(
        &self,
        scope: AccountScope,
        request: PageRequest,
    ) -> Result<Vec<crate::watch::WatchProgressRow>, StorageError> {
        crate::watch::list_continue(&self.readers, scope, request).await
    }

    /// A page of everything one account has watched, most recently updated first.
    pub async fn list_watch_progress(
        &self,
        scope: AccountScope,
        request: PageRequest,
    ) -> Result<Vec<crate::watch::WatchProgressRow>, StorageError> {
        crate::watch::list_progress(&self.readers, scope, request).await
    }

    /// Writes one watchlist entry.
    pub async fn add_to_watchlist(&self, entry: WatchlistEntry) -> Result<(), StorageError> {
        match self
            .writer
            .send(WriteRequest::UpsertWatchlist(Box::new(entry)))
            .await?
        {
            WriteResponse::Applied => Ok(()),
            other => Err(unexpected_response("add_to_watchlist", &other)),
        }
    }

    /// Removes one watchlist entry, reporting whether it was there.
    pub async fn remove_from_watchlist(
        &self,
        account_id: peregrine_domain::AccountId,
        title_id: i64,
    ) -> Result<bool, StorageError> {
        match self
            .writer
            .send(WriteRequest::RemoveWatchlist {
                account_id,
                title_id,
            })
            .await?
        {
            WriteResponse::RowChanged { changed } => Ok(changed),
            other => Err(unexpected_response("remove_from_watchlist", &other)),
        }
    }

    /// One watchlist entry, when `scope` may see the title.
    pub async fn watchlist_entry(
        &self,
        scope: AccountScope,
        title_id: i64,
    ) -> Result<Option<WatchlistEntry>, StorageError> {
        crate::watch::watchlist_entry(&self.readers, scope, title_id).await
    }

    /// A page of one account's watchlist, newest first.
    pub async fn list_watchlist(
        &self,
        scope: AccountScope,
        request: PageRequest,
    ) -> Result<Vec<crate::watch::WatchlistRow>, StorageError> {
        crate::watch::list_watchlist(&self.readers, scope, request).await
    }

    /// How many rows `event_log` holds, read through the pool.
    ///
    /// This is the read path in miniature — a typed query on a pooled connection — and
    /// it is what makes "no write was lost" checkable from outside the writer.
    pub async fn event_count(&self) -> Result<u64, StorageError> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM event_log")
            .fetch_one(&self.readers)
            .await
            .map_err(StorageError::from_sqlx)?;
        // `COUNT(*)` cannot be negative; the conversion exists so a count cannot be
        // mistaken for the rowid `append_event` returns.
        Ok(u64::try_from(count).unwrap_or_default())
    }
}

/// The writer answered a request it was not sent.
///
/// Unreachable by construction — the handle sends one request and reads its own answer —
/// so a mismatch is an internal invariant violation rather than a database condition.
fn unexpected_response(operation: &'static str, response: &WriteResponse) -> StorageError {
    StorageError::UnexpectedValue {
        column: operation,
        found: format!("{response:?}"),
    }
}

/// Checks an existing file for damage and rebuilds the database when it is damaged.
///
/// A file that is not there yet, or is empty, has nothing to check: that is a first install.
/// The check runs on its own connection, which is closed before anything is renamed, because
/// a filesystem that refuses to move an open file would turn detection into a failure.
async fn recover(path: &Path) -> Result<Recovery, StorageError> {
    match tokio::fs::metadata(path).await {
        Ok(metadata) if metadata.len() > 0 => {}
        Ok(_) => return Ok(Recovery::Intact),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Recovery::Intact);
        }
        Err(error) => {
            return Err(StorageError::Open {
                path: path.to_path_buf(),
                source: Box::new(error),
            });
        }
    }

    let mut connection = crate::integrity::probe(path).await?;
    let damage = crate::integrity::check(&mut connection).await?;
    connection
        .close()
        .await
        .map_err(|error| StorageError::open(path, error))?;

    let Some(reason) = damage else {
        return Ok(Recovery::Intact);
    };
    warn!(
        path = %path.display(),
        reason = %reason,
        "the database failed its integrity check"
    );

    let quarantine = crate::integrity::quarantine(path).await?;
    warn!(
        path = %path.display(),
        quarantine = %quarantine.display(),
        "a clean schema replaces the damaged database, so the library must be rescanned"
    );
    Ok(Recovery::Rebuilt { quarantine })
}

/// Opens the connection the writer task will own, with the documented pragmas applied
/// and then verified, because an existing database cannot change every one of them.
///
/// A required pragma that the database will not report fails the open with
/// [`StorageError::Pragma`]; this is the connection every write runs on, so it is the one
/// whose pragmas actually decide whether the schema's constraints are enforced.
pub(crate) async fn open_writer_connection(path: &Path) -> Result<SqliteConnection, StorageError> {
    let mut connection = connection_options(path)
        .connect()
        .await
        .map_err(|error| StorageError::open(path, error))?;
    pragmas::enforce(&mut connection, path).await?;
    Ok(connection)
}

/// Opens the reader pool with the documented pragma set on every connection.
///
/// The options cover connections the pool opens later, and the `after_connect` hook
/// verifies them, so a connection that cannot report a required pragma is discarded by
/// the pool instead of serving reads under different pragmas. `sqlx` retries a connection
/// its `after_connect` hook rejects, so that path reaches the caller as `PoolTimedOut`
/// rather than as [`StorageError::Pragma`]; the writer connection checked in `open` is the
/// one that reports the typed failure.
pub(crate) async fn open_reader_pool(path: &Path) -> Result<SqlitePool, sqlx::Error> {
    let pragma_path = path.to_path_buf();
    SqlitePoolOptions::new()
        .max_connections(reader_pool_size())
        .after_connect(move |connection, _metadata| {
            // `after_connect` can only return a driver error, so the typed pragma failure
            // travels inside one.
            let pragma_path = pragma_path.clone();
            Box::pin(async move {
                pragmas::enforce(connection, &pragma_path)
                    .await
                    .map_err(|error| sqlx::Error::Configuration(Box::new(error)))
            })
        })
        .connect_with(connection_options(path))
        .await
}

/// `min(4, cores)`, per ARCHITECTURE §10.
fn reader_pool_size() -> u32 {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    u32::try_from(cores)
        .unwrap_or(READER_POOL_MAX)
        .min(READER_POOL_MAX)
}

fn connection_options(path: &Path) -> SqliteConnectOptions {
    pragmas::apply_to(
        SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::JobPhase;
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use tempfile::TempDir;

    const WRITERS: i64 = 20;
    const WRITES_PER_WRITER: i64 = 5;

    async fn database(directory: &TempDir) -> Database {
        Database::open(directory.path().join("peregrine.sqlite"))
            .await
            .unwrap()
    }

    /// A job that a process was running when it stopped existing.
    fn unfinished_job(kind: &str, dedupe_key: &str) -> NewJob {
        NewJob {
            kind: kind.to_owned(),
            priority: 100,
            dedupe_key: Some(dedupe_key.to_owned()),
            payload_json: "{}".to_owned(),
            max_attempts: 2,
            // Injected, never the wall clock (AGENTS R14).
            created_at: 1_700_000_000_000,
        }
    }

    /// A job cannot outlive the process that was running it, and a row left non-terminal is worse
    /// than a wrong status: `job_dedupe_uq` covers the non-terminal rows only, so the key is held
    /// against every later submission — which is what left Discover joining a job that would never
    /// run, on every launch, with pressing Refresh going through the same join.
    #[tokio::test]
    async fn opening_the_database_closes_the_jobs_a_previous_process_left_unfinished() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");

        {
            let database = Database::open(&path).await.unwrap();
            database
                .insert_job(unfinished_job("discover.rails", "discover.rails"))
                .await
                .unwrap();
            // The write is the writer task's, so the file is only guaranteed to hold it once the
            // handle is gone — which is also what a killed process leaves behind.
            drop(database);
        }

        let database = Database::open(&path).await.unwrap();
        let row = database
            .list_jobs(PageRequest::first(10))
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.kind == "discover.rails")
            .expect("the row is still there, reported rather than deleted");
        assert_eq!(row.state, JobPhase::Failed);
        assert_eq!(row.last_error_code.as_deref(), Some("INTERRUPTED"));
        assert!(
            row.last_error.is_some(),
            "the row says why it stopped: {row:?}"
        );
        assert!(
            row.finished_at.is_none(),
            "the instant it stopped is exactly what nobody knows"
        );

        // And the key is free again, which is the whole point: the next launch can submit the fetch.
        assert!(
            database
                .insert_job(unfinished_job("discover.rails", "discover.rails"))
                .await
                .is_ok(),
            "a closed row must not hold its dedupe key against the next process"
        );
    }

    fn event(writer: i64, sequence: i64) -> NewEvent {
        NewEvent {
            // Injected, never the wall clock (AGENTS R14).
            at_millis: 1_700_000_000_000 + writer * WRITES_PER_WRITER + sequence,
            // A declared event, because the writer accepts nothing else (PG-E4-16); the writer
            // index stands in for a job id so the rows are distinguishable.
            event: peregrine_domain::Event::DownloadQueued { job_id: writer },
            detail_json: None,
        }
    }

    /// PG-E3-02: twenty callers writing at once must serialize behind the single writer
    /// and every write must land.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn twenty_concurrent_writers_serialize_without_busy_or_lost_writes() {
        let directory = TempDir::new().unwrap();
        let database = Arc::new(database(&directory).await);

        let mut writers = Vec::new();
        for writer in 0..WRITERS {
            let database = Arc::clone(&database);
            writers.push(tokio::spawn(async move {
                let mut outcomes = Vec::new();
                for sequence in 0..WRITES_PER_WRITER {
                    outcomes.push(database.append_event(event(writer, sequence)).await);
                }
                outcomes
            }));
        }

        let mut ids = BTreeSet::new();
        let mut busy = 0_usize;
        let mut failures = Vec::new();
        for writer in writers {
            for outcome in writer.await.unwrap() {
                match outcome {
                    Ok(id) => {
                        ids.insert(id);
                    }
                    Err(error @ StorageError::Busy { .. }) => {
                        busy += 1;
                        failures.push(error.to_string());
                    }
                    Err(error) => failures.push(error.to_string()),
                }
            }
        }

        assert_eq!(busy, 0, "no writer may see SQLITE_BUSY: {failures:#?}");
        assert!(
            failures.is_empty(),
            "twenty writers must serialize through the single writer: {failures:#?}"
        );

        let expected = usize::try_from(WRITERS * WRITES_PER_WRITER).unwrap();
        assert_eq!(
            ids.len(),
            expected,
            "a lost write would reuse or skip a rowid"
        );
        assert_eq!(
            *ids.first().unwrap(),
            1,
            "SQLite numbers a fresh table's rowids from one"
        );
        assert_eq!(*ids.last().unwrap(), i64::try_from(expected).unwrap());

        let stored = database.event_count().await.unwrap();
        assert_eq!(
            usize::try_from(stored).unwrap(),
            expected,
            "every acknowledged write must be readable from the log"
        );
    }

    #[tokio::test]
    async fn the_reader_pool_is_sized_by_cores_within_the_documented_ceiling() {
        let directory = TempDir::new().unwrap();
        let pool = open_reader_pool(&directory.path().join("peregrine.sqlite"))
            .await
            .unwrap();

        let max_connections = pool.options().get_max_connections();
        assert!((1..=READER_POOL_MAX).contains(&max_connections));
        assert_eq!(max_connections, reader_pool_size());
    }

    #[tokio::test]
    async fn reopening_an_existing_database_keeps_the_writer_working() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");

        let first = Database::open(&path).await.unwrap();
        let first_id = first.append_event(event(0, 0)).await.unwrap();
        let recovery = first.recovery().clone();
        drop(first);
        assert_eq!(recovery, Recovery::Intact);

        let second = Database::open(&path).await.unwrap();
        let second_id = second.append_event(event(0, 1)).await.unwrap();

        assert_eq!(first_id, 1);
        assert_eq!(second_id, 2, "the reopened writer continues the log");
    }

    /// The files a recovery path writes, by name: a copy taken before an upgrade and a
    /// quarantined file are the only two things a boot may add to the data directory.
    async fn recovery_artifacts(directory: &TempDir) -> Vec<String> {
        let mut names = Vec::new();
        let mut entries = tokio::fs::read_dir(directory.path()).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.contains(".backup-") || name.contains(".corrupt-") {
                names.push(name);
            }
        }
        names.sort();
        names
    }

    /// PG-E3-14's healthy path: nothing observable happens. No copy, no quarantine, and the
    /// rows that were there are still there.
    #[tokio::test]
    async fn a_healthy_database_boots_without_writing_anything() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");

        let first = Database::open(&path).await.unwrap();
        assert_eq!(*first.recovery(), Recovery::Intact);
        assert!(!first.recovery().requires_rescan());
        assert_eq!(first.append_event(event(0, 0)).await.unwrap(), 1);
        drop(first);

        let second = Database::open(&path).await.unwrap();

        assert_eq!(*second.recovery(), Recovery::Intact);
        assert_eq!(
            second.event_count().await.unwrap(),
            1,
            "the healthy path must not touch the rows"
        );
        assert!(
            recovery_artifacts(&directory).await.is_empty(),
            "a healthy boot leaves no copy and no quarantine: {:?}",
            recovery_artifacts(&directory).await
        );
    }

    /// PG-E3-14's acceptance: injected corruption is detected, the damaged file is kept
    /// rather than deleted, a clean schema replaces it, and the boot says the library has
    /// to be rescanned.
    #[tokio::test]
    async fn a_corrupt_database_is_quarantined_and_rebuilt() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");

        // A database that worked and held a row, closed cleanly so the fixture is not
        // relying on a destructor running before the file is overwritten.
        let mut connection = open_writer_connection(&path).await.unwrap();
        crate::migrations::apply(&mut connection, &path)
            .await
            .unwrap();
        sqlx::query("INSERT INTO event_log (at, kind) VALUES (0, 'test.before_corruption')")
            .execute(&mut connection)
            .await
            .unwrap();
        connection.close().await.unwrap();

        // Real corruption: the file's header is overwritten, so it is no longer a database.
        let mut bytes = tokio::fs::read(&path).await.unwrap();
        bytes[..100].fill(0xff);
        tokio::fs::write(&path, &bytes).await.unwrap();

        let database = Database::open(&path).await.unwrap();

        let Recovery::Rebuilt { quarantine } = database.recovery() else {
            panic!(
                "the damaged file must be rebuilt, got {:?}",
                database.recovery()
            );
        };
        assert!(
            database.recovery().requires_rescan(),
            "the rows are gone, so the library has to be scanned again"
        );
        assert_eq!(
            tokio::fs::read(quarantine).await.unwrap(),
            bytes,
            "the damaged file is kept byte for byte, not deleted"
        );
        assert_eq!(
            database.event_count().await.unwrap(),
            0,
            "the rebuilt schema is empty: the old rows are in the quarantined file"
        );
        let tables: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'title'",
        )
        .fetch_one(&database.readers)
        .await
        .unwrap();
        assert_eq!(tables, 1, "the clean schema is the embedded one");

        // The rebuilt database is a working one, and the next boot finds it healthy.
        assert_eq!(database.append_event(event(0, 0)).await.unwrap(), 1);
        drop(database);
        let next = Database::open(&path).await.unwrap();
        assert_eq!(*next.recovery(), Recovery::Intact);
    }

    /// A file that was never a database at all is treated the same way: the recovery path
    /// is about the file, not about how it became unreadable.
    #[tokio::test]
    async fn a_file_that_is_not_a_database_is_quarantined_too() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");
        tokio::fs::write(&path, b"this was never a database")
            .await
            .unwrap();

        let database = Database::open(&path).await.unwrap();

        let Recovery::Rebuilt { quarantine } = database.recovery() else {
            panic!("expected a rebuild, got {:?}", database.recovery());
        };
        assert_eq!(
            tokio::fs::read(&quarantine).await.unwrap(),
            b"this was never a database"
        );
        assert_eq!(database.title_count().await.unwrap(), 0);
    }

    /// The permissions edge case: damage that cannot be moved aside is reported, and the
    /// file is still there. Nothing is deleted to make the boot succeed.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_damaged_database_that_cannot_be_moved_aside_is_reported() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = TempDir::new().unwrap();
        let path = directory.path().join("peregrine.sqlite");
        tokio::fs::write(&path, b"not a database").await.unwrap();
        let original = tokio::fs::metadata(directory.path())
            .await
            .unwrap()
            .permissions();
        tokio::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o500))
            .await
            .unwrap();

        let error = Database::open(&path)
            .await
            .expect_err("a damaged file that cannot be moved must not be silently replaced");

        // Restore before asserting, or a failing assertion leaves the temporary directory
        // undeletable.
        tokio::fs::set_permissions(directory.path(), original)
            .await
            .unwrap();

        assert!(
            matches!(error, StorageError::Quarantine { .. }),
            "expected a typed quarantine failure, got {error:?}"
        );
        assert!(
            error.to_string().contains("close any other copy"),
            "the error must say what to do: {error}"
        );
        assert_eq!(
            tokio::fs::read(&path).await.unwrap(),
            b"not a database",
            "the damaged file must not be deleted when it cannot be moved"
        );
    }
}
