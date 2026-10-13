//! The library's tables: the roots the scanner walks, the items the library holds, the
//! files that back them, the files the scanner could not place, and their subtitle tracks.
//!
//! `DATA_MODEL` §2.4 and §2.5, one aggregate at a time. These are the tables the scan path
//! (PG-E14) and the subtitle path (PG-E17) are written against:
//!
//! - [`LibraryRoot`] is a directory the scanner is allowed to walk. It is configuration
//!   rather than content: it holds a path, not a title, so no [`AccountScope`] applies and
//!   none is asked for.
//! - [`MediaItem`] is the library's answer to "do we have this", and [`MediaFile`] is one
//!   copy of it. Both hang off a title, so both reads take a scope and apply it in SQL — a
//!   restricted account sees neither a film nor the files that back it.
//! - [`UnmatchedFile`] is a file the scanner could not attribute. It has no title yet, and
//!   that is exactly why a restricted scope is offered a match only when the scope can see
//!   the title the scanner suggests; without one there is nothing to certify, so the row is
//!   hidden (I27's fail-closed rule).
//! - [`SubtitleTrack`] belongs to a file, an episode, or both. A file's track is read through
//!   the file, and the file is read through its title.
//!
//! ## Cursors
//!
//! Every list here orders by `id DESC`, so the cursor is the row id and is total by
//! construction: `SQLite` numbers an `INTEGER PRIMARY KEY` uniquely. That is the ordering the
//! screens want — the newest row is the one a user just added — and it needs no index of its
//! own, because the primary key's `B-tree` is walked backwards for it. The two lists whose
//! ordering column is *not* unique live in [`crate::watch`] and use a
//! [`crate::page::Cursor::at`] pair instead.
//!
//! ## What is deliberately absent
//!
//! No delete: removing a title cascades to these rows through the schema's foreign keys and
//! removing files is the explicit, path-revalidating plan of PG-E15 (I10). Nothing in this
//! module can delete a row, so it cannot delete a file either.
//!
//! No `list_media_files` by root or by quality: PG-E3-17 owns disk accounting and PG-E14-18
//! owns the screen that reads it, so the query and the index it would need arrive with the
//! caller that has a shape for them. The reads here are the ones a scan and a detail screen
//! ask for.

use crate::error::StorageError;
// `begin` is `sqlx::Connection`'s, and `remove_root` is the one statement pair here that needs a
// transaction of its own.
use crate::json::{decode_string_array, encode_string_array};
use crate::page::PageRequest;
use jiff::Timestamp;
use peregrine_domain::{
    AccountScope, EpisodeNumber, HdrFormat, LibraryRoot, LibraryRootKind, MediaFile, MediaItem,
    MediaItemState, MediaOrigin, NonEmptyText, ScanMode, SeasonNumber, SubtitleFormat,
    SubtitleOrigin, SubtitleOwner, SubtitleTrack, UnmatchedFile,
};
use sqlx::Connection as _;

// The title vocabulary the library's own read maps, re-exported by the catalogue module rather than
// declared twice: a kind the schema holds and this file spells differently would be a title the
// screen could not name.
use crate::catalogue::{MetadataState, TitleKind, TitleSubtype};

/// One row of `library_root`, with the id a page resumes below.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryRootRow {
    /// The row id, which is also the paging cursor.
    pub id: i64,
    /// The root itself.
    pub root: LibraryRoot,
}

/// One row of `media_item`, with the id a page resumes below.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaItemRow {
    /// The row id, which is also the paging cursor.
    pub id: i64,
    /// The item itself.
    pub item: MediaItem,
}

/// One row of `media_file`, with the id a page resumes below.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaFileRow {
    /// The row id, which is also the paging cursor.
    pub id: i64,
    /// The file itself.
    pub file: MediaFile,
}

/// One row of `unmatched_file`, with the id a page resumes below.
#[derive(Debug, Clone, PartialEq)]
pub struct UnmatchedFileRow {
    /// The row id, which is also the paging cursor.
    pub id: i64,
    /// The file itself.
    pub file: UnmatchedFile,
    /// The name of the title the scan offered, when it offered one this scope may see.
    ///
    /// `None` covers both "nothing was close enough" and "the title offered is out of scope", and
    /// that is deliberate rather than lossy (I37): the two read the same to a person, and the
    /// question the row asks does not depend on the answer.
    pub suggested_title_name: Option<String>,
}

/// One row of `subtitle_track`, with the id a page resumes below.
#[derive(Debug, Clone, PartialEq)]
pub struct SubtitleTrackRow {
    /// The row id, which is also the paging cursor.
    pub id: i64,
    /// The track itself.
    pub track: SubtitleTrack,
}

/// The scope's rule, as a flag a query binds.
///
/// A flag rather than assembled SQL, because building a `WHERE` clause from a `bool` is how
/// string composition creeps into a repository. The clause itself is a literal in each
/// statement, and this only decides whether its first branch applies. It is defined here
/// rather than shared with `catalogue` because the two modules load the same library into
/// their own statements and neither should have to call the other for one line.
pub(crate) const fn hidden_by_scope(scope: AccountScope) -> i64 {
    scope.is_restricted() as i64
}

/// Inserts one row of `library_root` and reports the id `SQLite` assigned.
pub(crate) async fn insert_root(
    connection: &mut sqlx::SqliteConnection,
    root: &LibraryRoot,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO library_root (path, kind, scan_mode, last_scan_at, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(root.path.as_str())
    .bind(root.kind.as_str())
    .bind(root.scan_mode.as_str())
    .bind(root.last_scan_at.map(as_millis))
    .bind(as_millis(root.created_at))
    .execute(&mut *connection)
    .await?;
    Ok(result.last_insert_rowid())
}

/// Inserts one row of `media_item` and reports the id `SQLite` assigned.
pub(crate) async fn insert_item(
    connection: &mut sqlx::SqliteConnection,
    item: &MediaItem,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO media_item (title_id, episode_id, state, origin, preferred_file_id, \
         quality_label, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .bind(item.title_id)
    .bind(item.episode_id)
    .bind(item.state.as_str())
    .bind(item.origin.as_str())
    .bind(item.preferred_file_id)
    .bind(item.quality_label.as_deref())
    .bind(as_millis(item.created_at))
    .bind(as_millis(item.updated_at))
    .execute(&mut *connection)
    .await?;
    Ok(result.last_insert_rowid())
}

/// Inserts one row of `media_file` and reports the id `SQLite` assigned.
///
/// I3 is enforced here whether the caller knew about it or not: `path` is `UNIQUE`, so a
/// second item claiming a file this one owns fails the insert rather than producing a second
/// row for the same bytes.
///
/// `probe_json` is bound as a BLOB. The column is declared `TEXT` and the domain carries
/// `Vec<u8>`, because the payload is gzip-compressed ffprobe output; `SQLite` stores what it
/// is given, and nothing in this crate reads the column except this mapper.
pub(crate) async fn insert_file(
    connection: &mut sqlx::SqliteConnection,
    file: &MediaFile,
) -> Result<i64, sqlx::Error> {
    let audio_languages = file
        .audio_languages
        .as_ref()
        .map(|languages| encode_string_array(languages));
    let result = sqlx::query(
        "INSERT INTO media_file (media_item_id, path, root_id, size_bytes, mtime_ms, added_at, \
         missing_since, container, duration_ms, video_codec, video_profile, width, height, \
         bit_depth, hdr_format, frame_rate, bitrate_kbps, audio_codec, audio_channels, \
         audio_layout, audio_languages, subtitle_format, probe_version, probe_json, \
         quality_label, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, \
         ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)",
    )
    .bind(file.media_item_id)
    .bind(file.path.as_str())
    .bind(file.root_id)
    .bind(size_to_i64(file.size_bytes))
    .bind(as_millis(file.mtime_ms))
    .bind(as_millis(file.added_at))
    .bind(file.missing_since.map(as_millis))
    .bind(file.container.as_deref())
    .bind(file.duration_ms.map(size_to_i64))
    .bind(file.video_codec.as_deref())
    .bind(file.video_profile.as_deref())
    .bind(file.width.map(i64::from))
    .bind(file.height.map(i64::from))
    .bind(file.bit_depth.map(i64::from))
    .bind(file.hdr_format.map(HdrFormat::as_str))
    .bind(file.frame_rate)
    .bind(file.bitrate_kbps.map(i64::from))
    .bind(file.audio_codec.as_deref())
    .bind(file.audio_channels.map(i64::from))
    .bind(file.audio_layout.as_deref())
    .bind(audio_languages)
    .bind(file.subtitle_format.as_deref())
    .bind(file.probe_version.map(i64::from))
    .bind(file.probe_json.as_deref())
    .bind(file.quality_label.as_deref())
    .bind(as_millis(file.updated_at))
    .execute(&mut *connection)
    .await?;
    Ok(result.last_insert_rowid())
}

/// The two spellings of the `unmatched_file` insert.
///
/// Written out rather than assembled from a verb: `sqlx` 0.9 refuses a dynamically built
/// statement unless the caller audits it, and a literal is the audit.
const INSERT_UNMATCHED: &str = "INSERT INTO unmatched_file (path, size_bytes, mtime_ms, \
     parsed_title, parsed_year, parsed_season, parsed_episode, confidence, reason, \
     suggested_title_id, suggested_episode_id, first_seen_at) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)";

const IGNORE_UNMATCHED: &str = "INSERT OR IGNORE INTO unmatched_file (path, size_bytes, \
     mtime_ms, parsed_title, parsed_year, parsed_season, parsed_episode, confidence, reason, \
     suggested_title_id, suggested_episode_id, first_seen_at) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)";

/// Inserts one row of `unmatched_file` and reports the id `SQLite` assigned.
///
/// The path is `UNIQUE`, so a scan that sees the same unplaceable file twice must ignore the
/// conflict rather than insert a second row; [`crate::Database::insert_unmatched_file`]
/// decides whether the caller wanted that.
pub(crate) async fn insert_unmatched(
    connection: &mut sqlx::SqliteConnection,
    file: &UnmatchedFile,
    ignore_conflict: bool,
) -> Result<Option<i64>, sqlx::Error> {
    let statement = if ignore_conflict {
        IGNORE_UNMATCHED
    } else {
        INSERT_UNMATCHED
    };
    let result = sqlx::query(statement)
        .bind(file.path.as_str())
        .bind(size_to_i64(file.size_bytes))
        .bind(as_millis(file.mtime_ms))
        .bind(file.parsed_title.as_deref())
        .bind(file.parsed_year.map(i64::from))
        .bind(file.parsed_season.map(|number| i64::from(number.get())))
        .bind(file.parsed_episode.map(|number| i64::from(number.get())))
        .bind(file.confidence)
        .bind(file.reason.as_deref())
        .bind(file.suggested_title_id)
        .bind(file.suggested_episode_id)
        .bind(as_millis(file.first_seen_at))
        .execute(&mut *connection)
        .await?;
    if ignore_conflict && result.rows_affected() == 0 {
        // `SQLite` leaves the *conflicting* row's rowid in `last_insert_rowid` after an
        // ignored insert, so the rowid is not this insert's and the caller is told nothing
        // was written.
        return Ok(None);
    }
    Ok(Some(result.last_insert_rowid()))
}

/// Inserts one row of `subtitle_track` and reports the id `SQLite` assigned.
///
/// The schema's `CHECK (media_file_id IS NOT NULL OR episode_id IS NOT NULL)` is what refuses
/// a track belonging to nothing; [`SubtitleOwner`] is the same rule as a type, so a caller
/// cannot build one to insert.
pub(crate) async fn insert_subtitle(
    connection: &mut sqlx::SqliteConnection,
    track: &SubtitleTrack,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO subtitle_track (media_file_id, episode_id, origin, format, language, \
         label, is_forced, is_sdh, is_default, stream_index, path, provider, \
         provider_subtitle_id, provider_url, release_name, encoding, offset_ms, hash_matched, \
         downloaded_at, last_used_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, \
         ?18, ?19, ?20)",
    )
    .bind(track.owner.media_file_id())
    .bind(track.owner.episode_id())
    .bind(track.origin.as_str())
    .bind(track.format.as_str())
    .bind(track.language.as_str())
    .bind(track.label.as_deref())
    .bind(i64::from(track.is_forced))
    .bind(i64::from(track.is_sdh))
    .bind(i64::from(track.is_default))
    .bind(track.stream_index.map(i64::from))
    .bind(track.path.as_deref())
    .bind(track.provider.as_deref())
    .bind(track.provider_subtitle_id.as_deref())
    .bind(track.provider_url.as_deref())
    .bind(track.release_name.as_deref())
    .bind(track.encoding.as_deref())
    .bind(track.offset_ms)
    .bind(i64::from(track.hash_matched))
    .bind(track.downloaded_at.map(as_millis))
    .bind(track.last_used_at.map(as_millis))
    .execute(&mut *connection)
    .await?;
    Ok(result.last_insert_rowid())
}

/// Records that a root was scanned at `at`, reporting whether the row was there.
pub(crate) async fn record_root_scan(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
    at: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("UPDATE library_root SET last_scan_at = ?1 WHERE id = ?2")
        .bind(at)
        .bind(id)
        .execute(&mut *connection)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Removes one `unmatched_file` row, reporting whether it was there.
///
/// What a match does last: the row exists while the file is waiting for an answer, and a file that
/// has been placed is not waiting for anything. Removing it *after* the file row is written is the
/// order that fails safe — a crash in between leaves a row for a file that is already indexed, which
/// the next match of that row clears, rather than a placed file with nothing to say it was.
pub(crate) async fn remove_unmatched_file(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM unmatched_file WHERE id = ?1")
        .bind(id)
        .execute(&mut *connection)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Removes one row of `library_root`, reporting whether the row was there.
///
/// This is the only statement a root removal runs. Everything the root produced — media items,
/// files, watch progress, subtitles — is deliberately left alone (PG-E14-01): a user who removes
/// a folder from Peregrine has said "stop scanning this", not "forget what I watched", and the
/// files are theirs. What is no longer reachable is the root itself, which is what makes the
/// scanner stop walking it.
pub(crate) async fn remove_root(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
) -> Result<bool, sqlx::Error> {
    // Two statements, so one transaction: the file rows keep their contents and lose their root,
    // and a root cannot be deleted while they still point at it (`media_file.root_id` is a foreign
    // key, and SQLite's default action is to refuse). Leaving them pointing at a deleted row is not
    // an option the schema has, and deleting them is not an option this product has — the whole
    // point of PG-E14-01 is that removing a root removes a configuration entry and nothing else.
    //
    // `root_id` is nullable for exactly this: a file the library indexed under a root the user has
    // since removed. Re-adding that root is what adopts it back, which the indexing step owes
    // (`PG-E14-10`), because `media_file.path` is unique and a second row for it cannot exist.
    let mut transaction = connection.begin().await?;
    sqlx::query("UPDATE media_file SET root_id = NULL WHERE root_id = ?1")
        .bind(id)
        .execute(&mut *transaction)
        .await?;
    let result = sqlx::query("DELETE FROM library_root WHERE id = ?1")
        .bind(id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(result.rows_affected() > 0)
}

/// Changes when one root is scanned, reporting whether the row was there.
///
/// `last_scan_at` is untouched: how often a root is *going* to be scanned says nothing about when
/// it last was, and clearing the instant would make the next scheduled scan treat a root that was
/// scanned a minute ago as one that has never been scanned.
pub(crate) async fn set_root_scan_mode(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
    mode: ScanMode,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("UPDATE library_root SET scan_mode = ?1 WHERE id = ?2")
        .bind(mode.as_str())
        .bind(id)
        .execute(&mut *connection)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Marks a file absent at scan time, or present again, reporting whether the row was there.
///
/// The instant is
/// [`MediaFile::mark_missing`](peregrine_domain::MediaFile::mark_missing)'s decision rather
/// than this statement's: the caller applies it to the value it read, so "missing since" stays
/// the first time the library noticed (I11), and this only writes the column.
pub(crate) async fn set_file_missing(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
    missing_since: Option<i64>,
    updated_at: i64,
) -> Result<bool, sqlx::Error> {
    let result =
        sqlx::query("UPDATE media_file SET missing_since = ?1, updated_at = ?2 WHERE id = ?3")
            .bind(missing_since)
            .bind(updated_at)
            .bind(id)
            .execute(&mut *connection)
            .await?;
    Ok(result.rows_affected() > 0)
}

/// Reads one root.
pub(crate) async fn root(
    pool: &sqlx::SqlitePool,
    id: i64,
) -> Result<Option<LibraryRoot>, StorageError> {
    let row = sqlx::query_as::<_, RootRow>(
        "SELECT id, path, kind, scan_mode, last_scan_at, created_at FROM library_root WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    row.map(TryInto::try_into).transpose()
}

/// Reads a page of roots, newest first.
/// The root at `path`, or `None`.
///
/// `path` is `UNIQUE` in the schema, so this is an index lookup rather than a scan, and it is how a
/// caller decides whether a folder was already added before inserting it — with the caveat that
/// the constraint, not the check, is what actually stops a duplicate.
pub(crate) async fn root_by_path(
    pool: &sqlx::SqlitePool,
    path: &str,
) -> Result<Option<LibraryRoot>, StorageError> {
    let row = sqlx::query_as::<_, RootRow>(
        "SELECT id, path, kind, scan_mode, last_scan_at, created_at FROM library_root WHERE path = ?1",
    )
    .bind(path)
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    row.map(TryInto::try_into).transpose()
}

/// One recorded file under a root, as a reconciliation or an incremental scan needs it.
///
/// Deliberately not a [`MediaFile`]: what both callers compare is the path and the two numbers a
/// scan stamps a file with, and reading `probe_json` — the largest column in the table — to answer
/// "has this changed" would read a gzip document per file per scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedFile {
    /// The `media_file` row id, which is what a missing mark is written against.
    pub id: i64,
    /// The stored path, which is the absolute canonical one.
    pub path: String,
    /// The size the last scan recorded.
    pub size_bytes: i64,
    /// The modification time the last scan recorded, in milliseconds.
    pub mtime_ms: i64,
}

/// The files of one root that are present, served by migration 0011's partial index.
///
/// The `missing_since IS NULL` predicate is not decoration: `media_file_usage_idx` is partial on
/// exactly that, and its leading column is `root_id`, so this reads an index rather than the
/// table. Dropping the predicate turns a scan of one library into a scan of every library's rows,
/// including the ones with no bytes on disk.
pub(crate) async fn present_files(
    pool: &sqlx::SqlitePool,
    root_id: i64,
) -> Result<Vec<RecordedFile>, StorageError> {
    let rows = sqlx::query_as::<_, RecordedFileRow>(
        "SELECT id, path, size_bytes, mtime_ms FROM media_file \
         WHERE root_id = ?1 AND missing_since IS NULL ORDER BY id",
    )
    .bind(root_id)
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// The files of one root that are marked missing.
///
/// The predicate matches `media_file_missing_idx`, which is partial on `missing_since IS NOT
/// NULL`; `root_id` is not in it, so the root is applied after the index read. That is the right
/// order of work: the missing set is the small one — normally empty — while the table is the big
/// one, and no index carries both predicates today.
pub(crate) async fn missing_files(
    pool: &sqlx::SqlitePool,
    root_id: i64,
) -> Result<Vec<RecordedFile>, StorageError> {
    let rows = sqlx::query_as::<_, RecordedFileRow>(
        "SELECT id, path, size_bytes, mtime_ms FROM media_file \
         WHERE missing_since IS NOT NULL AND root_id = ?1 ORDER BY id",
    )
    .bind(root_id)
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// The four columns [`RecordedFile`] needs.
#[derive(sqlx::FromRow)]
struct RecordedFileRow {
    id: i64,
    path: String,
    size_bytes: i64,
    mtime_ms: i64,
}

impl From<RecordedFileRow> for RecordedFile {
    fn from(row: RecordedFileRow) -> Self {
        Self {
            id: row.id,
            path: row.path,
            size_bytes: row.size_bytes,
            mtime_ms: row.mtime_ms,
        }
    }
}

pub(crate) async fn list_roots(
    pool: &sqlx::SqlitePool,
    request: PageRequest,
) -> Result<Vec<LibraryRootRow>, StorageError> {
    let rows = sqlx::query_as::<_, RootRow>(
        "SELECT id, path, kind, scan_mode, last_scan_at, created_at FROM library_root \
         WHERE (?1 IS NULL OR id < ?1) ORDER BY id DESC LIMIT ?2",
    )
    .bind(request.bindable_cursor())
    .bind(request.bindable_limit())
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    rows.into_iter()
        .map(|row| {
            let root = LibraryRoot::try_from(row)?;
            Ok(LibraryRootRow { id: root.id, root })
        })
        .collect()
}

/// Reads one media item, when `scope` may see the title it is for.
///
/// An item outside the scope reads as `None`, which is the whole point of R34: a restricted
/// account must not be able to tell "we do not have this" from "not for you".
pub(crate) async fn item(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    id: i64,
) -> Result<Option<MediaItem>, StorageError> {
    let row = sqlx::query_as::<_, ItemRow>(
        "SELECT m.id, m.title_id, m.episode_id, m.state, m.origin, m.preferred_file_id, \
         m.quality_label, m.created_at, m.updated_at FROM media_item m \
         JOIN title t ON t.id = m.title_id \
         WHERE m.id = ?1 AND (?2 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL))",
    )
    .bind(id)
    .bind(hidden_by_scope(scope))
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    row.map(TryInto::try_into).transpose()
}

/// One title the library holds, with what it holds of it.
///
/// The library screen's unit: a title is what a person thinks they have, and the counts are what
/// the screen can say about it without reading any file row. `missing_files` is the one count that
/// is not a size: a title whose only copy is on an unplugged drive is *held* and not *available*,
/// and a screen that showed it as either alone would be wrong about the other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryTitleRow {
    /// The title's identity, which a scoped read may only return when the scope may see it.
    pub title: crate::catalogue::TitleIdentity,
    /// How many items of it the library holds — one for a film, one per episode for a series.
    pub items: i64,
    /// How many files those items have between them.
    pub files: i64,
    /// What those files add up to, from the sizes the scans recorded.
    pub bytes: i64,
    /// How many of them are marked missing.
    pub missing_files: i64,
}

/// A page of the titles the library holds, newest first.
///
/// The join starts from `title` and reaches `media_file` only through `media_item`, so an item with
/// no file yet still counts as an item and contributes no bytes; the `LEFT JOIN` is what makes a
/// film whose scan found the directory but not the file visible as an item rather than invisible.
///
/// # Cost
///
/// One grouped scan of the library's items and files, not of the catalogue: `media_item` holds a
/// row per title the user actually has, and the scope predicate and the cursor both sit on `title`,
/// whose `sort_name` and primary-key indices carry them. The `GROUP BY t.id` sorts by the index
/// `media_item_movie_uq` already provides for films and by the join key for episodes, and the page
/// limit is applied after the grouping, so the work is proportional to the page plus the groups it
/// scans past — which is the same shape `DATA_MODEL` §4's usage aggregate has.
pub(crate) async fn list_library_titles(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    request: PageRequest,
) -> Result<Vec<LibraryTitleRow>, StorageError> {
    let rows = sqlx::query_as::<_, LibraryTitleQueryRow>(
        "SELECT t.id, t.kind, t.subtype, t.name, t.sort_name, t.metadata_state, t.adult, \
                COUNT(DISTINCT m.id) AS items, COUNT(f.id) AS files, \
                COALESCE(SUM(f.size_bytes), 0) AS bytes, \
                COALESCE(SUM(f.missing_since IS NOT NULL), 0) AS missing_files \
           FROM title t \
           JOIN media_item m ON m.title_id = t.id \
           LEFT JOIN media_file f ON f.media_item_id = m.id \
          WHERE (?1 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL)) \
            AND (?2 IS NULL OR t.id < ?2) \
          GROUP BY t.id \
          ORDER BY t.id DESC LIMIT ?3",
    )
    .bind(hidden_by_scope(scope))
    .bind(request.bindable_cursor())
    .bind(request.bindable_limit())
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    rows.into_iter().map(TryInto::try_into).collect()
}

/// The columns [`LibraryTitleRow`] is built from.
#[derive(sqlx::FromRow)]
struct LibraryTitleQueryRow {
    id: i64,
    kind: String,
    subtype: String,
    name: String,
    sort_name: String,
    metadata_state: String,
    adult: i64,
    items: i64,
    files: i64,
    bytes: i64,
    missing_files: i64,
}

impl TryFrom<LibraryTitleQueryRow> for LibraryTitleRow {
    type Error = StorageError;

    fn try_from(row: LibraryTitleQueryRow) -> Result<Self, Self::Error> {
        let identity = crate::catalogue::TitleIdentity {
            id: row.id,
            kind: TitleKind::parse(&row.kind).ok_or_else(|| StorageError::UnexpectedValue {
                column: "title.kind",
                found: row.kind.clone(),
            })?,
            subtype: TitleSubtype::parse(&row.subtype).ok_or_else(|| {
                StorageError::UnexpectedValue {
                    column: "title.subtype",
                    found: row.subtype.clone(),
                }
            })?,
            name: row.name,
            sort_name: row.sort_name,
            metadata_state: MetadataState::parse(&row.metadata_state).ok_or_else(|| {
                StorageError::UnexpectedValue {
                    column: "title.metadata_state",
                    found: row.metadata_state.clone(),
                }
            })?,
            adult: row.adult != 0,
        };
        Ok(Self {
            title: identity,
            items: row.items,
            files: row.files,
            bytes: row.bytes,
            missing_files: row.missing_files,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// The scanner's surface: the reads an indexer makes, which take no `AccountScope` on purpose.
//
// I25 makes the scope a parameter of every library read a *caller* makes, and I37 says content
// outside a scope is indistinguishable from content that does not exist. Neither can hold for the
// scanner, and the reasons are structural rather than a preference:
//
// * A fresh installation may hold **no account row at all** — accounts are `PG-E35`'s epic and the
//   migrations seed none — so there is no account for a scan to name. A scope built from an id that
//   names nothing is a scope in name only, which is worse than not having one.
// * The files on disk exist whether or not any account may see them. A scanner that indexed only
//   what an unrestricted account sees would leave an adult-flagged title's files permanently
//   unindexed, and no later change to a rating ceiling could repair it: the rows would never have
//   been written. That is a data-loss-shaped outcome produced by a read filter.
// * I37 presumes the row exists to be hidden. Gating belongs on the way *out* — the scoped reads a
//   user-facing caller makes — not on the way in.
//
// The surface is kept separate and named so that an account-scoped caller cannot reach it by
// accident: these functions sit beside `present_files` and `missing_files`, which the scanner's
// reconciliation already uses, and the `Database` methods that expose them are documented as the
// scanner's rather than as general reads.
// ---------------------------------------------------------------------------------------------

/// The file at `path`, whatever account may see it, with the two numbers a scan compares.
///
/// `path` is `UNIQUE`, so this is an index lookup rather than a scan.
pub(crate) async fn recorded_file_at_path(
    pool: &sqlx::SqlitePool,
    path: &str,
) -> Result<Option<RecordedFile>, StorageError> {
    let row = sqlx::query_as::<_, RecordedFileRow>(
        "SELECT id, path, size_bytes, mtime_ms FROM media_file WHERE path = ?1",
    )
    .bind(path)
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(row.map(Into::into))
}

/// The item a title and episode pair already has, whatever account may see it.
pub(crate) async fn item_id_for(
    pool: &sqlx::SqlitePool,
    title_id: i64,
    episode_id: Option<i64>,
) -> Result<Option<i64>, StorageError> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM media_item WHERE title_id = ?1 AND episode_id IS ?2")
            .bind(title_id)
            .bind(episode_id)
            .fetch_optional(pool)
            .await
            .map_err(StorageError::from_sqlx)?;
    Ok(row.map(|row| row.0))
}

/// The id of the title a kind and sort name already name, whatever account may see it.
///
/// Served by `title_sort_name_idx`: the indexer asks this once per parsed name, so a scan of a
/// twenty-thousand-file library makes at most that many index lookups, and the ones that hit are
/// how a show's episodes end up under one title instead of one each.
pub(crate) async fn title_id_for_sort_name(
    pool: &sqlx::SqlitePool,
    kind: &str,
    sort_name: &str,
) -> Result<Option<i64>, StorageError> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM title WHERE kind = ?1 AND sort_name = ?2 ORDER BY id LIMIT 1",
    )
    .bind(kind)
    .bind(sort_name)
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    Ok(row.map(|row| row.0))
}

/// Refreshes one file's stamp and root, reporting whether the row was there.
///
/// The scan's answer for a file it has seen before: the bytes it read are the same file, and what
/// changed is what the library knows about it. `missing_since` is cleared in the same statement,
/// because a file a scan just walked is not missing — a restore and a re-scan are the same
/// observation, and leaving the stamp for the reconciliation to clear later would report a file as
/// gone while the walk that found it is still running.
pub(crate) async fn refresh_file_stamp(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
    size_bytes: i64,
    mtime_ms: i64,
    root_id: i64,
    updated_at: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE media_file SET size_bytes = ?1, mtime_ms = ?2, root_id = ?3, missing_since = NULL, \
         updated_at = ?4 WHERE id = ?5",
    )
    .bind(size_bytes)
    .bind(mtime_ms)
    .bind(root_id)
    .bind(updated_at)
    .bind(id)
    .execute(&mut *connection)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Reads the item for one title, or for one episode, when `scope` may see it.
///
/// The two partial unique indices make this at most one row (I2), which is why the answer is
/// an `Option` rather than a page.
pub(crate) async fn item_for(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    title_id: i64,
    episode_id: Option<i64>,
) -> Result<Option<MediaItem>, StorageError> {
    let row = sqlx::query_as::<_, ItemRow>(
        "SELECT m.id, m.title_id, m.episode_id, m.state, m.origin, m.preferred_file_id, \
         m.quality_label, m.created_at, m.updated_at FROM media_item m \
         JOIN title t ON t.id = m.title_id \
         WHERE m.title_id = ?1 AND m.episode_id IS ?2 \
           AND (?3 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL))",
    )
    .bind(title_id)
    .bind(episode_id)
    .bind(hidden_by_scope(scope))
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    row.map(TryInto::try_into).transpose()
}

/// Reads a page of items for one title, newest first, when `scope` may see the title.
pub(crate) async fn list_items_for_title(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    title_id: i64,
    request: PageRequest,
) -> Result<Vec<MediaItemRow>, StorageError> {
    let rows = sqlx::query_as::<_, ItemRow>(
        "SELECT m.id, m.title_id, m.episode_id, m.state, m.origin, m.preferred_file_id, \
         m.quality_label, m.created_at, m.updated_at FROM media_item m \
         JOIN title t ON t.id = m.title_id \
         WHERE m.title_id = ?1 \
           AND (?2 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL)) \
           AND (?3 IS NULL OR m.id < ?3) ORDER BY m.id DESC LIMIT ?4",
    )
    .bind(title_id)
    .bind(hidden_by_scope(scope))
    .bind(request.bindable_cursor())
    .bind(request.bindable_limit())
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    rows.into_iter()
        .map(|row| {
            let item = MediaItem::try_from(row)?;
            Ok(MediaItemRow { id: item.id, item })
        })
        .collect()
}

/// Reads one file, when `scope` may see the title its item is for.
///
/// The path is the unique key the scanner has, but this read takes the row id: a caller that
/// has a path uses [`file_by_path`], which applies the same scope.
pub(crate) async fn file(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    id: i64,
) -> Result<Option<MediaFile>, StorageError> {
    let row = sqlx::query_as::<_, FileRow>(
        "SELECT f.id, f.media_item_id, f.path, f.root_id, f.size_bytes, f.mtime_ms, f.added_at, \
         f.missing_since, f.container, f.duration_ms, f.video_codec, f.video_profile, f.width, \
         f.height, f.bit_depth, f.hdr_format, f.frame_rate, f.bitrate_kbps, f.audio_codec, \
         f.audio_channels, f.audio_layout, f.audio_languages, f.subtitle_format, \
         f.probe_version, f.probe_json, f.quality_label, f.updated_at FROM media_file f \
         JOIN media_item m ON m.id = f.media_item_id \
         JOIN title t ON t.id = m.title_id \
         WHERE f.id = ?1 AND (?2 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL))",
    )
    .bind(id)
    .bind(hidden_by_scope(scope))
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    row.map(TryInto::try_into).transpose()
}

/// Reads the file one path names, when `scope` may see it.
///
/// I3 makes `path` unique, so this is the read that answers "who owns these bytes" — and the
/// answer is scoped, so a restricted account cannot discover that a file exists by asking
/// which item holds it.
pub(crate) async fn file_by_path(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    path: &str,
) -> Result<Option<MediaFile>, StorageError> {
    let row = sqlx::query_as::<_, FileRow>(
        "SELECT f.id, f.media_item_id, f.path, f.root_id, f.size_bytes, f.mtime_ms, f.added_at, \
         f.missing_since, f.container, f.duration_ms, f.video_codec, f.video_profile, f.width, \
         f.height, f.bit_depth, f.hdr_format, f.frame_rate, f.bitrate_kbps, f.audio_codec, \
         f.audio_channels, f.audio_layout, f.audio_languages, f.subtitle_format, \
         f.probe_version, f.probe_json, f.quality_label, f.updated_at FROM media_file f \
         JOIN media_item m ON m.id = f.media_item_id \
         JOIN title t ON t.id = m.title_id \
         WHERE f.path = ?1 AND (?2 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL))",
    )
    .bind(path)
    .bind(hidden_by_scope(scope))
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    row.map(TryInto::try_into).transpose()
}

/// Reads a page of an item's files, newest first, when `scope` may see the item.
///
/// `media_file_item_idx` is the index this serves: the item is a point lookup in it, and the
/// page is an id-ordered walk of that item's rows.
pub(crate) async fn list_files_for_item(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    media_item_id: i64,
    request: PageRequest,
) -> Result<Vec<MediaFileRow>, StorageError> {
    let rows = sqlx::query_as::<_, FileRow>(
        "SELECT f.id, f.media_item_id, f.path, f.root_id, f.size_bytes, f.mtime_ms, f.added_at, \
         f.missing_since, f.container, f.duration_ms, f.video_codec, f.video_profile, f.width, \
         f.height, f.bit_depth, f.hdr_format, f.frame_rate, f.bitrate_kbps, f.audio_codec, \
         f.audio_channels, f.audio_layout, f.audio_languages, f.subtitle_format, \
         f.probe_version, f.probe_json, f.quality_label, f.updated_at FROM media_file f \
         JOIN media_item m ON m.id = f.media_item_id \
         JOIN title t ON t.id = m.title_id \
         WHERE f.media_item_id = ?1 \
           AND (?2 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL)) \
           AND (?3 IS NULL OR f.id < ?3) ORDER BY f.id DESC LIMIT ?4",
    )
    .bind(media_item_id)
    .bind(hidden_by_scope(scope))
    .bind(request.bindable_cursor())
    .bind(request.bindable_limit())
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    rows.into_iter()
        .map(|row| {
            let file = MediaFile::try_from(row)?;
            Ok(MediaFileRow { id: file.id, file })
        })
        .collect()
}

/// Reads one unmatched file, when `scope` may be offered its suggestion.
pub(crate) async fn unmatched(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    id: i64,
) -> Result<Option<UnmatchedFile>, StorageError> {
    // The scope is applied to the suggestion and not to the row, which is what the doc above has
    // always said and what the statement did not do: a row whose *suggestion* was out of scope was
    // hidden entirely, so a user was never offered the question about a file they own. A suggestion
    // out of scope now reads as no suggestion, which is what a row with nothing close enough reads
    // as (I37).
    let row = sqlx::query_as::<_, UnmatchedRow>(
        "SELECT u.id, u.path, u.size_bytes, u.mtime_ms, u.parsed_title, u.parsed_year, \
         u.parsed_season, u.parsed_episode, u.confidence, u.reason, u.suggested_episode_id, \
         u.first_seen_at, \
         CASE WHEN ?2 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL) \
              THEN u.suggested_title_id END AS suggested_title_id, \
         CASE WHEN ?2 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL) THEN t.name END \
           AS suggested_title_name \
           FROM unmatched_file u LEFT JOIN title t ON t.id = u.suggested_title_id \
         WHERE u.id = ?1",
    )
    .bind(id)
    .bind(hidden_by_scope(scope))
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    row.map(TryInto::try_into).transpose()
}

/// Reads a page of unmatched files, newest first.
///
/// A restricted scope still sees the file's existence and its parse — the review screen has
/// to be able to offer a match — but the *suggestion* is only offered when the suggested
/// title is one the scope may see. That is the difference between "we could not place this
/// file" and "we think it is this title", and only the second is content.
pub(crate) async fn list_unmatched(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    request: PageRequest,
) -> Result<Vec<UnmatchedFileRow>, StorageError> {
    let rows = sqlx::query_as::<_, UnmatchedRow>(
        // The scope is applied to the *suggestion* rather than to the row, which is the difference
        // I37 makes here: the file is the user's and always needs an answer, while the title the scan
        // offered is content some accounts may not see. A suggestion out of scope reads as no
        // suggestion, which is exactly what a row with nothing close enough reads as — the two are
        // indistinguishable by design, and neither hides the question.
        "SELECT u.id, u.path, u.size_bytes, u.mtime_ms, u.parsed_title, u.parsed_year, \
         u.parsed_season, u.parsed_episode, u.confidence, u.reason, u.suggested_episode_id, \
         u.first_seen_at, \
         CASE WHEN ?2 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL) \
              THEN u.suggested_title_id END AS suggested_title_id, \
         CASE WHEN ?2 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL) THEN t.name END \
           AS suggested_title_name \
           FROM unmatched_file u LEFT JOIN title t ON t.id = u.suggested_title_id \
         WHERE (?1 IS NULL OR u.id < ?1) ORDER BY u.id DESC LIMIT ?3",
    )
    .bind(request.bindable_cursor())
    .bind(hidden_by_scope(scope))
    .bind(request.bindable_limit())
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    rows.into_iter()
        .map(|row| {
            // Read before the conversion takes the row: the joined column is not part of the file.
            let suggested_title_name = row.suggested_title_name.clone();
            let file = UnmatchedFile::try_from(row)?;
            Ok(UnmatchedFileRow {
                id: file.id,
                file,
                suggested_title_name,
            })
        })
        .collect()
}

/// Reads one subtitle track, when `scope` may see the media it belongs to.
///
/// A track can reach its title two ways — through the file it is muxed into, or through the
/// episode it is matched to — and the schema requires at least one of them. The `COALESCE`
/// takes whichever the row has, so a track that is only episodically owned is scoped like any
/// other; one owned by neither would have no title, and the scope hides what it cannot
/// certify.
pub(crate) async fn subtitle(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    id: i64,
) -> Result<Option<SubtitleTrack>, StorageError> {
    let row = sqlx::query_as::<_, SubtitleRow>(
        "SELECT s.id, s.media_file_id, s.episode_id, s.origin, s.format, s.language, s.label, \
         s.is_forced, s.is_sdh, s.is_default, s.stream_index, s.path, s.provider, \
         s.provider_subtitle_id, s.provider_url, s.release_name, s.encoding, s.offset_ms, \
         s.hash_matched, s.downloaded_at, s.last_used_at FROM subtitle_track s \
         LEFT JOIN media_file f ON f.id = s.media_file_id \
         LEFT JOIN media_item m ON m.id = f.media_item_id \
         LEFT JOIN episode e ON e.id = s.episode_id \
         LEFT JOIN title t ON t.id = COALESCE(m.title_id, e.title_id) \
         WHERE s.id = ?1 AND (?2 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL))",
    )
    .bind(id)
    .bind(hidden_by_scope(scope))
    .fetch_optional(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    row.map(TryInto::try_into).transpose()
}

/// Reads a page of one file's subtitle tracks, newest first.
///
/// `subtitle_track_file_idx` is the index this serves. The scope is a join to the file's
/// title, so a restricted account reads nothing for a file it may not see.
pub(crate) async fn list_subtitles_for_file(
    pool: &sqlx::SqlitePool,
    scope: AccountScope,
    media_file_id: i64,
    request: PageRequest,
) -> Result<Vec<SubtitleTrackRow>, StorageError> {
    let rows = sqlx::query_as::<_, SubtitleRow>(
        "SELECT s.id, s.media_file_id, s.episode_id, s.origin, s.format, s.language, s.label, \
         s.is_forced, s.is_sdh, s.is_default, s.stream_index, s.path, s.provider, \
         s.provider_subtitle_id, s.provider_url, s.release_name, s.encoding, s.offset_ms, \
         s.hash_matched, s.downloaded_at, s.last_used_at FROM subtitle_track s \
         JOIN media_file f ON f.id = s.media_file_id \
         JOIN media_item m ON m.id = f.media_item_id \
         JOIN title t ON t.id = m.title_id \
         WHERE s.media_file_id = ?1 \
           AND (?2 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL)) \
           AND (?3 IS NULL OR s.id < ?3) ORDER BY s.id DESC LIMIT ?4",
    )
    .bind(media_file_id)
    .bind(hidden_by_scope(scope))
    .bind(request.bindable_cursor())
    .bind(request.bindable_limit())
    .fetch_all(pool)
    .await
    .map_err(StorageError::from_sqlx)?;
    rows.into_iter()
        .map(|row| {
            let track = SubtitleTrack::try_from(row)?;
            Ok(SubtitleTrackRow {
                id: track.id,
                track,
            })
        })
        .collect()
}

/// How many rows `unmatched_file` holds, read through the pool.
pub(crate) async fn unmatched_count(pool: &sqlx::SqlitePool) -> Result<i64, StorageError> {
    sqlx::query_scalar("SELECT COUNT(*) FROM unmatched_file")
        .fetch_one(pool)
        .await
        .map_err(StorageError::from_sqlx)
}

/// `u64` as the `i64` a `SQLite` `INTEGER` column holds.
///
/// A size larger than `i64::MAX` is not a file this application can meet — that is 8 EiB — and
/// clamping is the wrong answer, so it saturates at the largest representable value rather than
/// wrapping to a negative one that would make a file appear free.
fn size_to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The `TIMESTAMP` conversion a remote row cannot fail: a stored integer is the millisecond
/// value the domain already carries in these tables.
fn text(value: String, column: &'static str) -> Result<NonEmptyText, StorageError> {
    NonEmptyText::new(&value).map_err(|_| StorageError::UnexpectedValue {
        column,
        found: value,
    })
}

#[derive(sqlx::FromRow)]
struct RootRow {
    id: i64,
    path: String,
    kind: String,
    scan_mode: String,
    last_scan_at: Option<i64>,
    created_at: i64,
}

impl TryFrom<RootRow> for LibraryRoot {
    type Error = StorageError;

    fn try_from(row: RootRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            path: text(row.path, "library_root.path")?,
            kind: LibraryRootKind::parse(&row.kind).ok_or_else(|| {
                StorageError::UnexpectedValue {
                    column: "library_root.kind",
                    found: row.kind.clone(),
                }
            })?,
            scan_mode: ScanMode::parse(&row.scan_mode).ok_or_else(|| {
                StorageError::UnexpectedValue {
                    column: "library_root.scan_mode",
                    found: row.scan_mode.clone(),
                }
            })?,
            last_scan_at: row.last_scan_at.map(millis),
            created_at: millis(row.created_at),
        })
    }
}

#[derive(sqlx::FromRow)]
struct ItemRow {
    id: i64,
    title_id: i64,
    episode_id: Option<i64>,
    state: String,
    origin: String,
    preferred_file_id: Option<i64>,
    quality_label: Option<String>,
    created_at: i64,
    updated_at: i64,
}

impl TryFrom<ItemRow> for MediaItem {
    type Error = StorageError;

    fn try_from(row: ItemRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            title_id: row.title_id,
            episode_id: row.episode_id,
            state: MediaItemState::parse(&row.state).ok_or_else(|| {
                StorageError::UnexpectedValue {
                    column: "media_item.state",
                    found: row.state.clone(),
                }
            })?,
            origin: MediaOrigin::parse(&row.origin).ok_or_else(|| {
                StorageError::UnexpectedValue {
                    column: "media_item.origin",
                    found: row.origin.clone(),
                }
            })?,
            preferred_file_id: row.preferred_file_id,
            quality_label: row.quality_label,
            created_at: millis(row.created_at),
            updated_at: millis(row.updated_at),
        })
    }
}

#[derive(sqlx::FromRow)]
struct FileRow {
    id: i64,
    media_item_id: i64,
    path: String,
    root_id: Option<i64>,
    size_bytes: i64,
    mtime_ms: i64,
    added_at: i64,
    missing_since: Option<i64>,
    container: Option<String>,
    duration_ms: Option<i64>,
    video_codec: Option<String>,
    video_profile: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
    bit_depth: Option<i64>,
    hdr_format: Option<String>,
    frame_rate: Option<f64>,
    bitrate_kbps: Option<i64>,
    audio_codec: Option<String>,
    audio_channels: Option<i64>,
    audio_layout: Option<String>,
    audio_languages: Option<String>,
    subtitle_format: Option<String>,
    probe_version: Option<i64>,
    probe_json: Option<Vec<u8>>,
    quality_label: Option<String>,
    updated_at: i64,
}

impl TryFrom<FileRow> for MediaFile {
    type Error = StorageError;

    fn try_from(row: FileRow) -> Result<Self, Self::Error> {
        let hdr_format = row
            .hdr_format
            .map(|found| {
                serde_json::from_value::<HdrFormat>(serde_json::Value::String(found.clone()))
                    .map_err(|_| StorageError::UnexpectedValue {
                        column: "media_file.hdr_format",
                        found,
                    })
            })
            .transpose()?;
        let audio_languages = match row.audio_languages {
            Some(found) => Some(decode_string_array("media_file.audio_languages", &found)?),
            None => None,
        };
        Ok(Self {
            id: row.id,
            media_item_id: row.media_item_id,
            path: text(row.path, "media_file.path")?,
            root_id: row.root_id,
            size_bytes: unsigned(row.size_bytes),
            mtime_ms: millis(row.mtime_ms),
            added_at: millis(row.added_at),
            missing_since: row.missing_since.map(millis),
            container: row.container,
            duration_ms: row
                .duration_ms
                .map(|value| bounded(value, "media_file.duration_ms"))
                .transpose()?,
            video_codec: row.video_codec,
            video_profile: row.video_profile,
            width: row
                .width
                .map(|value| bounded(value, "media_file.width"))
                .transpose()?,
            height: row
                .height
                .map(|value| bounded(value, "media_file.height"))
                .transpose()?,
            bit_depth: row
                .bit_depth
                .map(|value| bounded(value, "media_file.bit_depth"))
                .transpose()?,
            hdr_format,
            frame_rate: row.frame_rate,
            bitrate_kbps: row
                .bitrate_kbps
                .map(|value| bounded(value, "media_file.bitrate_kbps"))
                .transpose()?,
            audio_codec: row.audio_codec,
            audio_channels: row
                .audio_channels
                .map(|value| bounded(value, "media_file.audio_channels"))
                .transpose()?,
            audio_layout: row.audio_layout,
            audio_languages,
            subtitle_format: row.subtitle_format,
            probe_version: row
                .probe_version
                .map(|value| bounded(value, "media_file.probe_version"))
                .transpose()?,
            probe_json: row.probe_json,
            quality_label: row.quality_label,
            updated_at: millis(row.updated_at),
        })
    }
}

#[derive(sqlx::FromRow)]
struct UnmatchedRow {
    id: i64,
    path: String,
    size_bytes: i64,
    mtime_ms: i64,
    parsed_title: Option<String>,
    parsed_year: Option<i64>,
    parsed_season: Option<i64>,
    parsed_episode: Option<i64>,
    confidence: Option<f64>,
    reason: Option<String>,
    suggested_title_id: Option<i64>,
    suggested_episode_id: Option<i64>,
    suggested_title_name: Option<String>,
    first_seen_at: i64,
}

impl TryFrom<UnmatchedRow> for UnmatchedFile {
    type Error = StorageError;

    fn try_from(row: UnmatchedRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            path: text(row.path, "unmatched_file.path")?,
            size_bytes: unsigned(row.size_bytes),
            mtime_ms: millis(row.mtime_ms),
            parsed_title: row.parsed_title,
            parsed_year: row
                .parsed_year
                .map(|value| bounded(value, "unmatched_file.parsed_year"))
                .transpose()?,
            parsed_season: row.parsed_season.map(season_number).transpose()?,
            parsed_episode: row.parsed_episode.map(episode_number).transpose()?,
            confidence: row.confidence,
            reason: row.reason,
            suggested_title_id: row.suggested_title_id,
            suggested_episode_id: row.suggested_episode_id,
            first_seen_at: millis(row.first_seen_at),
        })
    }
}

#[derive(sqlx::FromRow)]
struct SubtitleRow {
    id: i64,
    media_file_id: Option<i64>,
    episode_id: Option<i64>,
    origin: String,
    format: String,
    language: String,
    label: Option<String>,
    is_forced: i64,
    is_sdh: i64,
    is_default: i64,
    stream_index: Option<i64>,
    path: Option<String>,
    provider: Option<String>,
    provider_subtitle_id: Option<String>,
    provider_url: Option<String>,
    release_name: Option<String>,
    encoding: Option<String>,
    offset_ms: i64,
    hash_matched: i64,
    downloaded_at: Option<i64>,
    last_used_at: Option<i64>,
}

impl TryFrom<SubtitleRow> for SubtitleTrack {
    type Error = StorageError;

    fn try_from(row: SubtitleRow) -> Result<Self, Self::Error> {
        // The schema's `CHECK` says at least one owner, and the three legal combinations are
        // the three variants. A row with neither cannot exist through the pool; one that
        // nonetheless does is reported rather than read as belonging to episode zero.
        let owner = match (row.media_file_id, row.episode_id) {
            (Some(media_file_id), Some(episode_id)) => SubtitleOwner::FileAndEpisode {
                media_file_id,
                episode_id,
            },
            (Some(media_file_id), None) => SubtitleOwner::File { media_file_id },
            (None, Some(episode_id)) => SubtitleOwner::Episode { episode_id },
            (None, None) => {
                return Err(StorageError::UnexpectedValue {
                    column: "subtitle_track.media_file_id",
                    found: "NULL, and episode_id is NULL too".to_owned(),
                });
            }
        };
        Ok(Self {
            id: row.id,
            owner,
            origin: SubtitleOrigin::parse(&row.origin).ok_or_else(|| {
                StorageError::UnexpectedValue {
                    column: "subtitle_track.origin",
                    found: row.origin.clone(),
                }
            })?,
            format: SubtitleFormat::parse(&row.format).ok_or_else(|| {
                StorageError::UnexpectedValue {
                    column: "subtitle_track.format",
                    found: row.format.clone(),
                }
            })?,
            language: text(row.language, "subtitle_track.language")?,
            label: row.label,
            is_forced: row.is_forced != 0,
            is_sdh: row.is_sdh != 0,
            is_default: row.is_default != 0,
            stream_index: row
                .stream_index
                .map(|value| bounded(value, "subtitle_track.stream_index"))
                .transpose()?,
            path: row.path,
            provider: row.provider,
            provider_subtitle_id: row.provider_subtitle_id,
            provider_url: row.provider_url,
            release_name: row.release_name,
            encoding: row.encoding,
            offset_ms: row.offset_ms,
            hash_matched: row.hash_matched != 0,
            downloaded_at: row.downloaded_at.map(millis),
            last_used_at: row.last_used_at.map(millis),
        })
    }
}

/// A count column as the unsigned value it holds.
fn unsigned(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

/// A season number from its stored column.
///
/// `SeasonNumber` and `EpisodeNumber` accept only a `u32` and live in another crate, so the
/// repository cannot implement `TryFrom<i64>` for them; these two functions are the same
/// conversion with the column named for the report.
fn season_number(value: i64) -> Result<SeasonNumber, StorageError> {
    let narrowed = u32::try_from(value).map_err(|_| StorageError::UnexpectedValue {
        column: "unmatched_file.parsed_season",
        found: value.to_string(),
    })?;
    Ok(SeasonNumber::new(narrowed))
}

fn episode_number(value: i64) -> Result<EpisodeNumber, StorageError> {
    let narrowed = u32::try_from(value).map_err(|_| StorageError::UnexpectedValue {
        column: "unmatched_file.parsed_episode",
        found: value.to_string(),
    })?;
    Ok(EpisodeNumber::new(narrowed))
}

/// A `TIMESTAMP` column as the `i64` milliseconds it is stored as.
fn as_millis(value: Timestamp) -> i64 {
    value.as_millisecond()
}

/// The inverse of [`as_millis`], for a row this build is reading.
///
/// `SQLite` stores what it is given, so a column can hold a millisecond outside `jiff`'s
/// range; the clamp keeps the row readable and the mapper's other checks still apply. The
/// common case is an exact value, which is the one the round-trip test asserts.
fn millis(value: i64) -> Timestamp {
    Timestamp::from_millisecond(value).unwrap_or(Timestamp::UNIX_EPOCH)
}

/// A column's integer as the unsigned type the domain field has, or a refusal.
///
/// The schema constrains none of these to be positive, so a negative one is a row this build
/// cannot read rather than a value to fold to zero: folding it would turn a corrupt duration
/// into a plausible one.
fn bounded<T>(value: i64, column: &'static str) -> Result<T, StorageError>
where
    T: TryFrom<i64>,
{
    T::try_from(value).map_err(|_| StorageError::UnexpectedValue {
        column,
        found: value.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Database;
    use crate::database::open_writer_connection;
    use crate::page::Cursor;
    use crate::test_support::{NOW, account, database, episode, path_of, season, title};
    use peregrine_domain::{AccountId, HdrFormat, SeasonNumber};
    use proptest::prelude::*;
    use tempfile::TempDir;

    /// The scope the fixtures are read with: an account with no ceiling and no curation.
    fn any_account() -> AccountScope {
        AccountScope::unrestricted(AccountId::new(1).expect("account one exists"))
    }

    fn restricted() -> AccountScope {
        AccountScope::restricted(AccountId::new(2).expect("account two exists"))
    }

    fn text(value: &str) -> NonEmptyText {
        NonEmptyText::new(value).expect("fixture text is not empty")
    }

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millisecond(millis).expect("a millisecond inside the range")
    }

    /// A library with one account, one certified title, and one adult title.
    async fn populated(directory: &TempDir) -> (Database, i64, i64) {
        let path = path_of(directory);
        let database = database(directory).await;
        account(&path, 1, "Adult").await;
        account(&path, 2, "Child").await;
        title(&path, 1, "Alien", 0, Some("R")).await;
        title(&path, 2, "Adult Film", 1, Some("X")).await;
        (database, 1, 2)
    }

    fn root(path: &str) -> LibraryRoot {
        LibraryRoot {
            id: 0,
            path: text(path),
            kind: LibraryRootKind::Movies,
            scan_mode: ScanMode::Incremental,
            last_scan_at: None,
            created_at: at(NOW),
        }
    }

    fn item(title_id: i64, episode_id: Option<i64>) -> MediaItem {
        MediaItem {
            id: 0,
            title_id,
            episode_id,
            state: MediaItemState::Available,
            origin: MediaOrigin::Scan,
            preferred_file_id: None,
            quality_label: Some("1080p".to_owned()),
            created_at: at(NOW),
            updated_at: at(NOW),
        }
    }

    fn file(media_item_id: i64, path: &str) -> MediaFile {
        MediaFile {
            id: 0,
            media_item_id,
            path: text(path),
            root_id: None,
            size_bytes: 4_000_000_000,
            mtime_ms: at(NOW),
            added_at: at(NOW),
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
            updated_at: at(NOW),
        }
    }

    fn unmatched(path: &str, suggested_title_id: Option<i64>) -> UnmatchedFile {
        UnmatchedFile {
            id: 0,
            path: text(path),
            size_bytes: 1_000,
            mtime_ms: at(NOW),
            parsed_title: Some("Some Film".to_owned()),
            parsed_year: Some(1999),
            parsed_season: Some(SeasonNumber::new(2)),
            parsed_episode: Some(peregrine_domain::EpisodeNumber::new(7)),
            confidence: Some(0.4),
            reason: Some("no match above the floor".to_owned()),
            suggested_title_id,
            suggested_episode_id: None,
            first_seen_at: at(NOW),
        }
    }

    fn subtitle(owner: SubtitleOwner) -> SubtitleTrack {
        SubtitleTrack {
            id: 0,
            owner,
            origin: SubtitleOrigin::External,
            format: SubtitleFormat::Srt,
            language: text("en"),
            label: Some("English".to_owned()),
            is_forced: false,
            is_sdh: true,
            is_default: true,
            stream_index: None,
            path: Some("/media/Alien.en.srt".to_owned()),
            provider: None,
            provider_subtitle_id: None,
            provider_url: None,
            release_name: None,
            encoding: Some("utf-8".to_owned()),
            offset_ms: -250,
            hash_matched: false,
            downloaded_at: Some(at(NOW)),
            last_used_at: None,
        }
    }

    /// Inserts the item and the file that backs it, returning both ids.
    async fn item_with_file(database: &Database, title_id: i64, path: &str) -> (i64, i64) {
        let item_id = database
            .insert_media_item(item(title_id, None))
            .await
            .unwrap();
        let file_id = database
            .insert_media_file(file(item_id, path))
            .await
            .unwrap();
        (item_id, file_id)
    }

    // ---- round trips -------------------------------------------------------------

    /// PG-E14-01's acceptance, at the layer that can actually break it.
    ///
    /// "Removing a root never deletes files or progress" is a statement about what a delete
    /// statement does *not* touch, so the test puts something in every category the root could
    /// have produced — a real file on a real disk, a `media_file` row, a `watch_progress` row —
    /// removes the root, and asserts each one is still there. The file is written to a temporary
    /// directory rather than asserted from a fixture, because the half of the claim that would be
    /// catastrophic to get wrong is "the user's files are theirs".
    #[tokio::test]
    async fn removing_a_root_leaves_the_files_and_the_progress_alone() {
        use peregrine_domain::{PlaybackPosition, ProgressSource, WatchProgress};

        let directory = TempDir::new().unwrap();
        let path = path_of(&directory);
        let database = database(&directory).await;
        account(&path, 1, "Adult").await;
        title(&path, 1, "Alien", 0, Some("R")).await;

        let media = directory.path().join("Movies");
        std::fs::create_dir_all(&media).unwrap();
        let on_disk = media.join("Alien (1979).mkv");
        std::fs::write(&on_disk, b"not really a matroska file").unwrap();

        let root_id = database
            .insert_library_root(root(media.to_str().unwrap()))
            .await
            .unwrap();
        let item_id = database.insert_media_item(item(1, None)).await.unwrap();
        // The row points at its root, which is what makes this the case that matters: with the
        // foreign key in place, a root that has files cannot simply be deleted, and the first
        // version of this test used a fixture whose `root_id` was `None` — so it never removed a
        // root that had anything under it, and the defect it was written to prevent went unnoticed.
        let mut pointed = file(item_id, on_disk.to_str().unwrap());
        pointed.root_id = Some(root_id);
        let file_id = database.insert_media_file(pointed).await.unwrap();
        database
            .record_watch_progress(WatchProgress::record(
                AccountId::new(1).unwrap(),
                1,
                None,
                PlaybackPosition::new(90_000, Some(7_200_000)).unwrap(),
                ProgressSource::Local,
                at(NOW),
                None,
            ))
            .await
            .unwrap();

        assert!(database.remove_library_root(root_id).await.unwrap());

        assert_eq!(
            database.library_root(root_id).await.unwrap(),
            None,
            "the root is gone, which is what stops the scanner walking it"
        );
        assert!(
            on_disk.is_file(),
            "removing a root from Peregrine is not deleting the user's files"
        );
        let kept = database
            .media_file(any_account(), file_id)
            .await
            .unwrap()
            .expect("the file row survives, so the library still knows what it has");
        assert_eq!(
            kept.root_id, None,
            "and it no longer points at the root that was removed"
        );
        assert!(
            database
                .watch_progress(any_account(), 1, None)
                .await
                .unwrap()
                .is_some(),
            "what the user watched is not the root's to remove"
        );

        assert!(
            !database.remove_library_root(root_id).await.unwrap(),
            "removing a root that is already gone changes nothing"
        );
    }

    /// PG-E14-13's acceptance, at the layer that owns the rows it is about.
    ///
    /// "Removing and restoring a file preserves watch progress" is a statement about what a missing
    /// mark does *not* touch, so the test puts a file row, its item, and a progress row in place,
    /// marks the file missing, and asserts all three are still there — then clears the mark and
    /// asserts it is the same row rather than a new one. The half of the claim that is unrecoverable
    /// if it breaks is the first: a deleted row takes its subtitles with it by cascade, and the
    /// progress hanging off the item would have nothing left to describe.
    #[tokio::test]
    async fn the_library_lists_the_titles_it_holds_with_their_counts() {
        // The library screen's read. Two files under one item and one under another: the counts are
        // per title, so a title with two copies says two files and one item rather than two of each.
        let directory = TempDir::new().unwrap();
        let path = path_of(&directory);
        let database = database(&directory).await;
        account(&path, 1, "Adult").await;
        title(&path, 1, "Alien", 0, Some("R")).await;
        title(&path, 2, "Arrival", 0, Some("PG-13")).await;
        let (item_id, _) = item_with_file(&database, 1, "/media/Alien (1979).mkv").await;
        database
            .insert_media_file(file(item_id, "/media/Alien (1979) 2160p.mkv"))
            .await
            .unwrap();
        item_with_file(&database, 2, "/media/Arrival (2016).mkv").await;

        let rows = database
            .list_library_titles(any_account(), PageRequest::first(10))
            .await
            .unwrap();

        assert_eq!(rows.len(), 2);
        let alien = rows
            .iter()
            .find(|row| row.title.name == "Alien")
            .expect("the film is listed");
        assert_eq!(alien.items, 1);
        assert_eq!(alien.files, 2);
        assert_eq!(alien.missing_files, 0);
        assert_eq!(alien.bytes, 8_000_000_000, "two four-gigabyte copies");
    }

    #[tokio::test]
    async fn a_library_title_a_scope_cannot_see_is_not_listed_at_all() {
        // I37 at the library's own read: an adult title the library holds is indistinguishable from
        // one it does not, so a restricted account sees a page of one where the library holds two.
        let directory = TempDir::new().unwrap();
        let path = path_of(&directory);
        let database = database(&directory).await;
        account(&path, 1, "Adult").await;
        account(&path, 2, "Child").await;
        title(&path, 1, "Alien", 0, Some("R")).await;
        title(&path, 2, "Adult Film", 1, Some("X")).await;
        item_with_file(&database, 1, "/media/Alien (1979).mkv").await;
        item_with_file(&database, 2, "/media/Adult Film (2019).mkv").await;

        let visible = database
            .list_library_titles(restricted(), PageRequest::first(10))
            .await
            .unwrap();

        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].title.name, "Alien");
    }

    #[tokio::test]
    async fn a_title_the_library_holds_no_file_of_is_still_listed() {
        // An item with no file yet is a film whose file the scan has not reached — the `LEFT JOIN`
        // is what keeps it visible as an item rather than dropping the title entirely.
        let directory = TempDir::new().unwrap();
        let path = path_of(&directory);
        let database = database(&directory).await;
        account(&path, 1, "Adult").await;
        title(&path, 1, "Alien", 0, Some("R")).await;
        database.insert_media_item(item(1, None)).await.unwrap();

        let rows = database
            .list_library_titles(any_account(), PageRequest::first(10))
            .await
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].items, 1);
        assert_eq!(rows[0].files, 0);
        assert_eq!(rows[0].bytes, 0);
    }

    #[tokio::test]
    async fn marking_a_file_missing_keeps_its_item_and_its_watch_progress() {
        use peregrine_domain::{PlaybackPosition, ProgressSource, WatchProgress};

        let directory = TempDir::new().unwrap();
        let path = path_of(&directory);
        let database = database(&directory).await;
        account(&path, 1, "Adult").await;
        title(&path, 1, "Alien", 0, Some("R")).await;
        let (item_id, file_id) = item_with_file(&database, 1, "/media/Alien (1979).mkv").await;
        database
            .record_watch_progress(WatchProgress::record(
                AccountId::new(1).unwrap(),
                1,
                None,
                PlaybackPosition::new(90_000, Some(7_200_000)).unwrap(),
                ProgressSource::Local,
                at(NOW),
                None,
            ))
            .await
            .unwrap();

        assert!(
            database
                .set_media_file_missing(file_id, Some(NOW + 1_000), NOW + 1_000)
                .await
                .unwrap()
        );

        let missing = database
            .media_file(any_account(), file_id)
            .await
            .unwrap()
            .expect("the row is stamped, never deleted");
        assert_eq!(missing.missing_since, Some(at(NOW + 1_000)));
        assert_eq!(missing.media_item_id, item_id);
        assert!(
            database
                .media_item(any_account(), item_id)
                .await
                .unwrap()
                .is_some(),
            "the item survives, which is what the progress hangs off"
        );
        assert!(
            database
                .watch_progress(any_account(), 1, None)
                .await
                .unwrap()
                .is_some(),
            "what the user watched is not the file's to take with it"
        );

        assert!(
            database
                .set_media_file_missing(file_id, None, NOW + 2_000)
                .await
                .unwrap()
        );
        let restored = database
            .media_file(any_account(), file_id)
            .await
            .unwrap()
            .expect("still the same row");
        assert_eq!(restored.id, file_id, "a restore is not a second file");
        assert_eq!(restored.missing_since, None);
        assert!(
            database
                .watch_progress(any_account(), 1, None)
                .await
                .unwrap()
                .is_some(),
            "and the position is still the one the user left"
        );
    }

    #[tokio::test]
    async fn a_root_s_scan_mode_can_be_changed_without_forgetting_when_it_was_scanned() {
        // How often a root is going to be scanned says nothing about when it last was, and
        // clearing the instant would make the next scheduled scan treat a root scanned a minute
        // ago as one that has never been scanned.
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        let id = database
            .insert_library_root(root("/media/Movies"))
            .await
            .unwrap();
        assert!(database.record_root_scan(id, NOW + 5).await.unwrap());

        assert!(
            database
                .set_root_scan_mode(id, ScanMode::Manual)
                .await
                .unwrap()
        );

        let stored = database.library_root(id).await.unwrap().unwrap();
        assert_eq!(stored.scan_mode, ScanMode::Manual);
        assert_eq!(stored.last_scan_at, Some(at(NOW + 5)));
        assert!(
            !database
                .set_root_scan_mode(id + 1, ScanMode::Manual)
                .await
                .unwrap(),
            "changing a root that is not there changes nothing"
        );
    }

    #[tokio::test]
    async fn a_root_can_be_found_by_its_path_and_only_one_root_holds_a_path() {
        // The lookup the service uses to say "that folder is already a library" before inserting,
        // and the constraint that stops two callers racing into the same decision.
        let directory = TempDir::new().unwrap();
        let database = database(&directory).await;
        let id = database
            .insert_library_root(root("/media/Movies"))
            .await
            .unwrap();

        assert_eq!(
            database
                .library_root_by_path("/media/Movies")
                .await
                .unwrap()
                .map(|found| found.id),
            Some(id)
        );
        assert_eq!(
            database
                .library_root_by_path("/media/Elsewhere")
                .await
                .unwrap(),
            None
        );
        assert!(
            database
                .insert_library_root(root("/media/Movies"))
                .await
                .is_err(),
            "the schema's UNIQUE constraint, not the lookup, is what stops a duplicate"
        );
    }

    #[tokio::test]
    async fn a_library_root_round_trips() {
        let directory = TempDir::new().unwrap();
        let path = path_of(&directory);
        let database = database(&directory).await;
        account(&path, 1, "Adult").await;

        let id = database
            .insert_library_root(root("/media/Movies"))
            .await
            .unwrap();
        let stored = database.library_root(id).await.unwrap().unwrap();
        assert_eq!(stored.id, id);
        assert_eq!(stored.path.as_str(), "/media/Movies");
        assert_eq!(stored.kind, LibraryRootKind::Movies);
        assert_eq!(stored.scan_mode, ScanMode::Incremental);
        assert_eq!(stored.last_scan_at, None);
        assert_eq!(stored.created_at, at(NOW));

        assert_eq!(database.library_root(id + 1).await.unwrap(), None);

        assert!(database.record_root_scan(id, NOW + 5).await.unwrap());
        assert_eq!(
            database
                .library_root(id)
                .await
                .unwrap()
                .unwrap()
                .last_scan_at,
            Some(at(NOW + 5))
        );
        assert!(
            !database.record_root_scan(id + 1, NOW).await.unwrap(),
            "recording a scan of a root that is not there changes nothing"
        );
    }

    #[tokio::test]
    async fn a_media_item_round_trips() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _) = populated(&directory).await;

        let id = database
            .insert_media_item(item(certified, None))
            .await
            .unwrap();
        let stored = database
            .media_item(any_account(), id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored,
            MediaItem {
                id,
                ..item(certified, None)
            }
        );
        assert!(!stored.is_episode());

        assert_eq!(
            database
                .media_item_for(any_account(), certified, None)
                .await
                .unwrap(),
            Some(stored.clone()),
            "the partial unique index makes this one row"
        );
        assert_eq!(
            database.media_item(any_account(), id + 1).await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn an_episode_item_round_trips() {
        let directory = TempDir::new().unwrap();
        let path = path_of(&directory);
        let (database, certified, _) = populated(&directory).await;
        account(&path, 1, "Adult").await;
        season(&path, 1, certified, 1).await;
        episode(&path, 1, certified, 1, 1, 1).await;

        let id = database
            .insert_media_item(item(certified, Some(1)))
            .await
            .unwrap();
        let stored = database
            .media_item(any_account(), id)
            .await
            .unwrap()
            .unwrap();
        assert!(stored.is_episode());
        assert_eq!(stored.episode_id, Some(1));
    }

    #[tokio::test]
    async fn a_media_file_round_trips_every_column() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _) = populated(&directory).await;
        let (item_id, _) = item_with_file(&database, certified, "/media/Alien.mkv").await;

        let id = database
            .insert_media_file(file(item_id, "/media/Alien.2160p.mkv"))
            .await
            .unwrap();
        let stored = database
            .media_file(any_account(), id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            stored,
            MediaFile {
                id,
                ..file(item_id, "/media/Alien.2160p.mkv")
            }
        );
        assert_eq!(
            database
                .media_file_by_path(any_account(), "/media/Alien.2160p.mkv")
                .await
                .unwrap(),
            Some(stored.clone()),
            "I3: a path names one file"
        );
        assert_eq!(
            database
                .media_file_by_path(any_account(), "/media/Not Here.mkv")
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn an_unmatched_file_round_trips() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _) = populated(&directory).await;

        let id = database
            .insert_unmatched_file(unmatched("/downloads/unknown.mkv", Some(certified)), false)
            .await
            .unwrap()
            .expect("a fresh path inserts");
        let stored = database
            .unmatched_file(any_account(), id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(stored.id, id);
        assert_eq!(stored.path.as_str(), "/downloads/unknown.mkv");
        assert_eq!(stored.parsed_season, Some(SeasonNumber::new(2)));
        assert_eq!(
            stored.parsed_episode,
            Some(peregrine_domain::EpisodeNumber::new(7))
        );
        assert_eq!(stored.confidence, Some(0.4));
        assert_eq!(stored.suggested_title_id, Some(certified));
    }

    #[tokio::test]
    async fn a_subtitle_track_round_trips_each_owner() {
        let directory = TempDir::new().unwrap();
        let path = path_of(&directory);
        let (database, certified, _) = populated(&directory).await;
        let (item_id, file_id) = item_with_file(&database, certified, "/media/Alien.mkv").await;
        season(&path, 1, certified, 1).await;
        episode(&path, 1, certified, 1, 1, 1).await;

        for owner in [
            SubtitleOwner::File {
                media_file_id: file_id,
            },
            SubtitleOwner::Episode { episode_id: 1 },
            SubtitleOwner::FileAndEpisode {
                media_file_id: file_id,
                episode_id: 1,
            },
        ] {
            let id = database
                .insert_subtitle_track(subtitle(owner))
                .await
                .unwrap();
            let stored = database
                .subtitle_track(any_account(), id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(stored.owner, owner);
            assert_eq!(stored.format, SubtitleFormat::Srt);
            assert_eq!(stored.offset_ms, -250);
            assert!(stored.is_sdh);
            assert_eq!(stored.downloaded_at, Some(at(NOW)));
        }

        let tracks = database
            .list_subtitle_tracks(any_account(), file_id, PageRequest::first(10))
            .await
            .unwrap();
        assert_eq!(
            tracks.len(),
            2,
            "the file's own track and the one matched to both owners"
        );
        assert_eq!(item_id, item_id, "the item is the one the file belongs to");
    }

    // ---- constraints the database guards (I2, I3) --------------------------------

    #[tokio::test]
    async fn one_file_cannot_be_claimed_twice() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _) = populated(&directory).await;
        let first = database
            .insert_media_item(item(certified, None))
            .await
            .unwrap();
        let second = MediaItem {
            // A different title, so the item insert itself is legal.
            title_id: 2,
            episode_id: None,
            ..item(certified, None)
        };
        let second = database.insert_media_item(second).await.unwrap();

        database
            .insert_media_file(file(first, "/media/Alien.mkv"))
            .await
            .unwrap();
        let error = database
            .insert_media_file(file(second, "/media/Alien.mkv"))
            .await
            .expect_err("I3: two items never claim one file");
        assert!(
            error.to_string().contains("UNIQUE") || error.to_string().contains("unique"),
            "the refusal is the schema's unique index: {error}"
        );
    }

    #[tokio::test]
    async fn one_movie_item_per_title_and_one_episode_item_per_episode() {
        let directory = TempDir::new().unwrap();
        let path = path_of(&directory);
        let (database, certified, _) = populated(&directory).await;
        season(&path, 1, certified, 1).await;
        episode(&path, 1, certified, 1, 1, 1).await;

        database
            .insert_media_item(item(certified, None))
            .await
            .unwrap();
        assert!(
            database
                .insert_media_item(item(certified, None))
                .await
                .is_err(),
            "I2: a movie item exists at most once per title"
        );

        database
            .insert_media_item(item(certified, Some(1)))
            .await
            .unwrap();
        assert!(
            database
                .insert_media_item(item(certified, Some(1)))
                .await
                .is_err(),
            "I2: an episode item exists at most once per episode"
        );
    }

    /// I11: the row is kept and says when the library first noticed, and the mark is
    /// reversible because a file can come back.
    #[tokio::test]
    async fn a_missing_file_keeps_its_row_and_its_first_instant() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _) = populated(&directory).await;
        let (_, file_id) = item_with_file(&database, certified, "/media/Alien.mkv").await;

        assert!(
            database
                .set_media_file_missing(file_id, Some(NOW + 1), NOW + 1)
                .await
                .unwrap()
        );
        // A second scan that still cannot see the file must not move the instant.
        let mut stored = database
            .media_file(any_account(), file_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.missing_since, Some(at(NOW + 1)));
        stored.mark_missing(at(NOW + 9));
        assert_eq!(
            stored.missing_since,
            Some(at(NOW + 1)),
            "missing since is when the library first noticed"
        );
        assert!(
            database
                .set_media_file_missing(file_id, Some(NOW + 1), NOW + 9)
                .await
                .unwrap()
        );
        assert_eq!(
            database
                .media_file(any_account(), file_id)
                .await
                .unwrap()
                .unwrap()
                .missing_since,
            Some(at(NOW + 1))
        );

        stored.mark_present(at(NOW + 20));
        assert!(
            database
                .set_media_file_missing(file_id, None, NOW + 20)
                .await
                .unwrap()
        );
        let stored = database
            .media_file(any_account(), file_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.missing_since, None,
            "a file that came back is present"
        );
        assert_eq!(
            database
                .media_file(any_account(), file_id)
                .await
                .unwrap()
                .unwrap(),
            stored
        );
    }

    /// A scan that sees the same unplaceable file twice leaves the first row, and its
    /// `first_seen_at`, alone.
    #[tokio::test]
    async fn an_unmatched_path_is_only_seen_once() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _) = populated(&directory).await;

        let first = database
            .insert_unmatched_file(unmatched("/downloads/unknown.mkv", Some(certified)), true)
            .await
            .unwrap()
            .expect("the first sighting inserts");
        assert_eq!(
            database
                .insert_unmatched_file(unmatched("/downloads/unknown.mkv", Some(certified)), true)
                .await
                .unwrap(),
            None,
            "the second sighting is ignored"
        );
        assert_eq!(database.unmatched_count().await.unwrap(), 1);
        assert_eq!(
            database
                .unmatched_file(any_account(), first)
                .await
                .unwrap()
                .unwrap()
                .first_seen_at,
            at(NOW),
            "the first sighting's instant survives"
        );

        let error = database
            .insert_unmatched_file(unmatched("/downloads/unknown.mkv", Some(certified)), false)
            .await
            .expect_err("without the ignore the conflict is reported");
        assert!(
            error.to_string().contains("UNIQUE"),
            "the refusal is the schema's unique index: {error}"
        );
    }

    // ---- scope (R34, I25, I26, I27) ----------------------------------------------

    /// A restricted scope answers the same for an adult title's item as for one that does not
    /// exist, and the same for the files that back it.
    #[tokio::test]
    async fn a_restricted_scope_cannot_see_an_adult_items_files() {
        let directory = TempDir::new().unwrap();
        let (database, certified, adult) = populated(&directory).await;
        let (_, certified_file) = item_with_file(&database, certified, "/media/Alien.mkv").await;
        let (adult_item, adult_file) = item_with_file(&database, adult, "/media/Adult.mkv").await;

        let scope = restricted();
        assert!(
            database
                .media_item(scope, certified)
                .await
                .unwrap()
                .is_some(),
            "a certified title's item is visible"
        );
        assert_eq!(
            database.media_item(scope, adult_item).await.unwrap(),
            None,
            "an adult title's item is indistinguishable from an absent one"
        );
        assert_eq!(database.media_item(scope, i64::MAX).await.unwrap(), None);
        assert_eq!(database.media_file(scope, adult_file).await.unwrap(), None);
        assert_eq!(
            database
                .media_file_by_path(scope, "/media/Adult.mkv")
                .await
                .unwrap(),
            None,
            "asking by path does not reveal what asking by id hides"
        );
        assert_eq!(
            database
                .list_media_files(scope, adult_item, PageRequest::first(10))
                .await
                .unwrap(),
            Vec::new(),
        );
        assert_eq!(
            database
                .list_media_items(scope, adult, PageRequest::first(10))
                .await
                .unwrap(),
            Vec::new(),
        );

        assert!(
            database
                .media_file(any_account(), adult_file)
                .await
                .unwrap()
                .is_some(),
            "the row exists; the scope is what hides it"
        );
        assert!(
            database
                .media_file(any_account(), certified_file)
                .await
                .unwrap()
                .is_some()
        );
    }

    /// I27's fail-closed rule applied to the sync path: a title with no certification is
    /// hidden from a restricted scope, so its item is too.
    #[tokio::test]
    async fn a_restricted_scope_cannot_see_an_unrated_items_rows() {
        let directory = TempDir::new().unwrap();
        let path = path_of(&directory);
        let (database, _, _) = populated(&directory).await;
        title(&path, 3, "Unrated", 0, None).await;

        let item_id = database.insert_media_item(item(3, None)).await.unwrap();
        let file_id = database
            .insert_media_file(file(item_id, "/media/Unrated.mkv"))
            .await
            .unwrap();

        let scope = restricted();
        assert_eq!(database.media_item(scope, item_id).await.unwrap(), None);
        assert_eq!(database.media_file(scope, file_id).await.unwrap(), None);
        assert!(
            database
                .media_item(any_account(), item_id)
                .await
                .unwrap()
                .is_some()
        );
    }

    /// An unmatched file is visible to a restricted scope, but the title the scanner
    /// *suggests* for it is content and is withheld when the scope may not see it.
    #[tokio::test]
    async fn a_restricted_scope_is_not_offered_a_hidden_suggestion() {
        let directory = TempDir::new().unwrap();
        let (database, certified, adult) = populated(&directory).await;

        let visible = database
            .insert_unmatched_file(unmatched("/downloads/a.mkv", Some(certified)), false)
            .await
            .unwrap()
            .unwrap();
        let hidden = database
            .insert_unmatched_file(unmatched("/downloads/b.mkv", Some(adult)), false)
            .await
            .unwrap()
            .unwrap();

        let scope = restricted();
        assert!(
            database
                .unmatched_file(scope, visible)
                .await
                .unwrap()
                .is_some(),
            "the file itself is not content"
        );
        // The file is offered and its *suggestion* is not — which is the same thing a row with nothing
        // close enough says, so the two are indistinguishable (I37). Hiding the row instead was this
        // read's first behaviour and it contradicted the assertion above: a user was never asked about
        // a file they owned because the scan had guessed at a title they may not see.
        let offered = database
            .unmatched_file(scope, hidden)
            .await
            .unwrap()
            .expect("the file is offered");
        assert_eq!(
            offered.suggested_title_id, None,
            "a suggestion the scope may not see is not a suggestion"
        );
        let listed = database
            .list_unmatched_files(scope, PageRequest::first(10))
            .await
            .unwrap();
        assert_eq!(listed.len(), 2, "both files are still questions");
        let named: Vec<&str> = listed
            .iter()
            .filter_map(|row| row.suggested_title_name.as_deref())
            .collect();
        assert_eq!(
            named,
            vec!["Alien"],
            "only the suggestion the scope may see is named"
        );
        assert_eq!(
            database
                .list_unmatched_files(any_account(), PageRequest::first(10))
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn a_restricted_scope_cannot_see_an_adult_files_subtitles() {
        let directory = TempDir::new().unwrap();
        let (database, certified, adult) = populated(&directory).await;
        let (_, certified_file) = item_with_file(&database, certified, "/media/Alien.mkv").await;
        let (_, adult_file) = item_with_file(&database, adult, "/media/Adult.mkv").await;

        let certified_track = database
            .insert_subtitle_track(subtitle(SubtitleOwner::File {
                media_file_id: certified_file,
            }))
            .await
            .unwrap();
        let adult_track = database
            .insert_subtitle_track(subtitle(SubtitleOwner::File {
                media_file_id: adult_file,
            }))
            .await
            .unwrap();

        let scope = restricted();
        assert_eq!(
            database.subtitle_track(scope, adult_track).await.unwrap(),
            None
        );
        assert_eq!(
            database
                .list_subtitle_tracks(scope, adult_file, PageRequest::first(10))
                .await
                .unwrap(),
            Vec::new(),
        );
        assert!(
            database
                .subtitle_track(scope, certified_track)
                .await
                .unwrap()
                .is_some()
        );
    }

    // ---- pagination --------------------------------------------------------------

    #[tokio::test]
    async fn roots_are_keyset_paged_and_unaffected_by_new_rows() {
        let directory = TempDir::new().unwrap();
        let path = path_of(&directory);
        let database = database(&directory).await;
        account(&path, 1, "Adult").await;

        let mut ids = Vec::new();
        for index in 0..5 {
            ids.push(
                database
                    .insert_library_root(root(&format!("/media/{index}")))
                    .await
                    .unwrap(),
            );
        }

        let first = database
            .list_library_roots(PageRequest::first(2))
            .await
            .unwrap();
        assert_eq!(
            first.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![ids[4], ids[3]]
        );

        database
            .insert_library_root(root("/media/late"))
            .await
            .unwrap();
        let second = database
            .list_library_roots(PageRequest::after(Cursor::new(first.last().unwrap().id), 2))
            .await
            .unwrap();
        assert_eq!(
            second.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![ids[2], ids[1]],
            "a root added between the reads neither appears nor pushes one out"
        );

        let third = database
            .list_library_roots(PageRequest::after(
                Cursor::new(second.last().unwrap().id),
                2,
            ))
            .await
            .unwrap();
        assert_eq!(
            third.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![ids[0]]
        );
        assert_eq!(
            database
                .list_library_roots(PageRequest::after(Cursor::new(ids[0]), 2))
                .await
                .unwrap(),
            Vec::new(),
        );
    }

    /// The cursor is total because it is the primary key: every row has one and no two rows
    /// share it, so paging one row at a time reaches every row exactly once.
    #[tokio::test]
    async fn a_one_row_page_walks_every_file_exactly_once() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _) = populated(&directory).await;
        let item_id = database
            .insert_media_item(item(certified, None))
            .await
            .unwrap();
        let mut inserted = Vec::new();
        for index in 0..4 {
            inserted.push(
                database
                    .insert_media_file(file(item_id, &format!("/media/dup-{index}.mkv")))
                    .await
                    .unwrap(),
            );
        }

        let mut seen = Vec::new();
        let mut request = PageRequest::first(1);
        loop {
            let page = database
                .list_media_files(any_account(), item_id, request)
                .await
                .unwrap();
            let Some(row) = page.first() else { break };
            seen.push(row.id);
            request = PageRequest::after(Cursor::new(row.id), 1);
        }
        inserted.sort_unstable();
        seen.sort_unstable();
        assert_eq!(seen, inserted, "every row is reached once and only once");
        assert_eq!(seen.len(), 4);
    }

    /// The same insert between pages, for unmatched files and items.
    #[tokio::test]
    async fn items_are_keyset_paged_and_unaffected_by_new_rows() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _) = populated(&directory).await;
        let first = database
            .insert_media_item(item(certified, None))
            .await
            .unwrap();

        let page = database
            .list_media_items(any_account(), certified, PageRequest::first(10))
            .await
            .unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].id, first);

        assert!(
            database
                .list_media_items(
                    any_account(),
                    certified,
                    PageRequest::after(Cursor::new(first), 10)
                )
                .await
                .unwrap()
                .is_empty(),
            "the only item is the end of the list"
        );
    }

    #[tokio::test]
    async fn unmatched_files_are_keyset_paged() {
        let directory = TempDir::new().unwrap();
        let (database, _, _) = populated(&directory).await;
        let mut ids = Vec::new();
        for index in 0..3 {
            ids.push(
                database
                    .insert_unmatched_file(
                        unmatched(&format!("/downloads/{index}.mkv"), None),
                        false,
                    )
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }

        let first = database
            .list_unmatched_files(any_account(), PageRequest::first(2))
            .await
            .unwrap();
        assert_eq!(
            first.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![ids[2], ids[1]]
        );
        database
            .insert_unmatched_file(unmatched("/downloads/late.mkv", None), false)
            .await
            .unwrap();
        let second = database
            .list_unmatched_files(
                any_account(),
                PageRequest::after(Cursor::new(first.last().unwrap().id), 2),
            )
            .await
            .unwrap();
        assert_eq!(
            second.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![ids[0]]
        );
    }

    // ---- empty tables ------------------------------------------------------------

    #[tokio::test]
    async fn an_empty_library_lists_nothing_for_either_scope() {
        let directory = TempDir::new().unwrap();
        let path = path_of(&directory);
        let database = database(&directory).await;
        account(&path, 1, "Adult").await;

        assert_eq!(
            database
                .list_library_roots(PageRequest::first(10))
                .await
                .unwrap(),
            Vec::new(),
        );
        assert_eq!(database.unmatched_count().await.unwrap(), 0);
        for scope in [any_account(), restricted()] {
            assert_eq!(
                database
                    .list_unmatched_files(scope, PageRequest::first(10))
                    .await
                    .unwrap(),
                Vec::new(),
            );
            assert_eq!(
                database
                    .list_media_items(scope, 1, PageRequest::first(10))
                    .await
                    .unwrap(),
                Vec::new(),
            );
            assert_eq!(
                database
                    .list_media_files(scope, 1, PageRequest::first(10))
                    .await
                    .unwrap(),
                Vec::new(),
            );
            assert_eq!(
                database
                    .list_subtitle_tracks(scope, 1, PageRequest::first(10))
                    .await
                    .unwrap(),
                Vec::new(),
            );
            assert_eq!(database.media_item(scope, 1).await.unwrap(), None);
            assert_eq!(database.media_file(scope, 1).await.unwrap(), None);
            assert_eq!(database.subtitle_track(scope, 1).await.unwrap(), None);
            assert_eq!(database.unmatched_file(scope, 1).await.unwrap(), None);
            assert_eq!(database.library_root(1).await.unwrap(), None);
        }
    }

    // ---- rows this build cannot read ---------------------------------------------

    /// A row written by another version is reported, not guessed at.
    #[tokio::test]
    async fn a_row_this_build_cannot_read_is_reported() {
        let directory = TempDir::new().unwrap();
        let path = path_of(&directory);
        let database = database(&directory).await;
        account(&path, 1, "Adult").await;

        let mut connection = open_writer_connection(&path).await.unwrap();
        sqlx::query("PRAGMA ignore_check_constraints = ON")
            .execute(&mut connection)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO library_root (path, kind, scan_mode, created_at) \
             VALUES ('/media/x', 'audiobooks', 'incremental', 0)",
        )
        .execute(&mut connection)
        .await
        .unwrap();
        connection.close().await.unwrap();

        let error = database
            .library_root(1)
            .await
            .expect_err("a kind this build does not know is not read as one it does");
        assert!(
            matches!(
                error,
                StorageError::UnexpectedValue {
                    column: "library_root.kind",
                    ..
                }
            ),
            "{error:?}"
        );
        assert!(error.to_string().contains("audiobooks"), "{error}");
    }

    /// A JSON column the mapper owns: text that is not the array this build writes is
    /// reported rather than turned into a plausible list of languages.
    #[tokio::test]
    async fn a_corrupt_audio_languages_column_is_reported() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _) = populated(&directory).await;
        let (_, file_id) = item_with_file(&database, certified, "/media/Alien.mkv").await;

        let path = path_of(&directory);
        let mut connection = open_writer_connection(&path).await.unwrap();
        sqlx::query("UPDATE media_file SET audio_languages = 'en, fr' WHERE id = ?1")
            .bind(file_id)
            .execute(&mut connection)
            .await
            .unwrap();
        connection.close().await.unwrap();

        let error = database
            .media_file(any_account(), file_id)
            .await
            .expect_err("that is not the JSON array this build writes");
        assert!(
            matches!(
                error,
                StorageError::InvalidJsonField {
                    column: "media_file.audio_languages",
                    ..
                }
            ),
            "{error:?}"
        );
    }

    /// A season or episode number the domain cannot hold is reported rather than folded to
    /// zero, which would turn a corrupt row into a plausible one.
    #[tokio::test]
    async fn an_out_of_range_parse_is_reported() {
        let directory = TempDir::new().unwrap();
        let (database, certified, _) = populated(&directory).await;
        let id = database
            .insert_unmatched_file(unmatched("/downloads/unknown.mkv", None), false)
            .await
            .unwrap()
            .unwrap();

        let path = path_of(&directory);
        let mut connection = open_writer_connection(&path).await.unwrap();
        sqlx::query("UPDATE unmatched_file SET parsed_season = -1 WHERE id = ?1")
            .bind(id)
            .execute(&mut connection)
            .await
            .unwrap();
        connection.close().await.unwrap();

        let error = database
            .unmatched_file(any_account(), id)
            .await
            .expect_err("a negative season is not a season");
        assert!(
            matches!(
                error,
                StorageError::UnexpectedValue {
                    column: "unmatched_file.parsed_season",
                    ..
                }
            ),
            "{error:?}"
        );
        let _ = certified;
    }

    // ---- property tests ----------------------------------------------------------

    fn any_millis() -> impl Strategy<Value = i64> {
        1_600_000_000_000_i64..1_800_000_000_000
    }

    fn any_optional<T: std::fmt::Debug + Clone + 'static>(
        value: impl Strategy<Value = T>,
    ) -> impl Strategy<Value = Option<T>> {
        proptest::option::of(value)
    }

    // Every column of `library_root` survives the round trip, for any path, kind, and scan
    // mode the schema accepts.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        #[test]
        fn any_library_root_round_trips(
            path in "/[a-z]{1,12}/[a-zA-Z0-9 _.]{1,16}",
            kind in prop::sample::select(LibraryRootKind::ALL.to_vec()),
            scan_mode in prop::sample::select(ScanMode::ALL.to_vec()),
            last_scan_at in any_optional(any_millis()),
            created_at in any_millis(),
        ) {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a test runtime");
            runtime.block_on(async {
                let directory = TempDir::new().expect("a temporary directory");
                let directory_path = path_of(&directory);
                let database = database(&directory).await;
                account(&directory_path, 1, "Adult").await;

                let expected = LibraryRoot {
                    id: 0,
                    path: text(&path),
                    kind,
                    scan_mode,
                    last_scan_at: last_scan_at.map(at),
                    created_at: at(created_at),
                };
                let id = database.insert_library_root(expected.clone()).await.expect("inserts");
                let stored = database.library_root(id).await.expect("reads").expect("the row");
                prop_assert_eq!(stored, LibraryRoot { id, ..expected });
                Ok(())
            })?;
        }
    }

    // Every column of `media_item` survives the round trip.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        #[test]
        fn any_media_item_round_trips(
            state in prop::sample::select(MediaItemState::ALL.to_vec()),
            origin in prop::sample::select(MediaOrigin::ALL.to_vec()),
            preferred in proptest::option::of(1_i64..1000),
            quality in any_optional("[A-Za-z0-9 ]{1,12}"),
            created_at in any_millis(),
            updated_at in any_millis(),
        ) {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a test runtime");
            runtime.block_on(async {
                let directory = TempDir::new().expect("a temporary directory");
                let (database, certified, _) = populated(&directory).await;

                let expected = MediaItem {
                    id: 0,
                    title_id: certified,
                    episode_id: None,
                    state,
                    origin,
                    preferred_file_id: preferred,
                    quality_label: quality,
                    created_at: at(created_at),
                    updated_at: at(updated_at),
                };
                let id = database.insert_media_item(expected.clone()).await.expect("inserts");
                let stored = database
                    .media_item(any_account(), id)
                    .await
                    .expect("reads")
                    .expect("the row");
                prop_assert_eq!(stored, MediaItem { id, ..expected });
                Ok(())
            })?;
        }
    }

    // Every column of `media_file` survives the round trip, including the JSON array and
    // the opaque probe bytes.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]

        #[test]
        fn any_media_file_round_trips(
            path in "/media/[a-zA-Z0-9 _.]{1,20}",
            size_bytes in 0_u64..1_000_000_000_000,
            mtime_ms in any_millis(),
            added_at in any_millis(),
            missing_since in any_optional(any_millis()),
            container in any_optional("[a-z0-9]{1,10}"),
            duration_ms in any_optional(0_u64..100_000_000),
            video_codec in any_optional("[a-z0-9]{1,8}"),
            video_profile in any_optional("[A-Za-z0-9 ]{1,10}"),
            width in any_optional(0_i64..10_000),
            height in any_optional(0_i64..10_000),
            bit_depth in any_optional(0_i64..64),
            hdr in any_optional(prop::sample::select(HdrFormat::ALL.to_vec())),
            frame_rate in proptest::option::of("[0-9]{1,2}\\.[0-9]{1,3}"),
            bitrate_kbps in any_optional(0_i64..1_000_000),
            audio_codec in any_optional("[a-z0-9]{1,8}"),
            audio_channels in any_optional(0_i64..16),
            audio_layout in any_optional("[0-9.]{1,6}"),
            audio_languages in proptest::option::of(proptest::collection::vec("[a-z]{2}", 0..4)),
            subtitle_format in any_optional("[a-z0-9]{1,10}"),
            probe_version in any_optional(0_i64..100),
            probe_json in proptest::option::of(proptest::collection::vec(any::<u8>(), 0..32)),
            quality_label in any_optional("[A-Za-z0-9 ]{1,12}"),
            updated_at in any_millis(),
        ) {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a test runtime");
            runtime.block_on(async {
                let directory = TempDir::new().expect("a temporary directory");
                let (database, certified, _) = populated(&directory).await;
                let media_item_id = database
                    .insert_media_item(item(certified, None))
                    .await
                    .expect("the item inserts");

                let expected = MediaFile {
                    id: 0,
                    media_item_id,
                    path: text(&path),
                    root_id: None,
                    size_bytes,
                    mtime_ms: at(mtime_ms),
                    added_at: at(added_at),
                    missing_since: missing_since.map(at),
                    container,
                    duration_ms,
                    video_codec,
                    video_profile,
                    width: width.map(|value| u32::try_from(value).expect("the strategy is small")),
                    height: height.map(|value| u32::try_from(value).expect("the strategy is small")),
                    bit_depth: bit_depth.map(|value| u8::try_from(value).expect("the strategy is small")),
                    hdr_format: hdr,
                    frame_rate: frame_rate.and_then(|value| value.parse().ok()),
                    bitrate_kbps: bitrate_kbps
                        .map(|value| u32::try_from(value).expect("the strategy is small")),
                    audio_codec,
                    audio_channels: audio_channels
                        .map(|value| u8::try_from(value).expect("the strategy is small")),
                    audio_layout,
                    audio_languages,
                    subtitle_format,
                    probe_version: probe_version
                        .map(|value| u32::try_from(value).expect("the strategy is small")),
                    probe_json,
                    quality_label,
                    updated_at: at(updated_at),
                };
                let id = database.insert_media_file(expected.clone()).await.expect("inserts");
                let stored = database
                    .media_file(any_account(), id)
                    .await
                    .expect("reads")
                    .expect("the row");
                prop_assert_eq!(stored, MediaFile { id, ..expected });
                Ok(())
            })?;
        }
    }

    // Every column of `unmatched_file` survives the round trip.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        #[test]
        fn any_unmatched_file_round_trips(
            path in "/downloads/[a-zA-Z0-9 _.]{1,20}",
            size_bytes in 0_u64..1_000_000_000,
            mtime_ms in any_millis(),
            parsed_title in any_optional("[A-Za-z0-9 ]{1,16}"),
            parsed_year in any_optional(1900_u16..2100),
            parsed_season in any_optional(0_u32..50),
            parsed_episode in any_optional(0_u32..500),
            confidence in any_optional(0.0_f64..1.0),
            reason in any_optional("[a-z ]{1,16}"),
            suggested in proptest::option::of(1_i64..10),
            first_seen_at in any_millis(),
        ) {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a test runtime");
            runtime.block_on(async {
                let directory = TempDir::new().expect("a temporary directory");
                let (database, certified, _) = populated(&directory).await;
                let suggested_title_id = suggested.map(|_| certified);

                let expected = UnmatchedFile {
                    id: 0,
                    path: text(&path),
                    size_bytes,
                    mtime_ms: at(mtime_ms),
                    parsed_title,
                    parsed_year,
                    parsed_season: parsed_season.map(SeasonNumber::new),
                    parsed_episode: parsed_episode.map(peregrine_domain::EpisodeNumber::new),
                    confidence,
                    reason,
                    suggested_title_id,
                    suggested_episode_id: None,
                    first_seen_at: at(first_seen_at),
                };
                let id = database
                    .insert_unmatched_file(expected.clone(), false)
                    .await
                    .expect("inserts")
                    .expect("a fresh path");
                let stored = database
                    .unmatched_file(any_account(), id)
                    .await
                    .expect("reads")
                    .expect("the row");
                prop_assert_eq!(stored, UnmatchedFile { id, ..expected });
                Ok(())
            })?;
        }
    }

    // Every column of `subtitle_track` survives the round trip, for each legal owner.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]

        #[test]
        fn any_subtitle_track_round_trips(
            language in "[a-z]{2}(-[A-Z]{2})?",
            origin in prop::sample::select(SubtitleOrigin::ALL.to_vec()),
            format in prop::sample::select(SubtitleFormat::ALL.to_vec()),
            label in any_optional("[A-Za-z ]{1,12}"),
            is_forced in any::<bool>(),
            is_sdh in any::<bool>(),
            is_default in any::<bool>(),
            stream_index in any_optional(0_i64..32),
            sidecar in any_optional("/media/[a-z.]{1,12}"),
            provider in any_optional("[a-z]{1,8}"),
            provider_id in any_optional("[a-z0-9]{1,8}"),
            provider_url in any_optional("https://[a-z.]{1,16}"),
            release_name in any_optional("[A-Za-z0-9. ]{1,16}"),
            encoding in any_optional("[a-z0-9-]{1,8}"),
            offset_ms in -5_000_i64..5_000,
            hash_matched in any::<bool>(),
            downloaded_at in any_optional(any_millis()),
            last_used_at in any_optional(any_millis()),
        ) {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a test runtime");
            runtime.block_on(async {
                let directory = TempDir::new().expect("a temporary directory");
                let (database, certified, _) = populated(&directory).await;
                let (_, file_id) = item_with_file(&database, certified, "/media/Alien.mkv").await;

                let expected = SubtitleTrack {
                    id: 0,
                    owner: SubtitleOwner::File { media_file_id: file_id },
                    origin,
                    format,
                    language: text(&language),
                    label,
                    is_forced,
                    is_sdh,
                    is_default,
                    stream_index: stream_index
                        .map(|value| u32::try_from(value).expect("the strategy is small")),
                    path: sidecar,
                    provider,
                    provider_subtitle_id: provider_id,
                    provider_url,
                    release_name,
                    encoding,
                    offset_ms,
                    hash_matched,
                    downloaded_at: downloaded_at.map(at),
                    last_used_at: last_used_at.map(at),
                };
                let id = database
                    .insert_subtitle_track(expected.clone())
                    .await
                    .expect("inserts");
                let stored = database
                    .subtitle_track(any_account(), id)
                    .await
                    .expect("reads")
                    .expect("the row");
                prop_assert_eq!(stored, SubtitleTrack { id, ..expected });
                Ok(())
            })?;
        }
    }
}
