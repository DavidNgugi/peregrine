//! The library: where media lives, what the application has, and how far the user has
//! watched.
//!
//! One type per table in `DATA_MODEL.md` §2.4 to §2.6, with a field per column and a field
//! type that carries the column's constraint, the same shape as [`crate::catalogue`]. Two
//! things here need more than a field type, and both get it:
//!
//! - A subtitle track's owner is a `CHECK` over two columns (`media_file_id IS NOT NULL OR
//!   episode_id IS NOT NULL`), so it is [`SubtitleOwner`] rather than two nullable ids: the
//!   illegal combination — a track belonging to nothing — has no value that spells it.
//! - A watch-progress row's percentage is *derived* from its position and runtime (I6), so
//!   [`WatchProgress`] keeps its fields private and writes that derivation in one place.
//!   `percent` cannot be set from outside; a stored row is validated by [`WatchProgress::restore`]
//!   against the same derivation before it becomes a value.
//!
//! [`WatchProgress`] decides *the storage floor* and nothing else: a watched row below
//! [`STORAGE_FLOOR_PERCENT`] must have been set by hand, because I6 only promises that the
//! database never holds a measured watched row below it. When playback finishes a title is
//! [`crate::watch`]'s decision, at a higher threshold, and the row takes that decision through
//! [`WatchProgress::apply_finalize`]; the entity does not recompute it from a percentage.
//!
//! [`WatchProgress`] is also the one entity here that carries no row id. `watch_progress` has a
//! generated `entity_key` column and a `UNIQUE (account_id, entity_key)` constraint, so its
//! natural key is the account and the title or episode; the surrogate `id` exists for the row
//! mapper's own `UPDATE`, and a value that has not been written yet cannot have one.
//!
//! `media_file.path` and `library_root.path` are [`NonEmptyText`] rather than
//! [`SafePath`](crate::path::SafePath):
//! a value rebuilt from a stored row has no `PathGuard` to resolve against, and containment
//! is the scan boundary's rule (AGENTS R21), not the entity's. I3's uniqueness and I11's
//! missing-file behaviour are described where they are enforced, on [`MediaFile`].

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::account::AccountId;
use crate::catalogue::{EpisodeNumber, SeasonNumber};
use crate::release::HdrFormat;
use crate::stored_enum;
use crate::text::NonEmptyText;

stored_enum! {
    /// What a library root holds, as `library_root.kind` stores it.
    LibraryRootKind {
        /// Films only.
        Movies => "movies",
        /// Series only.
        Shows => "shows",
        /// Both.
        Mixed => "mixed",
        /// A torrent client's download directory, watched rather than scanned.
        Downloads => "downloads",
    }
}

stored_enum! {
    /// When a root is scanned, as `library_root.scan_mode` stores it.
    ScanMode {
        /// Rescan it as part of an incremental pass.
        Incremental => "incremental",
        /// Rescan it on a schedule.
        Scheduled => "scheduled",
        /// Rescan it only when the user asks.
        Manual => "manual",
    }
}

stored_enum! {
    /// Whether the library has a title or an episode, as `media_item.state` stores it.
    MediaItemState {
        /// Wanted but not acquired.
        Wanted => "wanted",
        /// Being acquired.
        Downloading => "downloading",
        /// Present and complete.
        Available => "available",
        /// Some files are present or some are missing.
        Partial => "partial",
        /// Acquisition failed.
        Failed => "failed",
    }
}

stored_enum! {
    /// How a media item came to exist, as `media_item.origin` stores it.
    MediaOrigin {
        /// Found by a library scan.
        Scan => "scan",
        /// Fulfilled by a torrent.
        Torrent => "torrent",
        /// Added by hand.
        Manual => "manual",
    }
}

stored_enum! {
    /// Where a subtitle track came from, as `subtitle_track.origin` stores it.
    SubtitleOrigin {
        /// Muxed into the media file.
        Embedded => "embedded",
        /// A file beside the media file.
        External => "external",
        /// Fetched from a subtitle provider.
        Downloaded => "downloaded",
        /// Produced by transcription or translation.
        Generated => "generated",
    }
}

stored_enum! {
    /// A subtitle track's container, as `subtitle_track.format` stores it.
    ///
    /// `Pgs` and `Vobsub` are bitmap formats: they are shown as images and cannot be
    /// re-timed by editing text, which is why the format is modelled rather than inferred
    /// from the file extension.
    SubtitleFormat {
        /// `SubRip`.
        Srt => "srt",
        /// `Advanced SubStation Alpha`.
        Ass => "ass",
        /// `SubStation Alpha`.
        Ssa => "ssa",
        /// `WebVTT`.
        Vtt => "vtt",
        /// `MicroDVD` or `SubViewer`.
        Sub => "sub",
        /// Presentation Graphic Stream.
        Pgs => "pgs",
        /// DVD bitmap subtitles.
        Vobsub => "vobsub",
    }
}

stored_enum! {
    /// Where a watch-progress row's numbers came from, as `watch_progress.source` stores it.
    ///
    /// `Manual` is the marker I6 keys on: only a row the user set by hand may claim to be
    /// watched below the storage floor, because only then is the claim a statement of intent
    /// rather than a measurement.
    ProgressSource {
        /// Reported by this installation's player.
        Local => "local",
        /// Imported from Trakt.
        Trakt => "trakt",
        /// Set by the user.
        Manual => "manual",
    }
}

/// How a watched flag came to be set, for the one call that has to tell them apart.
///
/// I6 accepts a watched row below [`STORAGE_FLOOR_PERCENT`] only when the user put it there, so
/// the caller has to say which it is; guessing from the percentage would defeat the point of
/// having a floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchOrigin {
    /// The user marked it, which is a statement of intent and may sit below the floor.
    Manual,
    /// The watched policy decided it ([`crate::watch`]), which must satisfy the floor.
    Policy,
}

/// The lowest share of a runtime a watched row may record when the user did not set it (I6).
///
/// This is a *floor*, not the playback threshold: [`crate::watch::WATCHED_PERCENT`] is what
/// normal playback writes, and it is higher on purpose so that a manual or imported mark
/// between the two numbers stays representable instead of the database rejecting the row.
pub const STORAGE_FLOOR_PERCENT: f64 = 0.92;

/// A reported playback position and, when the player knows it, the runtime it is measured
/// against.
///
/// This is the input to the one derivation in the module, so the millisecond values are
/// validated once here and every percentage afterwards is a function of them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "PlaybackPositionDraft")]
pub struct PlaybackPosition {
    position_ms: i64,
    duration_ms: Option<i64>,
}

impl PlaybackPosition {
    /// Builds a position, refusing a negative millisecond value.
    ///
    /// A missing or zero runtime is accepted and means "not known": the percentage is then
    /// `0.0`, because a player reports a zero runtime for a stream it has not probed yet and
    /// dividing by it would be worse than admitting the gap.
    ///
    /// # Errors
    ///
    /// [`WatchProgressError::NegativePosition`] or [`WatchProgressError::NegativeDuration`].
    pub fn new(position_ms: i64, duration_ms: Option<i64>) -> Result<Self, WatchProgressError> {
        if position_ms < 0 {
            return Err(WatchProgressError::NegativePosition { value: position_ms });
        }
        if let Some(duration) = duration_ms
            && duration < 0
        {
            return Err(WatchProgressError::NegativeDuration { value: duration });
        }
        Ok(Self {
            position_ms,
            duration_ms,
        })
    }

    /// The position, in milliseconds from the start.
    #[must_use]
    pub const fn position_ms(self) -> i64 {
        self.position_ms
    }

    /// The runtime, in milliseconds, when it is known.
    #[must_use]
    pub const fn duration_ms(self) -> Option<i64> {
        self.duration_ms
    }

    /// The share of the runtime played; `0.0` when the runtime is unknown or zero.
    ///
    /// Past the end is reported as it is rather than clamped: a player that reports a
    /// position beyond a stale runtime is telling the truth about the position, and pinning
    /// it to `1.0` would hide that.
    #[must_use]
    pub fn percent(self) -> f64 {
        percent_of(self.position_ms, self.duration_ms)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlaybackPositionDraft {
    position_ms: i64,
    duration_ms: Option<i64>,
}

impl TryFrom<PlaybackPositionDraft> for PlaybackPosition {
    type Error = WatchProgressError;

    fn try_from(draft: PlaybackPositionDraft) -> Result<Self, Self::Error> {
        Self::new(draft.position_ms, draft.duration_ms)
    }
}

// The cast is exact for every runtime that can exist: a millisecond count stays below 2^53,
// where every integer is representable in an `f64`, until roughly 285 000 years.
#[allow(clippy::cast_precision_loss)]
fn percent_of(position_ms: i64, duration_ms: Option<i64>) -> f64 {
    match duration_ms {
        Some(duration) if duration > 0 => position_ms as f64 / duration as f64,
        _ => 0.0,
    }
}

/// Why a watch-progress value was refused.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum WatchProgressError {
    /// A playback position was negative.
    #[error("a playback position cannot be negative, not {value} ms")]
    NegativePosition {
        /// The refused value.
        value: i64,
    },
    /// A runtime was negative.
    #[error("a runtime cannot be negative, not {value} ms")]
    NegativeDuration {
        /// The refused value.
        value: i64,
    },
    /// A row claims to be watched below the storage floor without having been set by hand (I6).
    #[error(
        "a watched row is {percent} played, below the {} storage floor, and was not set by hand",
        STORAGE_FLOOR_PERCENT
    )]
    WatchedBelowFloor {
        /// The percentage the row's own position and runtime derive.
        percent: f64,
    },
}

/// The `watch_progress` columns the domain reads, before I6 is applied.
///
/// The repository builds this from the row and [`WatchProgress::restore`] turns it into a
/// value, so the row shape exists in exactly one place and the validation in exactly one
/// other. The row's surrogate `id` is absent because the value does not need it: the mapper
/// keys its own writes on `(account_id, entity_key)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredWatchProgress {
    /// The account the progress belongs to.
    pub account_id: AccountId,
    /// The title being watched.
    pub title_id: i64,
    /// The episode being watched, when it is a series.
    pub episode_id: Option<i64>,
    /// The stored position, in milliseconds.
    pub position_ms: i64,
    /// The stored runtime, in milliseconds.
    pub duration_ms: Option<i64>,
    /// The stored percentage, carried so the row's shape is complete.
    // The value is ignored on the way in: it is a cache of `position_ms / duration_ms`, the
    // domain is its only writer (I6), and comparing two `f64`s that took different paths to
    // the same number would reject rows over a last-bit difference that means nothing.
    pub percent: f64,
    /// Whether the row is marked watched.
    pub watched: bool,
    /// When it was marked watched.
    pub watched_at: Option<Timestamp>,
    /// How many times it has been played.
    pub play_count: u32,
    /// When the row was last written.
    pub updated_at: Timestamp,
    /// Where the numbers came from.
    pub source: ProgressSource,
    /// The device that reported them, when it was not this one.
    pub device_label: Option<String>,
}

/// One row of `watch_progress`: how far one account has watched one title or episode.
///
/// `percent` is derived, never assigned, which is what makes I6 hold by construction rather
/// than by discipline. A fresh row comes from [`WatchProgress::record`]; a row read back from
/// the database goes through [`WatchProgress::restore`], which re-derives the percentage from
/// the row's own position and runtime and refuses a watched flag the derivation cannot
/// support.
///
/// The watched *flag* is not derived here, though: the playback threshold belongs to
/// [`crate::watch`], and the entity only enforces [`STORAGE_FLOOR_PERCENT`] on a flag it is
/// handed. [`WatchProgress::record`] therefore stores a position as unwatched, and
/// [`WatchProgress::apply_finalize`] applies the policy's decision at the end of a playback.
///
/// The value is identified by [`WatchProgress::entity_key`] and the account, not by the row's
/// surrogate id, which the mapper keeps for itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "StoredWatchProgress")]
pub struct WatchProgress {
    account_id: AccountId,
    title_id: i64,
    episode_id: Option<i64>,
    position_ms: i64,
    duration_ms: Option<i64>,
    percent: f64,
    watched: bool,
    watched_at: Option<Timestamp>,
    play_count: u32,
    updated_at: Timestamp,
    source: ProgressSource,
    device_label: Option<String>,
}

impl WatchProgress {
    /// Builds the value for a position a player reported, deriving the percentage.
    ///
    /// The row is stored unwatched: whether a position finishes a title is the policy's
    /// decision ([`crate::watch::finalize`]), taken at the end of the playback and applied with
    /// [`WatchProgress::apply_finalize`]. A position that has been reported is not evidence
    /// that the session ended, and the two answers differ exactly at the threshold — which is
    /// why a row reported at 0.93 stays resumable instead of being stored finished.
    #[must_use]
    pub fn record(
        account_id: AccountId,
        title_id: i64,
        episode_id: Option<i64>,
        position: PlaybackPosition,
        source: ProgressSource,
        at: Timestamp,
        device_label: Option<String>,
    ) -> Self {
        Self {
            account_id,
            title_id,
            episode_id,
            position_ms: position.position_ms(),
            duration_ms: position.duration_ms(),
            percent: position.percent(),
            watched: false,
            watched_at: None,
            play_count: 0,
            updated_at: at,
            source,
            device_label,
        }
    }

    /// Rebuilds a stored row, re-deriving the percentage and refusing a flag I6 cannot hold.
    ///
    /// # Errors
    ///
    /// [`WatchProgressError::NegativePosition`] and [`WatchProgressError::NegativeDuration`]
    /// for a row whose millisecond columns cannot be a position, and
    /// [`WatchProgressError::WatchedBelowFloor`] when a row that was not set by hand claims to
    /// be watched below the storage floor.
    pub fn restore(stored: StoredWatchProgress) -> Result<Self, WatchProgressError> {
        let position = PlaybackPosition::new(stored.position_ms, stored.duration_ms)?;
        let derived = position.percent();
        if stored.watched
            && stored.source != ProgressSource::Manual
            && derived < STORAGE_FLOOR_PERCENT
        {
            return Err(WatchProgressError::WatchedBelowFloor { percent: derived });
        }
        Ok(Self {
            account_id: stored.account_id,
            title_id: stored.title_id,
            episode_id: stored.episode_id,
            position_ms: position.position_ms(),
            duration_ms: position.duration_ms(),
            percent: derived,
            watched: stored.watched,
            watched_at: stored.watched_at,
            play_count: stored.play_count,
            updated_at: stored.updated_at,
            source: stored.source,
            device_label: stored.device_label,
        })
    }

    /// Moves the position, re-deriving the percentage and the watched flag.
    ///
    /// A row that is still watched keeps the instant it was first marked, whether this move
    /// takes it further past the floor or back below [`crate::watch::WATCHED_PERCENT`] but not
    /// below the floor: "watched at" means when it was finished, not when it was last touched,
    /// and a re-watch's coalesced writes must not keep re-stamping it. A row that drops below
    /// the storage floor loses the flag and its stamp — that is what makes marking watched
    /// reversible by simply playing the item again. The flag here is the floor's, not the
    /// policy's: a row above the floor but below [`crate::watch::WATCHED_PERCENT`] is left
    /// unwatched by [`WatchProgress::record`] and is unaffected either way.
    ///
    /// One row is exempt from all of that: one the user marked unwatched. Its percentage still
    /// follows the position, but the flag and the manual source stay, so that the next
    /// [`WatchProgress::apply_finalize`] — the only caller that closes a session — can still see
    /// the intent after the position has moved it past the floor. A hand-set *watched* flag is
    /// not exempt: it cannot persist below the floor anyway (I6 refuses to read it back), and
    /// keeping a manual watched mark across a playback belongs to `PG-E18-06`, which owns the
    /// marking surface.
    pub fn set_position(&mut self, position: PlaybackPosition, at: Timestamp) {
        let percent = position.percent();
        if self.source == ProgressSource::Manual && !self.watched {
            self.position_ms = position.position_ms();
            self.duration_ms = position.duration_ms();
            self.percent = percent;
            self.updated_at = at;
            return;
        }
        let watched = percent >= STORAGE_FLOOR_PERCENT;
        if watched && !self.watched {
            self.watched_at = Some(at);
        }
        if !watched && self.watched {
            self.watched_at = None;
        }
        self.position_ms = position.position_ms();
        self.duration_ms = position.duration_ms();
        self.percent = percent;
        self.watched = watched;
        self.updated_at = at;
    }

    /// Marks the row watched, enforcing the storage floor unless the user did it by hand.
    ///
    /// A [`WatchOrigin::Policy`] mark has to satisfy I6, so a policy that decides a title is
    /// watched below the floor gets a typed refusal rather than an unwritable row. A
    /// [`WatchOrigin::Manual`] mark is the user's statement of intent and is exempt: it also
    /// becomes [`ProgressSource::Manual`], which is the marker [`WatchProgress::restore`]
    /// reads back so the row survives a reload.
    ///
    /// # Errors
    ///
    /// [`WatchProgressError::WatchedBelowFloor`] when a policy mark is below the floor.
    pub fn mark_watched(
        &mut self,
        origin: WatchOrigin,
        at: Timestamp,
    ) -> Result<(), WatchProgressError> {
        if origin == WatchOrigin::Policy && self.percent < STORAGE_FLOOR_PERCENT {
            return Err(WatchProgressError::WatchedBelowFloor {
                percent: self.percent,
            });
        }
        if origin == WatchOrigin::Manual {
            self.source = ProgressSource::Manual;
        }
        if !self.watched {
            self.watched_at = Some(at);
        }
        self.watched = true;
        self.updated_at = at;
        Ok(())
    }

    /// Clears the watched flag and its stamp, leaving the position alone.
    ///
    /// The source becomes [`ProgressSource::Manual`] even when unwatching, because the row now
    /// records an intent rather than a measurement, and I6 keys its exemption on exactly that.
    pub fn mark_unwatched(&mut self, at: Timestamp) {
        self.watched = false;
        self.watched_at = None;
        self.source = ProgressSource::Manual;
        self.updated_at = at;
    }

    /// Applies a finished playback's decision ([`crate::watch::finalize`]) to the row.
    ///
    /// The playback's final position is moved first with [`WatchProgress::set_position`] — the
    /// policy decides the watched flag from it — and this then records the flag, the count, and
    /// the write. Two rules here are the entity's rather than the policy's:
    ///
    /// - A policy mark is routed through [`WatchProgress::mark_watched`], so a decision the
    ///   storage floor cannot hold is refused instead of written.
    /// - A row the user has marked unwatched keeps that flag. I6 calls a manual row a statement
    ///   of intent and a measured row a measurement; a finalize is a measurement, so it does
    ///   not overwrite the intent. The user marks a re-watch watched again from the library,
    ///   which is `PG-E18-06`'s surface.
    ///
    /// Returns whether the row changed, so a caller closing a session can skip a write that
    /// would say nothing new.
    ///
    /// # Errors
    ///
    /// [`WatchProgressError::WatchedBelowFloor`] when the outcome marks watched below the floor,
    /// which no policy that respects [`crate::watch::WATCHED_PERCENT`] ever does.
    pub fn apply_finalize(
        &mut self,
        outcome: crate::watch::FinalizeOutcome,
        at: Timestamp,
    ) -> Result<bool, WatchProgressError> {
        if self.source == ProgressSource::Manual && !self.watched {
            return Ok(false);
        }
        if outcome.watched && !self.watched {
            self.mark_watched(WatchOrigin::Policy, at)?;
        }
        if !outcome.watched && self.watched {
            self.watched = false;
            self.watched_at = None;
        }
        self.play_count = self.play_count.saturating_add(outcome.play_count);
        self.updated_at = at;
        Ok(true)
    }

    /// The value of the `entity_key` generated column: one progress row per account per
    /// episode, or per title when the row is not episodic.
    #[must_use]
    pub fn entity_key(&self) -> String {
        match self.episode_id {
            Some(episode_id) => format!("e{episode_id}"),
            None => format!("t{}", self.title_id),
        }
    }

    /// The account the progress belongs to.
    #[must_use]
    pub const fn account_id(&self) -> AccountId {
        self.account_id
    }

    /// The title being watched.
    #[must_use]
    pub const fn title_id(&self) -> i64 {
        self.title_id
    }

    /// The episode being watched, when it is a series.
    #[must_use]
    pub const fn episode_id(&self) -> Option<i64> {
        self.episode_id
    }

    /// The position, in milliseconds.
    #[must_use]
    pub const fn position_ms(&self) -> i64 {
        self.position_ms
    }

    /// The runtime, in milliseconds, when it is known.
    #[must_use]
    pub const fn duration_ms(&self) -> Option<i64> {
        self.duration_ms
    }

    /// The derived percentage.
    #[must_use]
    pub const fn percent(&self) -> f64 {
        self.percent
    }

    /// Whether the row is marked watched.
    #[must_use]
    pub const fn watched(&self) -> bool {
        self.watched
    }

    /// When it was marked watched.
    #[must_use]
    pub const fn watched_at(&self) -> Option<Timestamp> {
        self.watched_at
    }

    /// How many times it has been played.
    #[must_use]
    pub const fn play_count(&self) -> u32 {
        self.play_count
    }

    /// When the row was last written.
    #[must_use]
    pub const fn updated_at(&self) -> Timestamp {
        self.updated_at
    }

    /// Where the numbers came from.
    #[must_use]
    pub const fn source(&self) -> ProgressSource {
        self.source
    }

    /// The device that reported them, when it was not this one.
    #[must_use]
    pub fn device_label(&self) -> Option<&str> {
        self.device_label.as_deref()
    }
}

impl TryFrom<StoredWatchProgress> for WatchProgress {
    type Error = WatchProgressError;

    fn try_from(stored: StoredWatchProgress) -> Result<Self, Self::Error> {
        Self::restore(stored)
    }
}

/// One row of `library_root`: a directory the scanner is allowed to walk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryRoot {
    /// The row id.
    pub id: i64,
    /// The absolute, canonical root path, resolved through `PathGuard` before it is stored.
    pub path: NonEmptyText,
    /// What the root is expected to hold.
    pub kind: LibraryRootKind,
    /// When it is scanned.
    pub scan_mode: ScanMode,
    /// When it was last scanned.
    pub last_scan_at: Option<Timestamp>,
    /// When the row was created.
    pub created_at: Timestamp,
}

/// One row of `media_item`: the library's answer to "do we have this".
///
/// Files are the copies; this is the item. One episode may have several files — a 1080p and a
/// 2160p copy, or a duplicate the user has not cleaned up — and they all hang off this row,
/// which is why deleting a file can downgrade the item without touching the title.
///
/// A film's item has no `episode_id` and a series' item always has one, which is what the two
/// partial unique indices (I2) say: `episode_id` is `Some` for an episode and `None` for a
/// film, and the database refuses a second item for either.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaItem {
    /// The row id.
    pub id: i64,
    /// The title the item is for.
    pub title_id: i64,
    /// The episode, for a series item; `None` for a film.
    pub episode_id: Option<i64>,
    /// Whether it is wanted, downloading, available, partial, or failed.
    pub state: MediaItemState,
    /// How it came to exist.
    pub origin: MediaOrigin,
    /// The file the item prefers, when it has more than one.
    pub preferred_file_id: Option<i64>,
    /// The best-known quality of the preferred file.
    pub quality_label: Option<String>,
    /// When the row was created.
    pub created_at: Timestamp,
    /// When the row was last written.
    pub updated_at: Timestamp,
}

impl MediaItem {
    /// Whether this item is for an episode rather than a film.
    #[must_use]
    pub const fn is_episode(&self) -> bool {
        self.episode_id.is_some()
    }
}

/// One row of `media_file`: one copy of one media item on disk.
///
/// I3 lives on `path`: it is unique and canonical, so two items never claim one file. The
/// canonicalization happens in `PathGuard` before the row is written, because resolving a
/// path is IO and this crate has none.
///
/// I11 lives on `missing_since`: a file that is gone at scan time is marked rather than
/// deleted, so the watch progress hanging off it survives a re-download. The marking is an
/// operation rather than a constructor because it happens to a row that already exists, and
/// it is reversible, because a file can come back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaFile {
    /// The row id.
    pub id: i64,
    /// The item this file is a copy of.
    pub media_item_id: i64,
    /// The absolute, canonical path.
    pub path: NonEmptyText,
    /// The library root the path is inside, when it is inside one.
    pub root_id: Option<i64>,
    /// The file size, in bytes.
    pub size_bytes: u64,
    /// The filesystem modification time.
    pub mtime_ms: Timestamp,
    /// When the file was added to the library.
    pub added_at: Timestamp,
    /// When the path was found absent, or `None` while it is present.
    pub missing_since: Option<Timestamp>,
    /// The container format.
    pub container: Option<String>,
    /// The runtime, in milliseconds, as probed.
    pub duration_ms: Option<u64>,
    /// The video codec.
    pub video_codec: Option<String>,
    /// The video profile, such as `High` or `Main 10`.
    pub video_profile: Option<String>,
    /// The frame width, in pixels.
    pub width: Option<u32>,
    /// The frame height, in pixels.
    pub height: Option<u32>,
    /// The video bit depth.
    pub bit_depth: Option<u8>,
    /// The high dynamic range format.
    pub hdr_format: Option<HdrFormat>,
    /// The frame rate, in frames per second.
    pub frame_rate: Option<f64>,
    /// The overall bitrate, in kilobits per second.
    pub bitrate_kbps: Option<u32>,
    /// The default audio codec.
    pub audio_codec: Option<String>,
    /// The default audio channel count.
    pub audio_channels: Option<u8>,
    /// The default audio channel layout.
    pub audio_layout: Option<String>,
    /// The audio languages, as BCP-47 tags.
    // The column is a JSON array; the repository owns the encoding.
    pub audio_languages: Option<Vec<String>>,
    /// The container's subtitle format, which the schema does not constrain to the subtitle
    /// track's list because a probe reports codec names rather than file formats.
    pub subtitle_format: Option<String>,
    /// The probe schema version that produced the cached probe output below.
    pub probe_version: Option<u32>,
    /// The raw, compressed probe output, carried opaquely.
    // Gzip bytes rather than text: the column is `TEXT` in the schema but the payload is
    // binary, and the domain never reads it.
    pub probe_json: Option<Vec<u8>>,
    /// The quality label derived from the probed fields.
    pub quality_label: Option<String>,
    /// When the row was last written.
    pub updated_at: Timestamp,
}

impl MediaFile {
    /// Marks the path absent at scan time, keeping the row and its first observation.
    ///
    /// Re-marking a file that is already missing keeps the earlier instant: "missing since" is
    /// when the library first noticed, which is what tells a user how long something has been
    /// gone (I11).
    pub fn mark_missing(&mut self, at: Timestamp) {
        if self.missing_since.is_none() {
            self.missing_since = Some(at);
        }
        self.updated_at = at;
    }

    /// Clears the missing mark because the path is present again.
    pub fn mark_present(&mut self, at: Timestamp) {
        self.missing_since = None;
        self.updated_at = at;
    }
}

/// One row of `unmatched_file`: a file the scanner could not confidently attribute.
///
/// The parsed fields are the scanner's guess and the confidence is how sure it is; keeping a
/// guess with a low confidence is the point, because the review screen exists to turn it into
/// a match or a dismissal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnmatchedFile {
    /// The row id.
    pub id: i64,
    /// The absolute, canonical path.
    pub path: NonEmptyText,
    /// The file size, in bytes.
    pub size_bytes: u64,
    /// The filesystem modification time.
    pub mtime_ms: Timestamp,
    /// The title name the scanner parsed out of the path.
    pub parsed_title: Option<String>,
    /// The year the scanner parsed out of the path.
    pub parsed_year: Option<u16>,
    /// The season the scanner parsed out of the path.
    pub parsed_season: Option<SeasonNumber>,
    /// The episode the scanner parsed out of the path.
    pub parsed_episode: Option<EpisodeNumber>,
    /// How confident the scanner is, from zero to one.
    pub confidence: Option<f64>,
    /// Why the file was not matched.
    pub reason: Option<String>,
    /// The title the scanner suggests.
    pub suggested_title_id: Option<i64>,
    /// The episode the scanner suggests.
    pub suggested_episode_id: Option<i64>,
    /// When the file was first seen.
    pub first_seen_at: Timestamp,
}

/// Which row a subtitle track hangs off.
///
/// The schema's constraint is "at least one owner", expressed over two nullable columns, so
/// the legal combinations are the three variants here and the illegal one — a track belonging
/// to nothing — has no value that spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SubtitleOwner {
    /// A track of one media file, embedded in it or stored beside it.
    File {
        /// The file the track belongs to.
        media_file_id: i64,
    },
    /// A track that belongs to an episode rather than to one copy of it.
    Episode {
        /// The episode the track belongs to.
        episode_id: i64,
    },
    /// A track matched both to a file and to the episode that file is.
    FileAndEpisode {
        /// The file the track was matched to.
        media_file_id: i64,
        /// The episode the file is.
        episode_id: i64,
    },
}

impl SubtitleOwner {
    /// The `media_file_id` column, when it is set.
    #[must_use]
    pub const fn media_file_id(self) -> Option<i64> {
        match self {
            Self::File { media_file_id } | Self::FileAndEpisode { media_file_id, .. } => {
                Some(media_file_id)
            }
            Self::Episode { .. } => None,
        }
    }

    /// The `episode_id` column, when it is set.
    #[must_use]
    pub const fn episode_id(self) -> Option<i64> {
        match self {
            Self::Episode { episode_id } | Self::FileAndEpisode { episode_id, .. } => {
                Some(episode_id)
            }
            Self::File { .. } => None,
        }
    }
}

/// One row of `subtitle_track`: a subtitle stream or sidecar file.
// Four independent flags rather than one state: forced display, SDH captions, default
// selection, and hash match are four separate database columns with four separate meanings,
// and collapsing them into one enum would misrepresent the schema.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubtitleTrack {
    /// The row id.
    pub id: i64,
    /// The file and episode the track belongs to.
    pub owner: SubtitleOwner,
    /// Where the track came from.
    pub origin: SubtitleOrigin,
    /// The track's container.
    pub format: SubtitleFormat,
    /// The language, as a BCP-47 tag.
    pub language: NonEmptyText,
    /// The label to show instead of the language.
    pub label: Option<String>,
    /// Whether the track only covers forced narrative.
    pub is_forced: bool,
    /// Whether the track carries descriptions for the hard of hearing.
    pub is_sdh: bool,
    /// Whether the track is preferred when nothing else is chosen.
    pub is_default: bool,
    /// The stream index inside the container, for an embedded track.
    pub stream_index: Option<u32>,
    /// The sidecar path, for an external track.
    pub path: Option<String>,
    /// The provider the track was downloaded from.
    pub provider: Option<String>,
    /// The track's id at that provider.
    pub provider_subtitle_id: Option<String>,
    /// The URL the track was downloaded from.
    pub provider_url: Option<String>,
    /// The release name the track was matched against.
    pub release_name: Option<String>,
    /// The text encoding of a sidecar file.
    pub encoding: Option<String>,
    /// A timing correction the user applied, in milliseconds.
    // Signed: a positive offset delays the subtitles and a negative one advances them.
    pub offset_ms: i64,
    /// Whether the track was matched by file hash rather than by release name.
    pub hash_matched: bool,
    /// When the track was downloaded.
    pub downloaded_at: Option<Timestamp>,
    /// When the track was last selected.
    pub last_used_at: Option<Timestamp>,
}

/// One row of `watchlist_entry`: a title one account intends to watch.
///
/// The primary key is `(account_id, title_id)`, so the entry is its own identity: adding a
/// title twice is the same entry rather than a duplicate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchlistEntry {
    /// The account the entry belongs to.
    pub account_id: AccountId,
    /// The title on the list.
    pub title_id: i64,
    /// When it was added.
    pub added_at: Timestamp,
    /// The user's manual ordering, when they have one.
    pub position: Option<u32>,
    /// A note the user left on the entry.
    pub note: Option<String>,
    /// Whether acquiring the title is automatic when it appears.
    pub auto_acquire: bool,
}

impl WatchlistEntry {
    /// Adds a title with no ordering, note, or automatic acquisition.
    #[must_use]
    pub const fn new(account_id: AccountId, title_id: i64, added_at: Timestamp) -> Self {
        Self {
            account_id,
            title_id,
            added_at,
            position: None,
            note: None,
            auto_acquire: false,
        }
    }
}

/// One row of `smart_collection`: a saved query with a name.
///
/// The query is stored as JSON and stays opaque here: parsing it needs a JSON dependency that
/// this crate deliberately does not have (AGENTS R6), so the entity guarantees only that a
/// query, a sort, and a name exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmartCollection {
    /// The row id.
    pub id: i64,
    /// The name shown in the sidebar.
    pub name: NonEmptyText,
    /// The icon to show beside it.
    pub icon: Option<String>,
    /// The saved query, as JSON.
    pub query_json: NonEmptyText,
    /// The sort expression.
    pub sort: NonEmptyText,
    /// Whether the collection is pinned to the top of the sidebar.
    pub pinned: bool,
    /// When the row was created.
    pub created_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    macro_rules! assert_check_values {
        ($enum:ty, [$($value:literal),+ $(,)?]) => {
            assert_eq!(
                <$enum>::ALL.iter().map(|value| value.as_str()).collect::<Vec<_>>(),
                [$($value),+],
                "{} does not match its CHECK constraint",
                stringify!($enum)
            );
        };
    }

    macro_rules! assert_text_round_trip {
        ($enum:ty) => {
            for value in <$enum>::ALL {
                assert_eq!(<$enum>::parse(value.as_str()), Some(*value));
                let json = serde_json::to_string(value).expect("serializes");
                assert_eq!(json, format!("\"{}\"", value.as_str()));
                assert_eq!(
                    serde_json::from_str::<$enum>(&json).expect("deserializes"),
                    *value
                );
            }
        };
    }

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millisecond(millis).expect("a millisecond inside the representable range")
    }

    fn now() -> Timestamp {
        at(1_700_000_000_000)
    }

    fn text(value: &str) -> NonEmptyText {
        NonEmptyText::new(value).expect("the fixture text is not empty")
    }

    fn account() -> AccountId {
        AccountId::new(1).expect("the fixture account exists")
    }

    fn position(position_ms: i64, duration_ms: Option<i64>) -> PlaybackPosition {
        PlaybackPosition::new(position_ms, duration_ms).expect("the fixture position is valid")
    }

    fn record(position_ms: i64, duration_ms: Option<i64>) -> WatchProgress {
        WatchProgress::record(
            account(),
            1,
            None,
            position(position_ms, duration_ms),
            ProgressSource::Local,
            now(),
            None,
        )
    }

    fn stored(percent: f64, watched: bool, source: ProgressSource) -> StoredWatchProgress {
        StoredWatchProgress {
            account_id: account(),
            title_id: 1,
            episode_id: None,
            position_ms: 500,
            duration_ms: Some(1000),
            percent,
            watched,
            watched_at: watched.then_some(now()),
            play_count: 1,
            updated_at: now(),
            source,
            device_label: Some("laptop".to_owned()),
        }
    }

    fn media_file() -> MediaFile {
        MediaFile {
            id: 1,
            media_item_id: 2,
            path: text("/media/Movies/Blade Runner 2049.mkv"),
            root_id: Some(3),
            size_bytes: 62_000_000_000,
            mtime_ms: at(1_690_000_000_000),
            added_at: at(1_695_000_000_000),
            missing_since: None,
            container: Some("matroska".to_owned()),
            duration_ms: Some(9_840_000),
            video_codec: Some("hevc".to_owned()),
            video_profile: Some("Main 10".to_owned()),
            width: Some(3840),
            height: Some(2160),
            bit_depth: Some(10),
            hdr_format: Some(HdrFormat::Hdr10),
            frame_rate: Some(23.976),
            bitrate_kbps: Some(51_000),
            audio_codec: Some("eac3".to_owned()),
            audio_channels: Some(6),
            audio_layout: Some("5.1".to_owned()),
            audio_languages: Some(vec!["en".to_owned(), "fr".to_owned()]),
            subtitle_format: Some("subrip".to_owned()),
            probe_version: Some(3),
            probe_json: Some(vec![0x1f, 0x8b, 0x08]),
            quality_label: Some("2160p HDR".to_owned()),
            updated_at: now(),
        }
    }

    fn subtitle_track() -> SubtitleTrack {
        SubtitleTrack {
            id: 1,
            owner: SubtitleOwner::FileAndEpisode {
                media_file_id: 1,
                episode_id: 9,
            },
            origin: SubtitleOrigin::Downloaded,
            format: SubtitleFormat::Srt,
            language: text("en"),
            label: Some("English".to_owned()),
            is_forced: false,
            is_sdh: true,
            is_default: true,
            stream_index: None,
            path: Some("/media/Movies/Blade Runner 2049.en.srt".to_owned()),
            provider: Some("opensubtitles".to_owned()),
            provider_subtitle_id: Some("12345".to_owned()),
            provider_url: Some("https://example.invalid/srt/12345".to_owned()),
            release_name: Some("Blade.Runner.2049.2017.2160p.WEB-DL".to_owned()),
            encoding: Some("UTF-8".to_owned()),
            offset_ms: -250,
            hash_matched: true,
            downloaded_at: Some(now()),
            last_used_at: None,
        }
    }

    fn round_trip<T>(value: &T) -> T
    where
        T: Serialize + for<'de> Deserialize<'de>,
    {
        let json = serde_json::to_string(value).expect("serializes");
        serde_json::from_str(&json).expect("deserializes")
    }

    // The expected value is computed here rather than through `percent_of`, so the property
    // test would fail if the production derivation ever stopped being position over runtime.
    #[allow(clippy::cast_precision_loss)]
    fn expected_percent(position_ms: i64, duration_ms: i64) -> f64 {
        position_ms as f64 / duration_ms as f64
    }

    #[test]
    fn every_enum_matches_the_check_constraint_it_mirrors() {
        assert_check_values!(LibraryRootKind, ["movies", "shows", "mixed", "downloads"]);
        assert_check_values!(ScanMode, ["incremental", "scheduled", "manual"]);
        assert_check_values!(
            MediaItemState,
            ["wanted", "downloading", "available", "partial", "failed"]
        );
        assert_check_values!(MediaOrigin, ["scan", "torrent", "manual"]);
        assert_check_values!(
            SubtitleOrigin,
            ["embedded", "external", "downloaded", "generated"]
        );
        assert_check_values!(
            SubtitleFormat,
            ["srt", "ass", "ssa", "vtt", "sub", "pgs", "vobsub"]
        );
        assert_check_values!(ProgressSource, ["local", "trakt", "manual"]);
    }

    #[test]
    fn every_enum_round_trips_through_its_stored_text_and_json() {
        assert_text_round_trip!(LibraryRootKind);
        assert_text_round_trip!(ScanMode);
        assert_text_round_trip!(MediaItemState);
        assert_text_round_trip!(MediaOrigin);
        assert_text_round_trip!(SubtitleOrigin);
        assert_text_round_trip!(SubtitleFormat);
        assert_text_round_trip!(ProgressSource);
    }

    #[test]
    fn an_enum_refuses_a_value_outside_its_constraint() {
        assert_eq!(MediaItemState::parse("complete"), None);
        assert_eq!(SubtitleFormat::parse("srt "), None);
        assert!(serde_json::from_str::<ProgressSource>("\"plex\"").is_err());
    }

    #[test]
    fn the_percentage_is_the_position_over_the_runtime() {
        assert_eq!(position(0, Some(1000)).percent(), 0.0);
        assert_eq!(position(500, Some(1000)).percent(), 0.5);
        assert_eq!(position(1000, Some(1000)).percent(), 1.0);
        assert_eq!(
            position(2000, Some(1000)).percent(),
            2.0,
            "past the end is reported, not clamped"
        );
    }

    #[test]
    fn an_unknown_or_zero_runtime_has_no_percentage() {
        for duration in [None, Some(0)] {
            let span = position(500, duration);
            assert_eq!(span.percent(), 0.0);
            assert_eq!(record(500, duration).percent(), 0.0);
            assert!(!record(500, duration).watched());
        }
    }

    #[test]
    fn a_recorded_position_is_not_watched_at_any_percentage() {
        let at_the_floor = record(920, Some(1000));
        let at_the_policy_threshold = record(950, Some(1000));
        let past_the_end = record(1000, Some(1000));

        assert!(
            !at_the_floor.watched(),
            "0.92 is the storage floor (I6), not a decision the entity may take"
        );
        assert_eq!(at_the_floor.watched_at(), None);
        assert!(
            !at_the_policy_threshold.watched() && !past_the_end.watched(),
            "a reported position is not a finished playback; the policy decides that at finalize"
        );
    }

    #[test]
    fn the_storage_floor_is_a_floor_and_not_the_playback_threshold() {
        assert_eq!(
            STORAGE_FLOOR_PERCENT, 0.92,
            "I6's floor is the weakest watched row the database accepts"
        );
        // The policy's own threshold is higher, which is the distinction the entity must not
        // blur; the compile-time comparison lives with the policy that owns that number.
        assert_eq!(crate::watch::WATCHED_PERCENT, 0.95);
    }

    #[test]
    fn a_negative_position_or_runtime_is_refused() {
        assert_eq!(
            PlaybackPosition::new(-1, Some(1000)),
            Err(WatchProgressError::NegativePosition { value: -1 })
        );
        assert_eq!(
            PlaybackPosition::new(0, Some(-1)),
            Err(WatchProgressError::NegativeDuration { value: -1 })
        );
        assert!(PlaybackPosition::new(i64::MIN, None).is_err());
    }

    #[test]
    fn a_stored_row_is_re_derived_rather_than_trusted() {
        let restored = WatchProgress::restore(stored(0.9, false, ProgressSource::Local))
            .expect("the row itself is sound");
        assert_eq!(
            restored.percent(),
            0.5,
            "the stored percentage is a cache of the derivation (I6), not an input"
        );

        let negative = StoredWatchProgress {
            position_ms: -5,
            ..stored(0.5, false, ProgressSource::Local)
        };
        assert_eq!(
            WatchProgress::restore(negative),
            Err(WatchProgressError::NegativePosition { value: -5 })
        );
    }

    #[test]
    fn a_watched_row_below_the_storage_floor_must_have_been_set_by_hand() {
        let measured = WatchProgress::restore(stored(0.5, true, ProgressSource::Local));
        assert_eq!(
            measured,
            Err(WatchProgressError::WatchedBelowFloor { percent: 0.5 })
        );

        let imported = WatchProgress::restore(stored(0.5, true, ProgressSource::Trakt));
        assert!(imported.is_err(), "an import is a measurement too");

        let manual = WatchProgress::restore(stored(0.5, true, ProgressSource::Manual))
            .expect("the user may mark a half-watched episode watched");
        assert!(manual.watched());
        assert_eq!(manual.source(), ProgressSource::Manual);
    }

    #[test]
    fn marking_watched_and_unwatched_is_reversible() {
        let mut progress = record(100, Some(1000));
        let later = at(1_700_000_100_000);

        progress
            .mark_watched(WatchOrigin::Manual, later)
            .expect("a hand mark may sit below the storage floor");
        assert!(progress.watched());
        assert_eq!(progress.watched_at(), Some(later));
        assert_eq!(progress.source(), ProgressSource::Manual);
        assert_eq!(
            progress.percent(),
            0.1,
            "marking watched is a statement, not a measurement"
        );

        progress.mark_unwatched(later);
        assert!(!progress.watched());
        assert_eq!(progress.watched_at(), None);
        assert_eq!(progress.position_ms(), 100, "the position is untouched");
    }

    #[test]
    fn a_policy_mark_below_the_storage_floor_is_refused() {
        let mut progress = record(910, Some(1000));
        assert_eq!(
            progress.mark_watched(WatchOrigin::Policy, now()),
            Err(WatchProgressError::WatchedBelowFloor { percent: 0.91 })
        );
        assert!(!progress.watched(), "a refused mark writes nothing");
        assert_eq!(progress.source(), ProgressSource::Local);

        let mut at_the_floor = record(920, Some(1000));
        at_the_floor
            .mark_watched(WatchOrigin::Policy, now())
            .expect("0.92 is the weakest row I6 accepts");
        assert!(at_the_floor.watched());
        assert_eq!(
            at_the_floor.source(),
            ProgressSource::Local,
            "a policy mark is still a measurement, so it keeps the source that reported it"
        );
    }

    #[test]
    fn recording_a_position_re_derives_the_storage_floor_flag() {
        let mut progress = record(1000, Some(1000));
        progress
            .mark_watched(WatchOrigin::Policy, now())
            .expect("1.0 is above the floor");
        assert!(progress.watched());
        let first_stamp = progress.watched_at();

        let later = at(1_700_000_100_000);
        progress.set_position(position(999, Some(1000)), later);
        assert!(progress.watched(), "0.999 is still above the floor");
        assert_eq!(
            progress.watched_at(),
            first_stamp,
            "the stamp is when it was finished, not when it was last touched"
        );

        progress.set_position(position(100, Some(1000)), later);
        assert!(!progress.watched(), "rewatching clears the flag");
        assert_eq!(progress.watched_at(), None);
    }

    #[test]
    fn a_hand_set_watch_flag_does_not_survive_a_reported_position() {
        let mut progress = record(100, Some(1000));
        progress
            .mark_watched(WatchOrigin::Manual, now())
            .expect("the user may mark it by hand");
        assert!(progress.watched());

        progress.set_position(position(200, Some(1000)), at(1_700_000_100_000));
        assert!(
            !progress.watched(),
            "the position is the only writer of the derived fields (I6); the library surface that \
             has to keep a manual override is PG-E18-06's"
        );
    }

    #[test]
    fn a_finalize_applies_the_policy_decision_to_the_row() {
        let mut progress = record(500, Some(1000));
        let ended = at(1_700_000_200_000);

        let outcome = crate::watch::finalize(position(990, Some(1000)), false, ended);
        progress.set_position(position(990, Some(1000)), ended);
        assert!(
            progress
                .apply_finalize(outcome, ended)
                .expect("0.99 clears the storage floor")
        );
        assert!(progress.watched());
        assert_eq!(progress.watched_at(), Some(ended));
        assert_eq!(progress.play_count(), 1);
        assert_eq!(progress.position_ms(), 990, "the final position is kept");
        assert_eq!(progress.updated_at(), ended);
    }

    #[test]
    fn a_manual_unwatch_survives_the_next_finalize() {
        let mut progress = record(500, Some(1000));
        let marked = at(1_700_000_100_000);
        progress.mark_unwatched(marked);
        assert_eq!(progress.source(), ProgressSource::Manual);

        let ended = at(1_700_000_200_000);
        let outcome = crate::watch::finalize(position(990, Some(1000)), true, ended);
        progress.set_position(position(990, Some(1000)), ended);
        assert!(
            !progress
                .apply_finalize(outcome, ended)
                .expect("the manual row is left alone"),
            "the policy does not overwrite a statement of intent"
        );
        assert!(!progress.watched());
        assert_eq!(progress.watched_at(), None);
        assert_eq!(
            progress.play_count(),
            0,
            "an ignored finalize counts nothing"
        );
    }

    #[test]
    fn a_re_watch_bumps_the_count_without_clearing_the_stamp() {
        let mut progress = record(500, Some(1000));
        let finished = at(1_700_000_100_000);
        progress.set_position(position(990, Some(1000)), finished);
        progress
            .apply_finalize(
                crate::watch::finalize(position(990, Some(1000)), false, finished),
                finished,
            )
            .expect("above the floor");
        assert_eq!(progress.play_count(), 1);
        let first_stamp = progress.watched_at();
        assert_eq!(first_stamp, Some(finished));

        // The second playback: the player reports the position again, which must not disturb a
        // stamp that already means "finished", and then closes the session.
        let again = at(1_700_000_200_000);
        progress.set_position(position(990, Some(1000)), again);
        progress
            .apply_finalize(
                crate::watch::finalize(position(990, Some(1000)), false, again),
                again,
            )
            .expect("still above the floor");

        assert_eq!(
            progress.play_count(),
            2,
            "the second playback is a second play"
        );
        assert_eq!(
            progress.watched_at(),
            first_stamp,
            "watched_at is when it was first finished, not when it was last played"
        );
        assert!(progress.watched());
    }

    #[test]
    fn a_policy_finalize_the_floor_cannot_hold_is_refused() {
        let mut progress = record(500, Some(1000));
        let ended = at(1_700_000_200_000);
        let outcome = crate::watch::FinalizeOutcome {
            watched: true,
            watched_at: Some(ended),
            play_count: 1,
        };
        assert_eq!(
            progress.apply_finalize(outcome, ended),
            Err(WatchProgressError::WatchedBelowFloor { percent: 0.5 })
        );
        assert!(!progress.watched());
    }

    #[test]
    fn the_entity_key_separates_a_title_row_from_an_episode_row() {
        let movie = record(0, Some(1000));
        let episode = WatchProgress::record(
            account(),
            1,
            Some(9),
            position(0, Some(1000)),
            ProgressSource::Local,
            now(),
            None,
        );
        assert_eq!(movie.entity_key(), "t1");
        assert_eq!(episode.entity_key(), "e9");
        assert_ne!(movie.entity_key(), episode.entity_key());
    }

    #[test]
    fn watch_progress_round_trips_through_json() {
        let progress = WatchProgress::record(
            account(),
            4,
            Some(9),
            position(920, Some(1000)),
            ProgressSource::Local,
            now(),
            Some("laptop".to_owned()),
        );
        let json = serde_json::to_string(&progress).unwrap();
        assert_eq!(
            serde_json::from_str::<WatchProgress>(&json).unwrap(),
            progress
        );
        assert!(
            json.contains("\"percent\":0.92"),
            "the derived percentage is part of the value that travels"
        );
        assert!(
            !progress.watched(),
            "and 0.92 does not make it watched, because that is the floor rather than the policy"
        );
    }

    #[test]
    fn a_recorded_row_starts_from_the_columns_the_player_reported() {
        let progress = WatchProgress::record(
            account(),
            4,
            Some(9),
            position(500, Some(1000)),
            ProgressSource::Local,
            now(),
            Some("laptop".to_owned()),
        );
        assert_eq!(progress.account_id(), account());
        assert_eq!(progress.title_id(), 4);
        assert_eq!(progress.episode_id(), Some(9));
        assert_eq!(progress.position_ms(), 500);
        assert_eq!(progress.duration_ms(), Some(1000));
        assert_eq!(progress.percent(), 0.5);
        assert!(!progress.watched());
        assert_eq!(progress.watched_at(), None);
        assert_eq!(progress.play_count(), 0, "a new row starts unplayed");
        assert_eq!(progress.updated_at(), now());
        assert_eq!(progress.source(), ProgressSource::Local);
        assert_eq!(progress.device_label(), Some("laptop"));
    }

    #[test]
    fn watch_progress_json_refuses_a_broken_row() {
        let mut json = serde_json::to_value(stored(0.5, false, ProgressSource::Local)).unwrap();
        json["source"] = serde_json::Value::String("sync".to_owned());
        assert!(serde_json::from_value::<WatchProgress>(json).is_err());

        let mut unmarked = serde_json::to_value(stored(0.5, false, ProgressSource::Local)).unwrap();
        unmarked["watched"] = serde_json::Value::Bool(true);
        assert!(serde_json::from_value::<WatchProgress>(unmarked).is_err());

        let mut negative = serde_json::to_value(stored(0.5, false, ProgressSource::Local)).unwrap();
        negative["duration_ms"] = serde_json::Value::from(-1);
        assert!(serde_json::from_value::<WatchProgress>(negative).is_err());
    }

    #[test]
    fn a_missing_file_keeps_its_row_and_can_come_back() {
        let mut file = media_file();
        assert!(file.missing_since.is_none());

        let noticed = at(1_710_000_000_000);
        file.mark_missing(noticed);
        assert_eq!(file.missing_since, Some(noticed));
        assert_eq!(file.updated_at, noticed);

        let noticed_again = at(1_711_000_000_000);
        file.mark_missing(noticed_again);
        assert_eq!(
            file.missing_since,
            Some(noticed),
            "missing since is the first observation"
        );

        file.mark_present(noticed_again);
        assert_eq!(file.missing_since, None);
        assert_eq!(file.updated_at, noticed_again);
    }

    #[test]
    fn a_media_item_knows_whether_it_is_an_episode() {
        let movie = MediaItem {
            id: 1,
            title_id: 2,
            episode_id: None,
            state: MediaItemState::Available,
            origin: MediaOrigin::Scan,
            preferred_file_id: Some(3),
            quality_label: Some("1080p".to_owned()),
            created_at: now(),
            updated_at: now(),
        };
        assert!(!movie.is_episode());

        let episode = MediaItem {
            episode_id: Some(9),
            ..movie.clone()
        };
        assert!(episode.is_episode());
        assert_eq!(movie.title_id, episode.title_id, "both belong to one title");
    }

    #[test]
    fn a_subtitle_track_always_has_an_owner() {
        let file = SubtitleOwner::File { media_file_id: 1 };
        assert_eq!(file.media_file_id(), Some(1));
        assert_eq!(file.episode_id(), None);

        let episode = SubtitleOwner::Episode { episode_id: 9 };
        assert_eq!(episode.media_file_id(), None);
        assert_eq!(episode.episode_id(), Some(9));

        let both = SubtitleOwner::FileAndEpisode {
            media_file_id: 1,
            episode_id: 9,
        };
        assert_eq!(both.media_file_id(), Some(1));
        assert_eq!(both.episode_id(), Some(9));
    }

    #[test]
    fn the_remaining_library_entities_round_trip_through_json() {
        let root = LibraryRoot {
            id: 3,
            path: text("/media/Movies"),
            kind: LibraryRootKind::Movies,
            scan_mode: ScanMode::Incremental,
            last_scan_at: Some(now()),
            created_at: now(),
        };
        assert_eq!(round_trip(&root), root);

        let item = MediaItem {
            id: 2,
            title_id: 1,
            episode_id: None,
            state: MediaItemState::Partial,
            origin: MediaOrigin::Torrent,
            preferred_file_id: None,
            quality_label: None,
            created_at: now(),
            updated_at: now(),
        };
        assert_eq!(round_trip(&item), item);

        let file = media_file();
        assert_eq!(round_trip(&file), file);

        let unmatched = UnmatchedFile {
            id: 4,
            path: text("/media/Downloads/mystery.2024.1080p.mkv"),
            size_bytes: 8_000_000_000,
            mtime_ms: at(1_690_000_000_000),
            parsed_title: Some("Mystery".to_owned()),
            parsed_year: Some(2024),
            parsed_season: Some(SeasonNumber::new(1)),
            parsed_episode: Some(EpisodeNumber::new(2)),
            confidence: Some(0.62),
            reason: Some("no external id".to_owned()),
            suggested_title_id: Some(1),
            suggested_episode_id: Some(9),
            first_seen_at: now(),
        };
        assert_eq!(round_trip(&unmatched), unmatched);

        let track = subtitle_track();
        assert_eq!(round_trip(&track), track);

        let entry = WatchlistEntry::new(account(), 1, now());
        assert_eq!(round_trip(&entry), entry);

        let collection = SmartCollection {
            id: 5,
            name: text("Unwatched films"),
            icon: Some("film".to_owned()),
            query_json: text("{\"kind\":\"movie\"}"),
            sort: text("added_at DESC"),
            pinned: true,
            created_at: now(),
        };
        assert_eq!(round_trip(&collection), collection);
    }

    #[test]
    fn required_library_columns_are_not_representable_when_empty() {
        assert!(
            serde_json::from_str::<LibraryRoot>(
                "{\"id\":1,\"path\":\"\",\"kind\":\"movies\",\"scan_mode\":\"manual\",\
             \"last_scan_at\":null,\"created_at\":0}"
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<SubtitleTrack>(
                "{\"id\":1,\"owner\":{\"File\":{\"media_file_id\":1}},\"origin\":\"external\",\
             \"format\":\"srt\",\"language\":\"\",\"label\":null,\"is_forced\":false,\
             \"is_sdh\":false,\"is_default\":false,\"stream_index\":null,\"path\":null,\
             \"provider\":null,\"provider_subtitle_id\":null,\"provider_url\":null,\
             \"release_name\":null,\"encoding\":null,\"offset_ms\":0,\"hash_matched\":false,\
             \"downloaded_at\":null,\"last_used_at\":null}"
            )
            .is_err()
        );
    }

    #[test]
    fn an_unmatched_file_refuses_an_impossible_year() {
        let mut json = serde_json::to_value(UnmatchedFile {
            id: 4,
            path: text("/media/Downloads/mystery.mkv"),
            size_bytes: 1,
            mtime_ms: now(),
            parsed_title: None,
            parsed_year: None,
            parsed_season: None,
            parsed_episode: None,
            confidence: None,
            reason: None,
            suggested_title_id: None,
            suggested_episode_id: None,
            first_seen_at: now(),
        })
        .unwrap();
        json["parsed_year"] = serde_json::Value::from(-1);
        assert!(serde_json::from_value::<UnmatchedFile>(json).is_err());
    }

    #[test]
    fn a_media_file_refuses_an_unknown_hdr_format() {
        let mut json = serde_json::to_value(media_file()).unwrap();
        json["hdr_format"] = serde_json::Value::String("hdr12".to_owned());
        assert!(serde_json::from_value::<MediaFile>(json).is_err());
    }

    proptest! {
        #[test]
        fn the_percentage_is_always_the_position_over_the_runtime(
            position_ms in 0i64..86_400_000,
            duration_ms in 1i64..86_400_000,
        ) {
            let span = PlaybackPosition::new(position_ms, Some(duration_ms)).expect("non-negative");
            prop_assert_eq!(span.percent().to_bits(), expected_percent(position_ms, duration_ms).to_bits());
        }

        #[test]
        fn a_recorded_row_is_never_watched_whatever_the_percentage(
            position_ms in 0i64..86_400_000,
            duration_ms in 1i64..86_400_000,
        ) {
            let span = PlaybackPosition::new(position_ms, Some(duration_ms)).expect("non-negative");
            let progress = WatchProgress::record(
                AccountId::new(1).expect("positive"),
                1,
                None,
                span,
                ProgressSource::Local,
                Timestamp::from_millisecond(0).expect("in range"),
                None,
            );
            // The flag is the policy's to set at the end of a playback, so no reported position,
            // however far past the end, may produce a watched row on its own.
            prop_assert!(!progress.watched());
            prop_assert_eq!(progress.watched_at(), None);
            prop_assert!(progress.percent().is_finite());
            prop_assert!(progress.percent() >= 0.0);
            prop_assert_eq!(progress.percent().to_bits(), expected_percent(position_ms, duration_ms).to_bits());
        }

        #[test]
        fn a_policy_mark_is_accepted_exactly_from_the_storage_floor(
            position_ms in 0i64..86_400_000,
            duration_ms in 1i64..86_400_000,
        ) {
            let span = PlaybackPosition::new(position_ms, Some(duration_ms)).expect("non-negative");
            let mut progress = WatchProgress::record(
                AccountId::new(1).expect("positive"),
                1,
                None,
                span,
                ProgressSource::Local,
                Timestamp::from_millisecond(0).expect("in range"),
                None,
            );
            let marked = progress.mark_watched(WatchOrigin::Policy, Timestamp::from_millisecond(1).expect("in range"));
            prop_assert_eq!(marked.is_ok(), span.percent() >= STORAGE_FLOOR_PERCENT);
            prop_assert_eq!(progress.watched(), marked.is_ok());
        }

        #[test]
        fn a_recorded_row_always_survives_its_own_round_trip(
            position_ms in 0i64..86_400_000,
            duration_ms in 0i64..86_400_000,
        ) {
            let span = PlaybackPosition::new(position_ms, Some(duration_ms)).expect("non-negative");
            let progress = WatchProgress::record(
                AccountId::new(1).expect("positive"),
                1,
                None,
                span,
                ProgressSource::Local,
                Timestamp::from_millisecond(1_700_000_000_000).expect("in range"),
                None,
            );
            let json = serde_json::to_string(&progress).expect("serializes");
            prop_assert_eq!(serde_json::from_str::<WatchProgress>(&json).expect("deserializes"), progress);
        }

        #[test]
        fn a_position_reference_never_panics(position_ms in any::<i64>(), duration_ms in any::<Option<i64>>()) {
            let _ = PlaybackPosition::new(position_ms, duration_ms);
        }
    }
}
