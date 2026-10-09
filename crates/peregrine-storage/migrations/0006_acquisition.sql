-- Migration 0006 — acquisition: quality profiles, indexers, download jobs, the files each
-- job holds, and the release-decision audit trail.
--
-- Transcribed from docs/DATA_MODEL.md §2.7. `job` and `event_log`, which §2.8 also
-- documents, are already in 0001 because PG-E3-03 landed the schedule they describe.
--
-- Journaled by `cargo xtask check-migrations`; immutable after merge, so a schema change
-- is a new migration, never an edit to this one.

CREATE TABLE quality_profile (
  id              INTEGER PRIMARY KEY,
  name            TEXT NOT NULL UNIQUE,
  is_default      INTEGER NOT NULL DEFAULT 0 CHECK (is_default IN (0,1)),
  rules_json      TEXT NOT NULL,
  cutoff_rank     INTEGER NOT NULL,
  upgrade_allowed INTEGER NOT NULL DEFAULT 1 CHECK (upgrade_allowed IN (0,1)),
  updated_at      INTEGER NOT NULL
);

CREATE TABLE indexer (
  id              INTEGER PRIMARY KEY,
  kind            TEXT NOT NULL CHECK (kind IN ('prowlarr','jackett','builtin','torznab','rss','local')),
  name            TEXT NOT NULL,
  base_url        TEXT,
  api_key_ref     TEXT REFERENCES secret_ref(key) ON DELETE SET NULL,
  enabled         INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0,1)),
  priority        INTEGER NOT NULL DEFAULT 100,
  categories      TEXT,
  rate_limit_json TEXT,
  health_state    TEXT NOT NULL DEFAULT 'unknown' CHECK (health_state IN ('unknown','ok','degraded','failing','disabled')),
  last_check_at   INTEGER,
  UNIQUE (kind, name)
);

CREATE TABLE download_job (
  id                  INTEGER PRIMARY KEY,
  job_id              INTEGER REFERENCES job(id) ON DELETE SET NULL,
  title_id            INTEGER NOT NULL REFERENCES title(id) ON DELETE CASCADE,
  episode_id          INTEGER REFERENCES episode(id) ON DELETE CASCADE,
  media_item_id       INTEGER REFERENCES media_item(id) ON DELETE SET NULL,
  quality_profile_id  INTEGER REFERENCES quality_profile(id) ON DELETE SET NULL,
  indexer_id          INTEGER REFERENCES indexer(id) ON DELETE SET NULL,
  state               TEXT NOT NULL CHECK (state IN ('queued','inspecting','metadata','downloading',
                                                     'paused','seeding','completed','failed','blocked','removed')),
  priority            INTEGER NOT NULL DEFAULT 100,
  is_upgrade          INTEGER NOT NULL DEFAULT 0 CHECK (is_upgrade IN (0,1)),
  release_title       TEXT NOT NULL,
  release_source      TEXT,
  release_group       TEXT,
  resolution          TEXT,
  video_codec         TEXT,
  hdr_format          TEXT,
  audio_summary       TEXT,
  reported_size_bytes INTEGER,
  info_hash           TEXT,
  magnet_uri          TEXT,
  torrent_file_path   TEXT,
  save_path           TEXT NOT NULL,
  downloaded_bytes    INTEGER NOT NULL DEFAULT 0,
  uploaded_bytes      INTEGER NOT NULL DEFAULT 0,
  progress_permille   INTEGER NOT NULL DEFAULT 0,
  download_rate_bps   INTEGER NOT NULL DEFAULT 0,
  upload_rate_bps     INTEGER NOT NULL DEFAULT 0,
  peers_connected     INTEGER NOT NULL DEFAULT 0,
  peers_swarm         INTEGER NOT NULL DEFAULT 0,
  eta_seconds         INTEGER,
  ratio               REAL,
  safety_report_json  TEXT,
  engine_state_json   TEXT,
  error_code          TEXT,
  error_message       TEXT,
  added_by            TEXT NOT NULL DEFAULT 'user' CHECK (added_by IN ('user','auto')),
  added_at            INTEGER NOT NULL,
  started_at          INTEGER,
  completed_at        INTEGER,
  seeding_until       INTEGER,
  removed_at          INTEGER
);
CREATE INDEX download_job_active_idx ON download_job (state, priority)
  WHERE state IN ('queued','inspecting','metadata','downloading','paused');
CREATE INDEX download_job_title_idx  ON download_job (title_id, episode_id);
CREATE INDEX download_job_hash_uq    ON download_job (info_hash) WHERE info_hash IS NOT NULL;

CREATE TABLE download_file (
  id                 INTEGER PRIMARY KEY,
  download_job_id    INTEGER NOT NULL REFERENCES download_job(id) ON DELETE CASCADE,
  torrent_file_index INTEGER NOT NULL,
  path               TEXT NOT NULL,
  size_bytes         INTEGER NOT NULL,
  selected           INTEGER NOT NULL DEFAULT 1 CHECK (selected IN (0,1)),
  is_media           INTEGER NOT NULL DEFAULT 0 CHECK (is_media IN (0,1)),
  blocked_reason     TEXT,
  UNIQUE (download_job_id, torrent_file_index)
);

CREATE TABLE release_decision (
  id              INTEGER PRIMARY KEY,
  download_job_id INTEGER REFERENCES download_job(id) ON DELETE CASCADE,
  title_id        INTEGER NOT NULL REFERENCES title(id) ON DELETE CASCADE,
  episode_id      INTEGER REFERENCES episode(id) ON DELETE CASCADE,
  release_title   TEXT NOT NULL,
  indexer         TEXT,
  score           REAL NOT NULL,
  rank_position   INTEGER NOT NULL,
  chosen          INTEGER NOT NULL DEFAULT 0 CHECK (chosen IN (0,1)),
  reasons_json    TEXT NOT NULL,
  created_at      INTEGER NOT NULL
);
CREATE INDEX release_decision_title_idx ON release_decision (title_id, created_at DESC);
