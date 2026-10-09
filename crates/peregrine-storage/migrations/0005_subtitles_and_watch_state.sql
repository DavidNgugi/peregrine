-- Migration 0005 — subtitles, watch progress, the watchlist, and smart collections.
--
-- Transcribed from docs/DATA_MODEL.md §2.5 and §2.6. Journaled by `cargo xtask check-migrations`;
-- immutable after merge, so a schema change is a new migration, never an edit to this one.

CREATE TABLE subtitle_track (
  id                  INTEGER PRIMARY KEY,
  media_file_id       INTEGER REFERENCES media_file(id) ON DELETE CASCADE,
  episode_id          INTEGER REFERENCES episode(id) ON DELETE CASCADE,
  origin              TEXT NOT NULL CHECK (origin IN ('embedded','external','downloaded','generated')),
  format              TEXT NOT NULL CHECK (format IN ('srt','ass','ssa','vtt','sub','pgs','vobsub')),
  language            TEXT NOT NULL,
  label               TEXT,
  is_forced           INTEGER NOT NULL DEFAULT 0 CHECK (is_forced IN (0,1)),
  is_sdh              INTEGER NOT NULL DEFAULT 0 CHECK (is_sdh IN (0,1)),
  is_default          INTEGER NOT NULL DEFAULT 0 CHECK (is_default IN (0,1)),
  stream_index        INTEGER,
  path                TEXT,
  provider            TEXT,
  provider_subtitle_id TEXT,
  provider_url        TEXT,
  release_name        TEXT,
  encoding            TEXT,
  offset_ms           INTEGER NOT NULL DEFAULT 0,
  hash_matched        INTEGER NOT NULL DEFAULT 0 CHECK (hash_matched IN (0,1)),
  downloaded_at       INTEGER,
  last_used_at        INTEGER,
  CHECK (media_file_id IS NOT NULL OR episode_id IS NOT NULL)
);
CREATE INDEX subtitle_track_file_idx    ON subtitle_track (media_file_id, language);
CREATE INDEX subtitle_track_episode_idx ON subtitle_track (episode_id, language);

CREATE TABLE subtitle_download (
  id           INTEGER PRIMARY KEY,
  media_file_id INTEGER REFERENCES media_file(id) ON DELETE CASCADE,
  episode_id   INTEGER REFERENCES episode(id) ON DELETE CASCADE,
  provider     TEXT NOT NULL,
  provider_subtitle_id TEXT NOT NULL,
  language     TEXT NOT NULL,
  score        REAL NOT NULL,
  score_reasons TEXT,
  created_at   INTEGER NOT NULL
);
CREATE UNIQUE INDEX subtitle_download_uq ON subtitle_download (
  provider, provider_subtitle_id, COALESCE(media_file_id, -1), COALESCE(episode_id, -1)
);

CREATE TABLE watch_progress (
  id           INTEGER PRIMARY KEY,
  account_id   INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
  title_id     INTEGER NOT NULL REFERENCES title(id) ON DELETE CASCADE,
  episode_id   INTEGER REFERENCES episode(id) ON DELETE CASCADE,
  entity_key   TEXT GENERATED ALWAYS AS (
                 CASE WHEN episode_id IS NULL THEN 't' ELSE 'e' END
                 || CASE WHEN episode_id IS NULL THEN title_id ELSE episode_id END
               ) STORED,
  position_ms  INTEGER NOT NULL DEFAULT 0,
  duration_ms  INTEGER,
  percent      REAL NOT NULL DEFAULT 0,
  watched      INTEGER NOT NULL DEFAULT 0 CHECK (watched IN (0,1)),
  watched_at   INTEGER,
  play_count   INTEGER NOT NULL DEFAULT 0,
  updated_at   INTEGER NOT NULL,
  source       TEXT NOT NULL DEFAULT 'local' CHECK (source IN ('local','trakt','manual')),
  device_label TEXT,
  UNIQUE (account_id, entity_key)
);
CREATE INDEX watch_progress_continue_idx
  ON watch_progress (account_id, updated_at DESC)
  WHERE watched = 0 AND percent BETWEEN 0.02 AND 0.95;

CREATE TABLE watchlist_entry (
  account_id INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
  title_id   INTEGER NOT NULL REFERENCES title(id) ON DELETE CASCADE,
  added_at   INTEGER NOT NULL,
  position   INTEGER,
  note       TEXT,
  auto_acquire INTEGER NOT NULL DEFAULT 0 CHECK (auto_acquire IN (0,1)),
  PRIMARY KEY (account_id, title_id)
);
CREATE INDEX watchlist_added_idx ON watchlist_entry (account_id, added_at DESC);

CREATE TABLE smart_collection (
  id          INTEGER PRIMARY KEY,
  name        TEXT NOT NULL,
  icon        TEXT,
  query_json  TEXT NOT NULL,
  sort        TEXT NOT NULL,
  pinned      INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0,1)),
  created_at  INTEGER NOT NULL
);
