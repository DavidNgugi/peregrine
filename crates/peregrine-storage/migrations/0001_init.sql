-- Migration 0001 — identity and preferences, the job journal, and the event log.
--
-- Transcribed from docs/DATA_MODEL.md §2.1 (account, settings, secret_ref) and §2.8
-- (job, event_log). The file is journaled by `cargo xtask check-migrations` and is
-- immutable after merge: a schema change is a new migration, never an edit to this one.

CREATE TABLE account (
  id                INTEGER PRIMARY KEY,
  name              TEXT    NOT NULL,
  kind              TEXT    NOT NULL DEFAULT 'adult' CHECK (kind IN ('adult','child','guest')),
  avatar_kind       TEXT    NOT NULL DEFAULT 'initials'
                    CHECK (avatar_kind IN ('builtin','initials','custom','default')),
  avatar_ref        TEXT,
  avatar_blurhash   TEXT,
  accent_color      TEXT,
  rating_ceiling    TEXT,
  curated_only      INTEGER NOT NULL DEFAULT 0 CHECK (curated_only IN (0,1)),
  can_acquire       INTEGER NOT NULL DEFAULT 1 CHECK (can_acquire IN (0,1)),
  can_delete        INTEGER NOT NULL DEFAULT 0 CHECK (can_delete IN (0,1)),
  can_manage        INTEGER NOT NULL DEFAULT 0 CHECK (can_manage IN (0,1)),
  can_cast          INTEGER NOT NULL DEFAULT 1 CHECK (can_cast IN (0,1)),
  can_use_agents    INTEGER NOT NULL DEFAULT 0 CHECK (can_use_agents IN (0,1)),
  can_see_queue     INTEGER NOT NULL DEFAULT 1 CHECK (can_see_queue IN (0,1)),
  lock_policy       TEXT    NOT NULL DEFAULT 'none'
                    CHECK (lock_policy IN ('none','app_pin','os_auth')),
  pin_hash          TEXT,
  idle_lock_minutes INTEGER,
  is_default        INTEGER NOT NULL DEFAULT 0 CHECK (is_default IN (0,1)),
  created_at        INTEGER NOT NULL,
  last_used_at      INTEGER,
  CHECK (kind <> 'child' OR rating_ceiling IS NOT NULL OR curated_only = 1),
  CHECK (kind <> 'child' OR can_acquire = 0)
);

CREATE TABLE settings (
  key        TEXT PRIMARY KEY,
  value_json TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE secret_ref (
  key             TEXT PRIMARY KEY,
  keyring_service TEXT NOT NULL,
  keyring_account TEXT NOT NULL,
  created_at      INTEGER NOT NULL
);

CREATE TABLE job (
  id                INTEGER PRIMARY KEY,
  kind              TEXT NOT NULL,
  state             TEXT NOT NULL CHECK (state IN ('queued','running','succeeded','failed','canceled','deferred')),
  priority          INTEGER NOT NULL DEFAULT 100,
  dedupe_key        TEXT,
  payload_json      TEXT NOT NULL,
  progress_permille INTEGER NOT NULL DEFAULT 0,
  attempts          INTEGER NOT NULL DEFAULT 0,
  max_attempts      INTEGER NOT NULL DEFAULT 3,
  next_attempt_at   INTEGER,
  last_error_code   TEXT,
  last_error        TEXT,
  created_at        INTEGER NOT NULL,
  started_at        INTEGER,
  finished_at       INTEGER,
  cancel_requested  INTEGER NOT NULL DEFAULT 0 CHECK (cancel_requested IN (0,1))
);

CREATE UNIQUE INDEX job_dedupe_uq ON job (kind, dedupe_key)
  WHERE dedupe_key IS NOT NULL AND state IN ('queued','running','deferred');
CREATE INDEX job_ready_idx ON job (state, priority, next_attempt_at);

CREATE TABLE event_log (
  id           INTEGER PRIMARY KEY,
  at           INTEGER NOT NULL,
  kind         TEXT NOT NULL,
  subject_kind TEXT,
  subject_id   INTEGER,
  detail_json  TEXT
);
CREATE INDEX event_log_at_idx ON event_log (at DESC);
