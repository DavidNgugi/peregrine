-- Migration 0004 — the library: media items, their files, library roots, and the files
-- the scanner could not confidently attribute.
--
-- Transcribed from docs/DATA_MODEL.md §2.4. Journaled by `cargo xtask check-migrations`;
-- immutable after merge, so a schema change is a new migration, never an edit to this one.

CREATE TABLE media_item (
  id            INTEGER PRIMARY KEY,
  title_id      INTEGER NOT NULL REFERENCES title(id) ON DELETE CASCADE,
  episode_id    INTEGER REFERENCES episode(id) ON DELETE CASCADE,
  state         TEXT NOT NULL CHECK (state IN ('wanted','downloading','available','partial','failed')),
  origin        TEXT NOT NULL CHECK (origin IN ('scan','torrent','manual')),
  preferred_file_id INTEGER,
  quality_label TEXT,
  created_at    INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL
);

CREATE UNIQUE INDEX media_item_movie_uq  ON media_item (title_id) WHERE episode_id IS NULL;
CREATE UNIQUE INDEX media_item_episode_uq ON media_item (episode_id) WHERE episode_id IS NOT NULL;

CREATE TABLE media_file (
  id              INTEGER PRIMARY KEY,
  media_item_id   INTEGER NOT NULL REFERENCES media_item(id) ON DELETE CASCADE,
  path            TEXT NOT NULL UNIQUE,
  root_id         INTEGER REFERENCES library_root(id),
  size_bytes      INTEGER NOT NULL,
  mtime_ms        INTEGER NOT NULL,
  added_at        INTEGER NOT NULL,
  missing_since   INTEGER,
  container       TEXT,
  duration_ms     INTEGER,
  video_codec     TEXT,
  video_account   TEXT,
  width           INTEGER,
  height          INTEGER,
  bit_depth       INTEGER,
  hdr_format      TEXT CHECK (hdr_format IN ('sdr','hdr10','hdr10_plus','hlg','dolby_vision')),
  frame_rate      REAL,
  bitrate_kbps    INTEGER,
  audio_codec     TEXT,
  audio_channels  INTEGER,
  audio_layout    TEXT,
  audio_languages TEXT,
  subtitle_format TEXT,
  probe_version   INTEGER,
  probe_json      TEXT,
  quality_label   TEXT,
  updated_at      INTEGER NOT NULL
);
CREATE INDEX media_file_item_idx    ON media_file (media_item_id);
CREATE INDEX media_file_missing_idx ON media_file (missing_since) WHERE missing_since IS NOT NULL;

CREATE TABLE library_root (
  id          INTEGER PRIMARY KEY,
  path        TEXT NOT NULL UNIQUE,
  kind        TEXT NOT NULL CHECK (kind IN ('movies','shows','mixed','downloads')),
  scan_mode   TEXT NOT NULL DEFAULT 'incremental' CHECK (scan_mode IN ('incremental','scheduled','manual')),
  last_scan_at INTEGER,
  created_at  INTEGER NOT NULL
);

CREATE TABLE unmatched_file (
  id            INTEGER PRIMARY KEY,
  path          TEXT NOT NULL UNIQUE,
  size_bytes    INTEGER NOT NULL,
  mtime_ms      INTEGER NOT NULL,
  parsed_title  TEXT,
  parsed_year   INTEGER,
  parsed_season INTEGER,
  parsed_episode INTEGER,
  confidence    REAL,
  reason        TEXT,
  suggested_title_id   INTEGER REFERENCES title(id),
  suggested_episode_id INTEGER REFERENCES episode(id),
  first_seen_at INTEGER NOT NULL
);
