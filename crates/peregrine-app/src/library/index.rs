//! Turning a scan's candidates into rows: the indexer (PG-E14-05, first cut).
//!
//! A walk answers "which files are there"; the reconciliation answers "which of them are gone". This
//! module answers the question in between — *what are they* — by parsing each candidate's name and
//! writing the title, item, and file rows the library needs before anything can be shown or played.
//!
//! # What this is, and what it is not yet
//!
//! `PG-E14-05` is the match pipeline into the catalogue, and its acceptance is attribution measured
//! against the naming corpus. This is the half of it that needs no provider: a filename is parsed
//! by `peregrine-domain`'s `LibraryName` and its title becomes a **stub** — the identity a later
//! sync hangs metadata off (`PG-E5-06`) — so a library can be built, searched and played from the
//! files that are there while the metadata road is still blocked on the provider stack.
//!
//! Three things are deliberately not here, and each belongs to a task rather than to a shortcut:
//!
//! * **Provider attribution.** A stub is matched by name and year, so `Alien (1979).mkv` and
//!   `Alien (1986).mkv` are two titles and a re-release is the same one. Deciding that a parsed
//!   name *is* a provider's title needs the provider (`PG-E5-13`).
//! * **Episodes.** A name that states a season or an episode number is parked in `unmatched_file`
//!   with the reason, because placing an episode needs the scheme and the numbering rules
//!   `PG-E14-07` owns. Parking loses nothing: the parse is stored beside the path.
//! * **Sidecars.** An `.nfo` beside a film is `PG-E14-06`'s, and a `.srt` is `PG-E14-09`'s. The
//!   scan policy already refuses them as candidates, so they never reach here.
//!
//! # The scope this runs under
//!
//! None, and that is a decision rather than an omission: the reads this uses are the *scanner's*
//! surface (`crate::library`'s module doc states the whole argument). A stub is created for every
//! file the walk found, including one a restricted account will never see, because the row has to
//! exist before `I37` can hide it.
//!
//! # Idempotence
//!
//! A scan may be interrupted, cancelled, and run again, so the indexer is written to be run over the
//! same candidates repeatedly: a title is found by its sort name, an item by its title and episode,
//! and a file by its path. A second pass creates nothing and refreshes the stamps it finds, which is
//! what makes the incremental walk's answer — "this file is unchanged" — a statement about rows that
//! are already correct rather than about work that was skipped.

use std::sync::Arc;

use peregrine_domain::{
    LibraryName, MediaFile, MediaItem, MediaItemState, MediaOrigin, NonEmptyText, TitleKind,
    TitleSubtype, UnmatchedFile,
};
use peregrine_storage::{Database, NewMatchRule, StorageError, StubTitle};
use thiserror::Error;
use tracing::debug;

use crate::library::scan::CandidateFile;

/// Where a season or an episode number comes from, which is not this task's question.
const EPISODES_ARE_PG_E14_07: &str =
    "the name states a season or an episode, which PG-E14-07 places";

/// Why an index pass could not finish.
#[derive(Debug, Error)]
pub enum IndexError {
    /// The database refused a read or a write.
    #[error(transparent)]
    Storage(#[from] StorageError),

    /// A path could not be spelled as the text the schema stores.
    ///
    /// The scan policy refuses a path whose components are not text, so a candidate that reaches
    /// this is text and the error is unreachable today. It is an error rather than an `expect`
    /// because the alternative to a typed refusal is a panic on a user's machine.
    #[error("the file {} cannot be stored as text", .path.display())]
    NotText {
        /// The path.
        path: std::path::PathBuf,
    },
}

/// What one index pass wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexReport {
    /// The title the file was placed under, or zero when this pass placed none.
    ///
    /// One id rather than a list because a report answers for one candidate: the walk folds reports
    /// together for its counts, and a caller that needs the title a *file* landed under asks the
    /// placement that placed it.
    pub placed_title_id: i64,
    /// Titles created for names the catalogue did not hold.
    pub created_titles: usize,
    /// Items created — one per title, or per episode once `PG-E14-07` lands.
    pub created_items: usize,
    /// Seasons created for episode numbers the library did not hold.
    pub created_seasons: usize,
    /// Episodes created for numbers it did not hold.
    pub created_episodes: usize,
    /// Files written for candidates the library had never seen.
    pub created_files: usize,
    /// Files whose size, mtime, or root was refreshed because the library had them.
    pub refreshed_files: usize,
    /// Candidates parked for a human or a later task, with the reason.
    pub unmatched: usize,
}

impl IndexReport {
    /// The title this pass placed a file under, which the rule a correction creates points at.
    ///
    /// Zero when the pass placed neither a film nor an episode — a parked file's report names no
    /// title, and a report that invented one would point a rule at a title nothing wrote.
    #[must_use]
    pub const fn created_title(&self) -> i64 {
        self.placed_title_id
    }

    /// Names the title this pass placed a file under.
    fn under(&mut self, title_id: i64) -> &mut Self {
        self.placed_title_id = title_id;
        self
    }

    /// Whether the pass found anything new to record.
    #[must_use]
    pub const fn is_unchanged(&self) -> bool {
        self.created_titles == 0
            && self.created_seasons == 0
            && self.created_episodes == 0
            && self.created_items == 0
            && self.created_files == 0
    }
}

/// Indexes one root's candidates.
///
/// # Errors
///
/// [`IndexError::Storage`] when a read or a write fails, and [`IndexError::NotText`] for a path the
/// schema cannot hold. A single candidate's failure fails the pass: a half-indexed root is a library
/// whose report disagrees with its rows, and the next scan indexes the whole root again.
pub async fn index_candidates(
    database: &Arc<Database>,
    root_id: i64,
    candidates: &[CandidateFile],
    at: jiff::Timestamp,
) -> Result<IndexReport, IndexError> {
    let mut report = IndexReport::default();

    for candidate in candidates {
        let Some(path) = candidate.path.to_str() else {
            return Err(IndexError::NotText {
                path: candidate.path.clone(),
            });
        };

        // A file the library has already indexed is refreshed rather than re-derived: its rows
        // exist, and the only thing a walk can add is the stamp it just read.
        if let Some(known) = database.recorded_file_at_path(path).await? {
            if database
                .refresh_file_stamp(
                    known.id,
                    i64::try_from(candidate.size_bytes).unwrap_or(i64::MAX),
                    candidate.mtime_ms,
                    root_id,
                    at.as_millisecond(),
                )
                .await?
            {
                report.refreshed_files += 1;
            }
            continue;
        }

        let name = candidate.path.file_name().and_then(|name| name.to_str());
        match name.map(LibraryName::parse) {
            Some(Ok(parsed)) => {
                report
                    .absorb(&index_parsed(database, root_id, path, &parsed, candidate, at).await?);
            }
            Some(Err(error)) => {
                // Before parking, the one thing a person may already have answered: a rule for this
                // naming pattern, which is the title the parse read, normalized. `PG-E14-09`'s
                // acceptance is this line — the second file that reads the way the first one did is
                // placed without asking again.
                if let Some(placed) =
                    rule_placement(database, root_id, path, &error.name, candidate, at).await?
                {
                    report.absorb(&placed);
                } else {
                    // The parse read what it could before it stopped, and `NameError` carries it
                    // so the row loses nothing. Parked rather than dropped: this is the queue a
                    // human works through, and a file that never became a row is a file the user
                    // cannot see.
                    park(
                        database,
                        path,
                        candidate,
                        &error.name,
                        Some(error.reason.to_string()),
                        at,
                    )
                    .await?;
                    report.unmatched += 1;
                }
            }
            None => {
                return Err(IndexError::NotText {
                    path: candidate.path.clone(),
                });
            }
        }
    }

    if !report.is_unchanged() {
        debug!(
            root_id,
            titles = report.created_titles,
            items = report.created_items,
            files = report.created_files,
            refreshed = report.refreshed_files,
            unmatched = report.unmatched,
            "indexed a root's files"
        );
    }
    Ok(report)
}

impl IndexReport {
    /// Folds one candidate's contribution in.
    fn absorb(&mut self, other: &Self) {
        self.created_titles += other.created_titles;
        self.created_seasons += other.created_seasons;
        self.created_episodes += other.created_episodes;
        self.created_items += other.created_items;
        self.created_files += other.created_files;
        self.refreshed_files += other.refreshed_files;
        self.unmatched += other.unmatched;
    }
}

/// Which title a user's answer files the parked file under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchAs {
    /// The title the parse read, which is what the row's own values say.
    Parsed,
    /// The title the scan offered as a suggestion, when it offered one.
    ///
    /// The better answer when it exists: the suggestion is built from a name the catalogue *already
    /// holds*, so filing under it joins an existing title rather than creating a near-duplicate of it.
    /// A row with no suggestion falls back to [`MatchAs::Parsed`] rather than failing, because a
    /// suggestion can be filtered from view (a restricted account, I37) while the row is not.
    Suggested,
    /// The title a person chose from the catalogue.
    ///
    /// What the queue could not do without a way to search: a name that neither parses to the right
    /// title nor looks like one the library holds is a name only a person can place. The id is
    /// resolved to the title's *name* before the placement, so a chosen title takes the same path as
    /// a parsed one — and a film is what gets placed, because one file is one item and the episode a
    /// file belongs to is a different question than the title it belongs to.
    Title(i64),
}

/// What answering a parked file did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Matched {
    /// The file was placed: its title, item, and file rows now exist, and the parked row is gone.
    Placed,
    /// The file was already indexed — a scan reached it between the row being parked and the answer —
    /// so the parked row was cleared and nothing was written.
    AlreadyIndexed,
    /// The parse read no title, so there is nothing to place it as. The row stays.
    NoTitle,
}

/// Answers one parked file by placing it as the title its parse read.
///
/// `PG-E14-08`'s manual match, at the size this build can honestly offer: a file the scan could not
/// place is offered to a person with what the parse read, and confirming it writes the same rows a
/// scan would have. It deliberately does **not** re-parse the path to do it: the row exists because
/// the parse *failed*, so re-parsing would fail again. What it places from is what the parse managed
/// to read before it stopped, which is the whole reason `unmatched_file` stores those fields.
///
/// Deciding *which* title a file belongs to when the parse read the wrong one is `PG-E14-09`'s
/// memory of user corrections; this is the confirmation half.
///
/// The placed file carries no root, because the parked row does not record one — a scan is what
/// knows which folder a file came from. That is not a gap: the next scan of that root finds the path,
/// refreshes its stamp, and adopts it.
///
/// # Errors
///
/// [`IndexError::Storage`] when a read or a write fails, and [`IndexError::NotText`] for a path the
/// schema cannot hold.
pub async fn match_unmatched(
    database: &Arc<Database>,
    scope: peregrine_domain::AccountScope,
    id: i64,
    as_title: MatchAs,
    at: jiff::Timestamp,
) -> Result<Matched, IndexError> {
    let Some(row) = database.unmatched_file(scope, id).await? else {
        return Ok(Matched::NoTitle);
    };
    let path = row.path.as_str();

    // A scan may have reached the file while the row waited, in which case the row is stale rather
    // than wrong: clearing it is the whole answer, and writing again would collide on the path's
    // unique index.
    if database.recorded_file_at_path(path).await?.is_some() {
        database.remove_unmatched_file(id).await?;
        return Ok(Matched::AlreadyIndexed);
    }

    let Some(title_name) = row.parsed_title.as_deref() else {
        return Ok(Matched::NoTitle);
    };
    // The offered title's own name, when the caller asked for it and the scan offered one. Read
    // rather than taken from the request, so a client cannot name a title it may not see.
    let suggested = match as_title {
        MatchAs::Parsed => None,
        MatchAs::Suggested => match row.suggested_title_id {
            Some(title_id) => database.title_name(title_id).await?,
            None => None,
        },
        // A chosen title is read rather than trusted from the request for the same reason the
        // suggestion is: a client cannot name a title it may not see, and a client that named one
        // that does not exist gets the parse's reading rather than a row pointing at nothing.
        MatchAs::Title(title_id) => database.title_name(title_id).await?,
    };
    let title_name = suggested.as_deref().unwrap_or(title_name);
    let candidate = CandidateFile {
        path: std::path::PathBuf::from(path),
        size_bytes: row.size_bytes,
        mtime_ms: row.mtime_ms.as_millisecond(),
    };

    let title_id = match (row.parsed_season, row.parsed_episode) {
        (Some(season_number), Some(episode_number)) => episode(
            database,
            None,
            path,
            Placement {
                title_name,
                season_number: season_number.get(),
                episode_number: episode_number.get(),
            },
            &candidate,
            at,
        )
        .await?
        .created_title(),
        _ => movie_from_title(database, None, path, title_name, &candidate, at)
            .await?
            .created_title(),
    };

    // The correction becomes a pattern, so the answer outlives the file it was made for
    // (`PG-E14-09`): the next name that reads the way this one did is placed without asking.
    database
        .insert_match_rule(NewMatchRule {
            pattern: sort_name_of(title_name),
            title_id,
            at: at.as_millisecond(),
        })
        .await?;
    database.remove_unmatched_file(id).await?;
    Ok(Matched::Placed)
}

/// Indexes one parsed name.
async fn index_parsed(
    database: &Arc<Database>,
    root_id: i64,
    path: &str,
    parsed: &LibraryName,
    candidate: &CandidateFile,
    at: jiff::Timestamp,
) -> Result<IndexReport, IndexError> {
    let mut report = IndexReport::default();

    // A name that states a season *and* exactly one episode is the unambiguous case, and placing it
    // is what turns a show from one title with sixty parked files into episodes. Everything else —
    // a range, an absolute number with no season, a bare season — is parked with the reason, because
    // which episode it is needs the numbering rules PG-E14-07 owns: a range is several episodes or
    // one file that holds them, an absolute number may or may not be the aired one, and a special
    // belongs in season zero by a convention the parser states and this task does not own.
    let (Some(season_number), Some(span)) = (parsed.season(), parsed.episodes()) else {
        if parsed.season().is_some() || parsed.episodes().is_some() || parsed.absolute().is_some() {
            park(
                database,
                path,
                candidate,
                parsed,
                Some(EPISODES_ARE_PG_E14_07.to_owned()),
                at,
            )
            .await?;
            report.unmatched += 1;
        } else {
            report.absorb(&movie(database, root_id, path, parsed, candidate, at).await?);
        }
        return Ok(report);
    };
    if span.is_multi() {
        park(
            database,
            path,
            candidate,
            parsed,
            Some(EPISODES_ARE_PG_E14_07.to_owned()),
            at,
        )
        .await?;
        report.unmatched += 1;
        return Ok(report);
    }

    let Some(title_name) = parsed.title() else {
        // Nothing was read to remember, so a rule cannot exist for this name and the row is parked
        // for a person instead (`PG-E14-09` matches on the title a parse read, and there is none).
        park(
            database,
            path,
            candidate,
            parsed,
            Some("the name states no title".to_owned()),
            at,
        )
        .await?;
        report.unmatched += 1;
        return Ok(report);
    };
    report.absorb(
        &episode(
            database,
            Some(root_id),
            path,
            Placement {
                title_name,
                season_number,
                episode_number: span.first(),
            },
            candidate,
            at,
        )
        .await?,
    );
    Ok(report)
}

/// Indexes one film: its title, its single item, and its file.
async fn movie(
    database: &Arc<Database>,
    root_id: i64,
    path: &str,
    parsed: &LibraryName,
    candidate: &CandidateFile,
    at: jiff::Timestamp,
) -> Result<IndexReport, IndexError> {
    let mut report = IndexReport::default();

    let Some(title_name) = parsed.title() else {
        // `parse` guarantees a title on `Ok`, so this is a shape guard rather than a case.
        park(
            database,
            path,
            candidate,
            parsed,
            Some("the name states no title".to_owned()),
            at,
        )
        .await?;
        report.unmatched += 1;
        return Ok(report);
    };

    let placed = movie_from_title(database, Some(root_id), path, title_name, candidate, at).await?;
    report.created_titles += placed.created_titles;
    report.created_items += placed.created_items;
    report.created_files += placed.created_files;
    report.under(placed.placed_title_id);
    Ok(report)
}

/// The title the catalogue already holds that a parked name most likely means.
///
/// Two steps, and the split is the architecture's: the statement reads the candidates a prefix could
/// name, and the *rule* over them is [`peregrine_domain::library::suggest_title`]. A read that
/// returned nothing is not a failure to suggest — it is the answer "nothing in the catalogue is close
/// enough", which is what most parked files get.
async fn suggested_title(
    database: &Arc<Database>,
    parsed_title: Option<&str>,
) -> Result<Option<i64>, IndexError> {
    let Some(parsed_title) = parsed_title else {
        return Ok(None);
    };
    let needs = sort_name_of(parsed_title);
    let lower: String = needs
        .chars()
        .take(peregrine_domain::library::MINIMUM_SUGGESTION_LENGTH)
        .collect();
    if lower.len() < peregrine_domain::library::MINIMUM_SUGGESTION_LENGTH {
        return Ok(None);
    }

    let candidates = database.title_candidates_between(&lower, &needs).await?;
    Ok(peregrine_domain::library::suggest_title(&needs, &candidates).map(|found| found.id))
}

/// Places a file under the title a user's rule names, when a rule exists for its pattern.
///
/// The placement is a film, and that is a property of where this is called rather than an assumption:
/// every episode-shaped refusal — a range, an absolute number, a bare season — is decided in
/// [`index_parsed`] before a rule is ever consulted, because a rule settles *which title* a name means
/// and never where in it the file sits. The migration that creates `match_rule` states the same thing
/// at the schema.
async fn rule_placement(
    database: &Arc<Database>,
    root_id: i64,
    path: &str,
    parsed: &LibraryName,
    candidate: &CandidateFile,
    at: jiff::Timestamp,
) -> Result<Option<IndexReport>, IndexError> {
    let Some(title_name) = parsed.title() else {
        return Ok(None);
    };
    let Some(remembered) = database.rule_title_for(&sort_name_of(title_name)).await? else {
        return Ok(None);
    };
    // The name the rule answers with, which is the name the user's correction was made under.
    Ok(Some(
        movie_from_title(database, Some(root_id), path, &remembered, candidate, at).await?,
    ))
}

/// Places one film from the title name a parse read, whatever read it.
///
/// The shared half of a scanned film and a hand-matched one, and the reason it takes a name rather
/// than a `LibraryName`: a parked row exists because the parse *failed*, so what it can be placed as
/// is the title the parse managed to read before it stopped.
async fn movie_from_title(
    database: &Arc<Database>,
    root_id: Option<i64>,
    path: &str,
    title_name: &str,
    candidate: &CandidateFile,
    at: jiff::Timestamp,
) -> Result<IndexReport, IndexError> {
    let mut report = IndexReport::default();
    let sort_name = sort_name_of(title_name);

    let (item_id, created) = movie_item(database, title_name, &sort_name, at).await?;
    report.under(created.title_id);
    report.created_titles += created.titles;
    report.created_items += created.items;
    report.under(created.title_id);

    write_file(database, root_id, path, item_id, candidate, at).await?;
    report.created_files += 1;
    Ok(report)
}

/// Where a name says an episode sits: the show, the season, and the number.
///
/// One value rather than three parameters because the three arrive together from one parse and mean
/// nothing apart: a season number with no title is not a placement, and passing them separately
/// invites a call that swaps two numbers the compiler cannot tell apart.
#[derive(Debug, Clone, Copy)]
struct Placement<'a> {
    /// The show's name, as the file stated it.
    title_name: &'a str,
    /// The season number the name stated.
    season_number: u32,
    /// The episode number the name stated.
    episode_number: u32,
}

/// Indexes one episode: its show's title, the season, the episode, the item, and the file.
///
/// The order matters and is the schema's: an episode's row needs its season's id, and its item needs
/// the episode's, so each step is the next one's input. Every step is a lookup before an insert,
/// which is what makes a second pass over the same folder create nothing.
async fn episode(
    database: &Arc<Database>,
    root_id: Option<i64>,
    path: &str,
    placement: Placement<'_>,
    candidate: &CandidateFile,
    at: jiff::Timestamp,
) -> Result<IndexReport, IndexError> {
    let Placement {
        title_name,
        season_number,
        episode_number,
    } = placement;
    let mut report = IndexReport::default();
    let sort_name = sort_name_of(title_name);

    let title_id = if let Some(id) = database
        .title_id_for_sort_name(TitleKind::Show.as_str(), &sort_name)
        .await?
    {
        id
    } else {
        let id = database
            .insert_stub_title(StubTitle {
                kind: TitleKind::Show,
                subtype: TitleSubtype::Standard,
                name: title_name.to_owned(),
                original_name: None,
                sort_name: sort_name.clone(),
                at: at.as_millisecond(),
            })
            .await?;
        report.created_titles += 1;
        id
    };

    let season_id = if let Some(id) = database.season_id_for(title_id, season_number).await? {
        id
    } else {
        let id = database
            .insert_stub_season(peregrine_storage::StubSeason {
                title_id,
                season_number,
                at: at.as_millisecond(),
            })
            .await?;
        report.created_seasons += 1;
        id
    };

    let episode_id = if let Some(id) = database
        .episode_id_for(title_id, season_number, episode_number)
        .await?
    {
        id
    } else {
        let id = database
            .insert_stub_episode(peregrine_storage::StubEpisode {
                title_id,
                season_id,
                season_number,
                episode_number,
                at: at.as_millisecond(),
            })
            .await?;
        report.created_episodes += 1;
        id
    };

    let item_id = if let Some(id) = database.item_id_for(title_id, Some(episode_id)).await? {
        id
    } else {
        let id = database
            .insert_media_item(MediaItem {
                id: 0,
                title_id,
                episode_id: Some(episode_id),
                state: MediaItemState::Available,
                origin: MediaOrigin::Scan,
                preferred_file_id: None,
                quality_label: None,
                created_at: at,
                updated_at: at,
            })
            .await?;
        report.created_items += 1;
        id
    };

    write_file(database, root_id, path, item_id, candidate, at).await?;
    report.created_files += 1;
    report.under(title_id);
    Ok(report)
}

/// Writes one file row, which is the same row for a film and for an episode.
async fn write_file(
    database: &Arc<Database>,
    root_id: Option<i64>,
    path: &str,
    item_id: i64,
    candidate: &CandidateFile,
    at: jiff::Timestamp,
) -> Result<(), IndexError> {
    database
        .insert_media_file(MediaFile {
            id: 0,
            media_item_id: item_id,
            path: text(path)?,
            root_id,
            size_bytes: candidate.size_bytes,
            // The file's own modification time, not the scan's instant: the incremental rule
            // compares what the library recorded against what the filesystem now says, and storing
            // the moment of the walk would make every file look changed on the next pass.
            mtime_ms: jiff::Timestamp::from_millisecond(candidate.mtime_ms).unwrap_or(at),
            added_at: at,
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
            updated_at: at,
        })
        .await?;
    Ok(())
}

/// What resolving a title and its item created.
#[derive(Debug, Clone, Copy, Default)]
struct Resolved {
    titles: usize,
    items: usize,
    /// The title the item belongs to, which a rule created from this placement points at.
    title_id: i64,
}

/// The item a parsed movie belongs to, creating the title and the item when they are missing.
///
/// The title is found by its sort name, which is what makes a second copy of one film one title
/// rather than two, and the item by its title and episode — `None`, because a film's item has no
/// episode and the partial unique index (I2) refuses a second one.
async fn movie_item(
    database: &Arc<Database>,
    title_name: &str,
    sort_name: &str,
    at: jiff::Timestamp,
) -> Result<(i64, Resolved), IndexError> {
    let mut created = Resolved::default();

    let title_id = if let Some(id) = database
        .title_id_for_sort_name(TitleKind::Movie.as_str(), sort_name)
        .await?
    {
        id
    } else {
        let id = database
            .insert_stub_title(StubTitle {
                kind: TitleKind::Movie,
                subtype: TitleSubtype::Standard,
                name: title_name.to_owned(),
                original_name: None,
                sort_name: sort_name.to_owned(),
                at: at.as_millisecond(),
            })
            .await?;
        created.titles += 1;
        id
    };

    created.title_id = title_id;

    let item_id = if let Some(id) = database.item_id_for(title_id, None).await? {
        id
    } else {
        let id = database
            .insert_media_item(MediaItem {
                id: 0,
                title_id,
                episode_id: None,
                state: MediaItemState::Available,
                origin: MediaOrigin::Scan,
                preferred_file_id: None,
                quality_label: None,
                created_at: at,
                updated_at: at,
            })
            .await?;
        created.items += 1;
        id
    };

    Ok((item_id, created))
}

/// Parks one candidate in `unmatched_file`, with whatever the parse read.
async fn park(
    database: &Arc<Database>,
    path: &str,
    candidate: &CandidateFile,
    parsed: &LibraryName,
    reason: Option<String>,
    at: jiff::Timestamp,
) -> Result<(), IndexError> {
    let stamp = jiff::Timestamp::from_millisecond(candidate.mtime_ms).unwrap_or(at);

    // A title the catalogue already holds is offered to the person who answers the row: the parse
    // read a name that could not be placed, and the catalogue may hold what it meant. The rule is
    // `peregrine_domain`'s — narrow, and deliberately a suggestion rather than a placement.
    let suggested = suggested_title(database, parsed.title()).await?;
    database
        .insert_unmatched_file(
            UnmatchedFile {
                id: 0,
                path: text(path)?,
                size_bytes: candidate.size_bytes,
                mtime_ms: stamp,
                parsed_title: parsed.title().map(str::to_owned),
                parsed_year: parsed.year(),
                parsed_season: parsed.season().map(peregrine_domain::SeasonNumber::new),
                parsed_episode: parsed
                    .episodes()
                    .map(|span| peregrine_domain::EpisodeNumber::new(span.first())),
                confidence: None,
                reason,
                suggested_title_id: suggested,
                suggested_episode_id: None,
                first_seen_at: at,
            },
            true,
        )
        .await?;
    Ok(())
}

/// The sort name a title is looked up and stored under.
///
/// Lower-cased and whitespace-collapsed, which is what makes `Alien` and `alien` one title. The full
/// normalization `PG-E14-05` owns — articles, punctuation, accents, and the `The Matrix`/`Matrix,
/// The` question — is not guessed at here: what this needs is a stable key, and a key that changes
/// when that task lands is a key that would have to be applied to rows written under the old one.
fn sort_name_of(title: &str) -> String {
    // The domain's, not a second copy: the catalogue stores a title's `sort_name` with this rule, the
    // matching engine compares provider names with it (`PG-E5-13`), and two implementations of
    // "what counts as the same name" is how a title ends up filed where a search cannot find it.
    // The body was moved unchanged, and the indexer's tests are what say so.
    peregrine_domain::matching::sort_name(title)
}

/// Wraps a path as the non-empty text the schema stores.
fn text(value: &str) -> Result<NonEmptyText, IndexError> {
    NonEmptyText::new(value).map_err(|_| IndexError::NotText {
        path: std::path::PathBuf::from(value),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use peregrine_domain::{AccountId, AccountScope, LibraryRoot, LibraryRootKind, ScanMode};
    use peregrine_storage::{MetadataState, PageRequest};
    use tempfile::TempDir;

    use peregrine_domain::ScanPolicy;

    use crate::library::scan::Scanner;

    use super::*;

    const NOW: i64 = 1_700_000_000_000;

    fn at(millis: i64) -> jiff::Timestamp {
        jiff::Timestamp::from_millisecond(millis).expect("the tests stay inside jiff's range")
    }

    /// A temp directory holding the files a test names, found by walking it.
    struct Corpus {
        directory: TempDir,
        database: Arc<Database>,
        root_id: i64,
    }

    async fn corpus(name: &str, files: &[&str]) -> Corpus {
        let directory = TempDir::new().unwrap();
        let media = directory.path().join(format!("Movies-{name}"));
        fs::create_dir_all(&media).unwrap();
        for relative in files {
            let path = media.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, b"a film, in bytes").unwrap();
        }
        let database = Arc::new(
            Database::open(directory.path().join("peregrine.sqlite"))
                .await
                .unwrap(),
        );
        let root_id = database
            .insert_library_root(LibraryRoot {
                id: 0,
                path: NonEmptyText::new(media.to_str().unwrap()).unwrap(),
                kind: LibraryRootKind::Movies,
                scan_mode: ScanMode::Incremental,
                last_scan_at: None,
                created_at: at(NOW),
            })
            .await
            .unwrap();
        Corpus {
            directory,
            database,
            root_id,
        }
    }

    impl Corpus {
        fn scan(&self) -> Vec<CandidateFile> {
            let root = self.database_root_path();
            Scanner::new(ScanPolicy::new())
                .scan(&root)
                .unwrap()
                .candidates
        }

        fn database_root_path(&self) -> PathBuf {
            self.directory.path().join(
                self.directory
                    .path()
                    .read_dir()
                    .unwrap()
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .find(|name| name.starts_with("Movies-"))
                    .expect("the media directory exists"),
            )
        }

        async fn index(&self) -> IndexReport {
            index_candidates(&self.database, self.root_id, &self.scan(), at(NOW + 1_000))
                .await
                .unwrap()
        }

        async fn titles(&self) -> Vec<String> {
            self.database
                .list_titles(
                    AccountScope::unrestricted(AccountId::new(1).expect("account one")),
                    PageRequest::first(50),
                )
                .await
                .unwrap()
                .into_iter()
                .map(|row| row.name.as_str().to_owned())
                .collect()
        }

        /// Writes more files into the root, for a second pass over the same folder.
        fn add(&self, names: &[&str]) {
            for name in names {
                let path = self.database_root_path().join(name);
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).unwrap();
                }
                fs::write(&path, b"a film, in bytes").unwrap();
            }
        }

        /// Every episode the corpus placed, as `(season, episode)` pairs.
        async fn episodes(&self) -> Vec<(u32, u32)> {
            self.database
                .list_title_episodes(
                    AccountScope::unrestricted(AccountId::new(1).expect("account one")),
                    self.only_title().await,
                    50,
                )
                .await
                .unwrap()
                .into_iter()
                .map(|row| (row.season_number.get(), row.episode_number.get()))
                .collect()
        }

        /// The id of the one title the corpus holds under `name`.
        async fn only_title_named(&self, name: &str) -> i64 {
            self.database
                .list_titles(
                    AccountScope::unrestricted(AccountId::new(1).expect("account one")),
                    PageRequest::first(50),
                )
                .await
                .unwrap()
                .into_iter()
                .find(|row| row.name.as_str() == name)
                .expect("the corpus holds that title")
                .id
        }

        /// The one title the corpus holds, for a test that needs its id.
        async fn only_title(&self) -> i64 {
            self.database
                .list_titles(
                    AccountScope::unrestricted(AccountId::new(1).expect("account one")),
                    PageRequest::first(10),
                )
                .await
                .unwrap()
                .first()
                .expect("the corpus holds one title")
                .id
        }

        /// The parked rows themselves, for a test that answers one.
        async fn unmatched_rows(&self) -> Vec<peregrine_storage::UnmatchedFileRow> {
            self.database
                .list_unmatched_files(
                    AccountScope::unrestricted(AccountId::new(1).expect("account one")),
                    PageRequest::first(50),
                )
                .await
                .unwrap()
        }

        async fn unmatched(&self) -> Vec<String> {
            self.database
                .list_unmatched_files(
                    AccountScope::unrestricted(AccountId::new(1).expect("account one")),
                    PageRequest::first(50),
                )
                .await
                .unwrap()
                .into_iter()
                .map(|row| row.file.path.to_string())
                .collect()
        }
    }

    #[tokio::test]
    async fn a_film_becomes_a_title_an_item_and_a_file() {
        let corpus = corpus("film", &["Alien (1979).mkv"]).await;

        let report = corpus.index().await;

        assert_eq!(report.created_titles, 1, "{report:?}");
        assert_eq!(report.created_items, 1);
        assert_eq!(report.created_files, 1);
        assert_eq!(report.unmatched, 0);
        assert_eq!(corpus.titles().await, vec!["Alien"]);
    }

    #[tokio::test]
    async fn a_title_the_indexer_creates_is_a_stub_rather_than_a_complete_one() {
        // `PG-E5-06`: a stub is the identity a provider sync hangs its data off, and a title the
        // library invented from a filename must not claim to be complete — a screen that showed it
        // as complete would promise artwork and a synopsis that no sync has fetched.
        let corpus = corpus("stub", &["Alien (1979).mkv"]).await;
        corpus.index().await;

        let titles = corpus
            .database
            .list_titles(
                AccountScope::unrestricted(AccountId::new(1).expect("account one")),
                PageRequest::first(10),
            )
            .await
            .unwrap();
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].metadata_state, MetadataState::Stub);
    }

    #[tokio::test]
    async fn the_file_keeps_the_mtime_the_filesystem_gave_it() {
        // Not the scan's instant: the incremental rule compares what the library recorded against
        // what the filesystem now says, and a stored scan instant would make every file look
        // changed on the next pass.
        let corpus = corpus("stamp", &["Alien (1979).mkv"]).await;
        corpus.index().await;

        let candidate = corpus.scan().into_iter().next().unwrap();
        let row = corpus
            .database
            .recorded_file_at_path(candidate.path.to_str().unwrap())
            .await
            .unwrap()
            .expect("the file was indexed");

        assert_eq!(row.mtime_ms, candidate.mtime_ms);
        assert_eq!(row.size_bytes, i64::try_from(candidate.size_bytes).unwrap());
    }

    #[tokio::test]
    async fn a_second_pass_creates_nothing_and_refreshes_what_it_finds() {
        // The indexer is run over the same candidates after any interruption, so a second pass over
        // an unchanged folder must be a no-op that reports the refresh rather than a second copy of
        // everything.
        let corpus = corpus("idempotent", &["Alien (1979).mkv"]).await;
        corpus.index().await;

        let second = corpus.index().await;

        assert_eq!(second.created_titles, 0, "{second:?}");
        assert_eq!(second.created_items, 0);
        assert_eq!(second.created_files, 0);
        assert_eq!(second.refreshed_files, 1);
        assert!(second.is_unchanged());
        assert_eq!(corpus.titles().await.len(), 1);
    }

    #[tokio::test]
    async fn two_files_of_one_film_share_a_title_and_have_an_item_each() {
        // A 1080p and a 2160p copy: one title, two items, two files. The title lookup is what makes
        // that true, and a lookup that missed would have produced two titles for one film.
        let corpus = corpus(
            "two-copies",
            &["Alien (1979).mkv", "Alien (1979) 2160p.mkv"],
        )
        .await;

        let report = corpus.index().await;

        assert_eq!(report.created_titles, 1, "{report:?}");
        assert_eq!(report.created_files, 2);
        assert_eq!(corpus.titles().await, vec!["Alien"]);
    }

    /// What the indexer does with one parsed name: the outcome a person would see.
    ///
    /// The table is `PG-E14-07`'s acceptance in miniature — "a fixture table of ambiguous cases with
    /// expected results" — and it measures the *placement* rather than the parse, which is the half the
    /// parser's own 200-name corpus does not cover. Every row is a name a real library contains, and
    /// every expectation was written from the naming conventions `peregrine-domain`'s `name.rs`
    /// documents rather than read back out of the indexer.
    #[derive(Debug, PartialEq, Eq)]
    enum Placement {
        /// Placed as this season and episode.
        Placed(u32, u32),
        /// Placed as a *film*, which is the indexer treating an episode-shaped name as a feature.
        ///
        /// In the table because it happens, and because a row that says so is how a wrong answer gets
        /// written down rather than quietly asserted away: the version-suffixed row in the table is
        /// what this variant exists for, and that row is expected to change when the parser learns the
        /// shape. A table that could not state the wrong outcome would have to either hide it or fail.
        PlacedAsFilm,
        /// Placed, or parked with the reason a person would read. The expected side holds a
        /// substring rather than the whole sentence, because two layers may refuse a name — the
        /// parser, with the specific thing it could not read, and the indexer, with the task that owns
        /// the decision — and a table that demanded one of them would be asserting an implementation
        /// detail rather than a behaviour.
        Parked(&'static str),
    }

    /// One row of the table.
    struct EpisodeCase {
        name: &'static str,
        expected: Placement,
        /// What the row is testing, read by a person when it fails.
        because: &'static str,
    }

    const EPISODE_CASES: &[EpisodeCase] = &[
        EpisodeCase {
            name: "Breaking Bad S01E01.mkv",
            expected: Placement::Placed(1, 1),
            because: "the unambiguous form everything else is measured against",
        },
        EpisodeCase {
            name: "breaking bad s1e2.mkv",
            expected: Placement::Placed(1, 2),
            because: "case does not change what a name says",
        },
        EpisodeCase {
            name: "Breaking Bad S01.E02.mkv",
            expected: Placement::Placed(1, 2),
            because: "a separator between season and episode is not part of either",
        },
        EpisodeCase {
            name: "Breaking Bad 1x02.mkv",
            expected: Placement::Placed(1, 2),
            because: "the `1x02` form states the same two numbers",
        },
        EpisodeCase {
            name: "Breaking Bad 01x02 - Cat's in the Bag.mkv",
            expected: Placement::Placed(1, 2),
            because: "the episode's own title follows the numbers",
        },
        EpisodeCase {
            name: "Breaking Bad (2008) S01E01.mkv",
            expected: Placement::Placed(1, 1),
            because: "the year belongs to the show, not to the episode",
        },
        EpisodeCase {
            name: "[Group] Breaking Bad - S01E01 [1080p].mkv",
            expected: Placement::Placed(1, 1),
            because: "a release group and a resolution surround the name rather than changing it",
        },
        EpisodeCase {
            name: "Breaking Bad S00E01.mkv",
            expected: Placement::Placed(0, 1),
            because: "season zero is where a show's specials live, and the name says so",
        },
        EpisodeCase {
            name: "Breaking Bad S01E01E02.mkv",
            expected: Placement::Parked("PG-E14-07"),
            because: "one file holding two episodes is a modelling question, not a guess",
        },
        EpisodeCase {
            name: "Breaking Bad S01E01-E02.mkv",
            expected: Placement::Parked("PG-E14-07"),
            because: "the same range written with a hyphen",
        },
        EpisodeCase {
            name: "Breaking Bad S01E01-02.mkv",
            expected: Placement::Parked("PG-E14-07"),
            because: "and with the season stated once",
        },
        EpisodeCase {
            name: "Breaking Bad 1x01-1x02.mkv",
            expected: Placement::Parked("both episode"),
            because: "a range in the other form, which the parser names as such rather than leaving \
                      to the indexer",
        },
        EpisodeCase {
            name: "Breaking Bad 05.mkv",
            expected: Placement::Parked("could be an absolute episode"),
            because: "a bare number is an absolute episode, a year, or a volume, and the parser \
                      refuses it for exactly that reason",
        },
        EpisodeCase {
            name: "Breaking Bad - 05.mkv",
            expected: Placement::Parked("PG-E14-07"),
            because: "a dash does not turn a bare number into a season and episode",
        },
        EpisodeCase {
            name: "Breaking Bad Season 1.mkv",
            expected: Placement::Parked("PG-E14-07"),
            because: "a season pack says which season and not which episode",
        },
        EpisodeCase {
            name: "Breaking Bad S01.mkv",
            expected: Placement::Parked("PG-E14-07"),
            because: "the abbreviated form of the same thing",
        },
        EpisodeCase {
            name: "Breaking Bad E01.mkv",
            expected: Placement::Parked("PG-E14-07"),
            because: "an episode number with no season is not a placement",
        },
        EpisodeCase {
            name: "Breaking Bad s1e1.mkv",
            expected: Placement::Placed(1, 1),
            because: "single digits are the same numbers",
        },
        EpisodeCase {
            name: "Breaking Bad Season 01 Episode 02.mkv",
            expected: Placement::Placed(1, 2),
            because: "the written-out form states both numbers",
        },
        EpisodeCase {
            name: "Breaking Bad [S01E01].mkv",
            expected: Placement::Placed(1, 1),
            because: "brackets around the numbers are punctuation",
        },
        EpisodeCase {
            name: "Breaking Bad S01E01v2.mkv",
            // **This is the table's first finding, and it is wrong.** The name is episode one of
            // season one with a version suffix, and the indexer files it as a *film* because the
            // parser's episode pattern does not accept a suffix glued to the episode number. The row
            // records what happens today so the fix has somewhere to land — when the parser learns the
            // shape, this row fails and the expectation becomes `Placed(1, 1)`. The parser is
            // `peregrine-domain`'s `name.rs`, which is PG-E4-04's.
            expected: Placement::PlacedAsFilm,
            because: "a version suffix is being read as part of the episode number, which the finding \
                      in PG-E14-07's note records against PG-E4-04",
        },
        EpisodeCase {
            name: "Breaking Bad S01E01 日本語.mkv",
            expected: Placement::Placed(1, 1),
            because: "a non-ASCII suffix is not a number and does not change the placement",
        },
        EpisodeCase {
            name: "Breaking Bad S01E00.mkv",
            // Recorded rather than asserted: an episode zero inside a season is a prologue, a special,
            // or a numbering mistake, and the naming conventions do not settle it. What the table
            // states is the placement a person would see today.
            expected: Placement::Placed(1, 0),
            because: "the name states a season and an episode, and the core places what a name states",
        },
    ];

    #[tokio::test]
    async fn the_placement_table_says_what_each_ambiguous_name_becomes() {
        // Every row is run and every disagreement is reported, so one run tells a reader the whole
        // state of the table rather than the first thing wrong with it.
        let mut failures: Vec<String> = Vec::new();

        for case in EPISODE_CASES {
            let corpus = corpus("placement-table", &[case.name]).await;
            let report = corpus.index().await;
            let actual = if report.created_episodes == 0
                && report.unmatched == 0
                && report.created_files == 1
            {
                Placement::PlacedAsFilm
            } else if report.created_episodes == 1 {
                let rows = corpus.unmatched_rows().await;
                if rows.is_empty() {
                    let episodes = corpus.episodes().await;
                    Placement::Placed(episodes[0].0, episodes[0].1)
                } else {
                    Placement::Parked("placed and parked at once")
                }
            } else if report.unmatched == 1 {
                let reason = corpus
                    .unmatched_rows()
                    .await
                    .first()
                    .and_then(|row| row.file.reason.clone())
                    .unwrap_or_default();
                Placement::Parked(Box::leak(reason.into_boxed_str()))
            } else {
                Placement::Parked("neither placed nor parked")
            };

            let agrees = match (&actual, &case.expected) {
                (Placement::Placed(a, b), Placement::Placed(c, d)) => a == c && b == d,
                (Placement::PlacedAsFilm, Placement::PlacedAsFilm) => true,
                (Placement::Parked(actual_reason), Placement::Parked(wanted)) => {
                    wanted.is_empty() || actual_reason.contains(wanted)
                }
                _ => false,
            };
            if !agrees {
                failures.push(format!(
                    "{}: expected {:?} ({}) and got {actual:?}",
                    case.name, case.expected, case.because
                ));
            }
        }

        assert!(
            failures.is_empty(),
            "{} of {} names are not placed the way the conventions say:\n{}",
            failures.len(),
            EPISODE_CASES.len(),
            failures.join("\n")
        );
    }

    #[tokio::test]
    async fn a_single_episode_is_placed_in_its_season() {
        // The unambiguous case, and the one that turns a show from one title with sixty parked
        // files into episodes: a name that states a season *and* exactly one episode. It creates the
        // show, the season, the episode, the item, and the file, in that order, because each row's
        // id is the next one's foreign key.
        let corpus = corpus("episode", &["Breaking Bad S01E01.mkv"]).await;

        let report = corpus.index().await;

        assert_eq!(report.created_titles, 1, "{report:?}");
        assert_eq!(report.created_seasons, 1, "{report:?}");
        assert_eq!(report.created_episodes, 1, "{report:?}");
        assert_eq!(report.created_items, 1, "{report:?}");
        assert_eq!(report.created_files, 1, "{report:?}");
        assert_eq!(report.unmatched, 0, "{report:?}");
        assert_eq!(corpus.titles().await.len(), 1);
        let title_id = corpus.only_title().await;
        assert!(
            corpus
                .database
                .episode_id_for(title_id, 1, 1)
                .await
                .unwrap()
                .is_some(),
            "the episode is findable by the numbers its name stated"
        );
    }

    #[tokio::test]
    async fn two_episodes_of_one_show_share_a_season_and_have_an_item_each() {
        let corpus = corpus(
            "two-episodes",
            &["Breaking Bad S01E01.mkv", "Breaking Bad S01E02.mkv"],
        )
        .await;

        let report = corpus.index().await;

        assert_eq!(report.created_titles, 1, "{report:?}");
        assert_eq!(report.created_seasons, 1, "{report:?}");
        assert_eq!(report.created_episodes, 2, "{report:?}");
        assert_eq!(report.created_items, 2, "{report:?}");
        assert_eq!(report.created_files, 2, "{report:?}");
    }

    #[tokio::test]
    async fn a_multi_episode_file_is_parked_rather_than_placed() {
        // `S01E01E02` is one file holding two episodes or a naming mistake, and which one it is needs
        // the numbering rules PG-E14-07 owns. Parking loses nothing: the parse is stored beside the
        // path, so the queue has everything a human needs to decide.
        let corpus = corpus("range", &["Breaking Bad S01E01E02.mkv"]).await;

        let report = corpus.index().await;

        assert_eq!(report.unmatched, 1, "{report:?}");
        assert_eq!(report.created_episodes, 0, "{report:?}");
        assert_eq!(corpus.titles().await, Vec::<String>::new());
        assert_eq!(corpus.unmatched().await.len(), 1);
    }

    #[tokio::test]
    async fn a_season_with_no_episode_is_parked_rather_than_guessed_at() {
        // A name that states a season and no episode is a season pack or a folder, and placing it
        // would mean inventing which episode it is.
        let corpus = corpus("season-only", &["Breaking Bad Season 1.mkv"]).await;

        let report = corpus.index().await;

        assert_eq!(report.unmatched, 1, "{report:?}");
        assert_eq!(report.created_episodes, 0, "{report:?}");
    }

    #[tokio::test]
    async fn a_second_pass_over_episodes_creates_nothing() {
        let corpus = corpus(
            "episodes-idempotent",
            &["Breaking Bad S01E01.mkv", "Breaking Bad S02E01.mkv"],
        )
        .await;
        corpus.index().await;

        let second = corpus.index().await;

        assert!(second.is_unchanged(), "{second:?}");
        assert_eq!(second.refreshed_files, 2, "{second:?}");
        assert_eq!(second.created_episodes, 0, "{second:?}");
        assert_eq!(second.created_seasons, 0, "{second:?}");
    }

    #[tokio::test]
    async fn a_name_that_cannot_be_parsed_is_parked_with_its_reason() {
        let corpus = corpus("unparsed", &["1080p.mkv"]).await;

        let report = corpus.index().await;

        assert_eq!(report.unmatched, 1, "{report:?}");
        assert_eq!(corpus.unmatched().await.len(), 1);
    }

    #[tokio::test]
    async fn a_parked_name_suggests_a_title_the_library_already_holds() {
        // The suggestion is what makes the queue answerable without a picker: the film is already
        // here under the name without the year, and the row says so. It is recorded at park time, so
        // the read that builds the queue does not have to match anything.
        let corpus = corpus("suggest", &["Alien (1979).mkv", "Alien 2 1979.mkv"]).await;

        let report = corpus.index().await;

        assert_eq!(
            report.created_titles, 1,
            "the clean name is placed: {report:?}"
        );
        assert_eq!(
            report.unmatched, 1,
            "the ambiguous one is parked: {report:?}"
        );
        let rows = corpus.unmatched_rows().await;
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].file.parsed_title.as_deref(),
            Some("Alien 2"),
            "what the parse read, which is what a suggestion is built from"
        );
        let suggested = rows[0].file.suggested_title_id;
        let placed = corpus.only_title().await;
        assert_eq!(
            suggested,
            Some(placed),
            "the row offers the title the library already holds"
        );
    }

    #[tokio::test]
    async fn answering_with_the_suggestion_files_the_file_under_the_existing_title() {
        // Why the suggestion is worth recording: answering with it *joins* a title rather than
        // creating a near-duplicate of it. The parse's reading would have made a second title called
        // `Alien 2049` beside `Alien`, which is the mistake the queue exists to prevent.
        let corpus = corpus("suggest-match", &["Alien (1979).mkv", "Alien 2 1979.mkv"]).await;
        corpus.index().await;
        let id = corpus.unmatched_rows().await[0].id;

        match_unmatched(
            &corpus.database,
            AccountScope::unrestricted(AccountId::new(1).expect("account one")),
            id,
            MatchAs::Suggested,
            at(NOW + 1_000),
        )
        .await
        .unwrap();

        assert_eq!(
            corpus.titles().await,
            vec!["Alien".to_owned()],
            "one title, not two"
        );
        assert_eq!(corpus.unmatched_rows().await.len(), 0);
    }

    #[tokio::test]
    async fn answering_with_the_parse_reading_still_works_when_nothing_was_suggested() {
        // A suggestion can be absent for two reasons — nothing close enough, or the offered title
        // filtered from view — and neither is a failure. The answer falls back to what the parse read
        // rather than refusing the row.
        let corpus = corpus("no-suggestion", &["The Matrix 2049.mkv"]).await;
        corpus.index().await;
        let rows = corpus.unmatched_rows().await;
        assert_eq!(rows[0].file.suggested_title_id, None, "nothing is close");

        match_unmatched(
            &corpus.database,
            AccountScope::unrestricted(AccountId::new(1).expect("account one")),
            rows[0].id,
            MatchAs::Suggested,
            at(NOW + 1_000),
        )
        .await
        .unwrap();

        assert_eq!(corpus.titles().await, vec!["The Matrix".to_owned()]);
    }

    #[tokio::test]
    async fn matching_a_parked_file_places_it_as_the_title_its_parse_read() {
        // PG-E14-08's first half. The row exists because the parse failed, so what it is placed as is
        // the title the parse managed to read: `The Matrix 2049` is ambiguous about which number is
        // the year, and a person saying "it is the title" is the answer.
        let corpus = corpus("match", &["The Matrix 2049.mkv"]).await;
        let parked = corpus.index().await;
        assert_eq!(parked.unmatched, 1, "the fixture must park: {parked:?}");
        let rows = corpus.unmatched_rows().await;
        assert_eq!(rows.len(), 1);
        let (id, parsed_title) = (rows[0].id, rows[0].file.parsed_title.clone());
        let scope = AccountScope::unrestricted(AccountId::new(1).expect("account one"));

        match_unmatched(
            &corpus.database,
            scope,
            id,
            MatchAs::Parsed,
            at(NOW + 1_000),
        )
        .await
        .unwrap();

        assert_eq!(
            corpus.titles().await,
            vec![parsed_title.expect("the fixture's parse read a title")],
            "the file became the title the parse read"
        );
        assert_eq!(
            corpus.unmatched_rows().await.len(),
            0,
            "and it is no longer waiting for an answer"
        );
    }

    #[tokio::test]
    async fn a_second_occurrence_of_a_corrected_name_is_placed_without_asking() {
        // PG-E14-09's acceptance. `The Matrix 2049.mkv` is ambiguous about which number is the year,
        // so it parks; a person says "the title is The Matrix"; and the *next* name that reads the
        // same way is placed by that answer rather than parked beside it.
        let corpus = corpus("remember", &["The Matrix 2049.mkv"]).await;
        corpus.index().await;
        let rows = corpus.unmatched_rows().await;
        let id = rows[0].id;
        let read = rows[0].file.parsed_title.clone();
        let scope = AccountScope::unrestricted(AccountId::new(1).expect("account one"));
        match_unmatched(
            &corpus.database,
            scope,
            id,
            MatchAs::Parsed,
            at(NOW + 1_000),
        )
        .await
        .unwrap();
        // The rule is recorded under the title the parse read, which is what a later name is
        // matched by; if these two ever diverge the rule is written where nothing looks.
        assert_eq!(
            corpus
                .database
                .rule_title_for(&sort_name_of(read.as_deref().unwrap_or_default()))
                .await
                .unwrap(),
            read,
            "the rule is recorded under the parse's own reading"
        );

        // A second file the parser refuses for the same reason, dropped into the root and walked.
        corpus.add(&["Disc 2/The Matrix 2049.mkv"]);
        let second = corpus.index().await;

        assert_eq!(
            second.unmatched, 0,
            "the second name is not asked about: {second:?}"
        );
        assert_eq!(second.created_files, 1, "{second:?}");
        assert_eq!(corpus.titles().await.len(), 1, "and it is the same title");
    }

    #[tokio::test]
    async fn a_rule_never_settles_a_numbering_question() {
        // A rule says which title a name means and nothing about where in it the file sits, so an
        // episode-shaped refusal is decided first: a folder full of `S01E01E02` ranges stays parked
        // even when a rule exists for the show's name.
        let corpus = corpus("rule-not-numbering", &["Breaking Bad S01E01E02.mkv"]).await;
        corpus.index().await;
        let id = corpus.unmatched_rows().await[0].id;
        match_unmatched(
            &corpus.database,
            AccountScope::unrestricted(AccountId::new(1).expect("account one")),
            id,
            MatchAs::Parsed,
            at(NOW + 1_000),
        )
        .await
        .unwrap();
        corpus.add(&["Breaking Bad S02E03E04.mkv", "Breaking Bad Season 3.mkv"]);

        let second = corpus.index().await;

        assert_eq!(second.unmatched, 2, "both are still questions: {second:?}");
        assert_eq!(second.created_seasons, 0, "{second:?}");
    }

    #[tokio::test]
    async fn answering_with_a_chosen_title_files_the_file_under_it() {
        // The third answer, and the only one a person supplies outright: a name whose parse and
        // suggestion are both wrong is placed under the title a search found. The chosen id resolves
        // to the title's *name* first, so a chosen title takes the same path a parsed one does.
        let corpus = corpus("chosen", &["The Matrix 2049.mkv", "Alien (1979).mkv"]).await;
        corpus.index().await;
        let chosen = corpus.only_title_named("Alien").await;

        match_unmatched(
            &corpus.database,
            AccountScope::unrestricted(AccountId::new(1).expect("account one")),
            corpus.unmatched_rows().await[0].id,
            MatchAs::Title(chosen),
            at(NOW + 1_000),
        )
        .await
        .unwrap();

        assert_eq!(
            corpus.titles().await.len(),
            1,
            "the file joined the chosen title"
        );
        assert_eq!(corpus.unmatched_rows().await.len(), 0);
    }

    #[tokio::test]
    async fn answering_with_a_title_that_does_not_exist_falls_back_to_the_parse() {
        // A malformed request that still names a file places it the way its own name says rather than
        // refusing the row: the alternative is a file nobody can place because a client sent an id
        // that means nothing.
        let corpus = corpus("chosen-missing", &["The Matrix 2049.mkv"]).await;
        corpus.index().await;
        let id = corpus.unmatched_rows().await[0].id;

        match_unmatched(
            &corpus.database,
            AccountScope::unrestricted(AccountId::new(1).expect("account one")),
            id,
            MatchAs::Title(9_999),
            at(NOW + 1_000),
        )
        .await
        .unwrap();

        assert_eq!(corpus.titles().await, vec!["The Matrix".to_owned()]);
    }

    #[tokio::test]
    async fn matching_a_file_a_scan_already_reached_clears_the_row_and_writes_nothing() {
        // The stale-row case: a scan reached the file while the row waited. Writing again would
        // collide on the path's unique index, so the answer is to clear the row.
        let corpus = corpus("stale", &["Alien (1979).mkv"]).await;
        corpus.index().await;
        let row = corpus
            .database
            .insert_unmatched_file(
                peregrine_domain::UnmatchedFile {
                    id: 0,
                    path: peregrine_domain::NonEmptyText::new(
                        corpus
                            .database_root_path()
                            .join("Alien (1979).mkv")
                            .to_str()
                            .unwrap(),
                    )
                    .unwrap(),
                    size_bytes: 17,
                    mtime_ms: at(NOW),
                    parsed_title: Some("Alien".to_owned()),
                    parsed_year: Some(1979),
                    parsed_season: None,
                    parsed_episode: None,
                    confidence: None,
                    reason: Some("stale".to_owned()),
                    suggested_title_id: None,
                    suggested_episode_id: None,
                    first_seen_at: at(NOW),
                },
                true,
            )
            .await
            .unwrap()
            .expect("the row is new");

        let matched = match_unmatched(
            &corpus.database,
            AccountScope::unrestricted(AccountId::new(1).expect("account one")),
            row,
            MatchAs::Parsed,
            at(NOW + 1_000),
        )
        .await
        .unwrap();

        assert_eq!(matched, Matched::AlreadyIndexed);
        assert_eq!(corpus.unmatched_rows().await.len(), 0);
        assert_eq!(corpus.titles().await.len(), 1, "nothing was written twice");
    }

    #[tokio::test]
    async fn matching_a_row_whose_parse_read_no_title_leaves_it_alone() {
        // `1080p.mkv` reads a resolution and no title, and there is nothing to place it as. The row
        // stays so it can be answered by something that does know, which is PG-E14-09's memory.
        let corpus = corpus("no-title", &["1080p.mkv"]).await;
        corpus.index().await;
        let id = corpus.unmatched_rows().await[0].id;

        let matched = match_unmatched(
            &corpus.database,
            AccountScope::unrestricted(AccountId::new(1).expect("account one")),
            id,
            MatchAs::Parsed,
            at(NOW + 1_000),
        )
        .await
        .unwrap();

        assert_eq!(matched, Matched::NoTitle);
        assert_eq!(corpus.unmatched_rows().await.len(), 1, "the row stays");
    }

    #[tokio::test]
    async fn a_scan_of_a_root_with_no_files_indexes_nothing() {
        let corpus = corpus("empty", &[]).await;

        assert_eq!(corpus.index().await, IndexReport::default());
        assert_eq!(corpus.titles().await, Vec::<String>::new());
    }

    #[tokio::test]
    async fn the_indexer_records_a_stub_for_every_file_including_one_an_account_cannot_see() {
        // The scope decision, pinned from both sides: the scanner indexes an adult-flagged title's
        // files (because the row has to exist before I37 can hide it), and a restricted account
        // still cannot see them. A scanner that read under a restricted scope would leave those
        // files permanently unindexed, and no later change to a rating ceiling could repair it.
        let corpus = corpus("scoped", &["Alien (1979).mkv", "Adult Film (2019).mkv"]).await;
        corpus.index().await;

        let unrestricted = AccountScope::unrestricted(AccountId::new(1).expect("account one"));
        let restricted = AccountScope::restricted(AccountId::new(2).expect("account two"));
        let visible = |scope| {
            let database = Arc::clone(&corpus.database);
            async move {
                database
                    .list_titles(scope, PageRequest::first(50))
                    .await
                    .unwrap()
                    .len()
            }
        };
        // Both titles are stubs, so a restricted scope hides them for want of a certification
        // rather than for want of a row — which is the distinction this test exists for.
        assert_eq!(visible(unrestricted).await, 2);
        assert_eq!(visible(restricted).await, 0);
    }
}
