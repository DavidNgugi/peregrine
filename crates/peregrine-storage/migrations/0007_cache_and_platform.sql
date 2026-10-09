-- Migration 0007 — caches, artwork references, dataset state, the notification centre,
-- and cast devices and sessions.
--
-- Transcribed from docs/DATA_MODEL.md §2.8, except `job` and `event_log`, which 0001
-- already creates. The two cache-tier tables are `provider_cache` and `image_cache`:
-- invariant I9 requires both to be droppable without loss of user data, so nothing may
-- reference them.
--
-- Journaled by `cargo xtask check-migrations`; immutable after merge, so a schema change
-- is a new migration, never an edit to this one.

CREATE TABLE provider_cache (
  key            TEXT PRIMARY KEY,
  provider       TEXT NOT NULL,
  resource       TEXT NOT NULL,
  etag           TEXT,
  last_modified  TEXT,
  fetched_at     INTEGER NOT NULL,
  expires_at     INTEGER NOT NULL,
  schema_version INTEGER NOT NULL,
  body           BLOB NOT NULL,
  size_bytes     INTEGER NOT NULL
);
CREATE INDEX provider_cache_expiry_idx ON provider_cache (expires_at);
CREATE INDEX provider_cache_resource_idx ON provider_cache (provider, resource);

CREATE TABLE image_cache (
  key          TEXT PRIMARY KEY,
  provider     TEXT NOT NULL,
  path         TEXT NOT NULL,
  variant      TEXT NOT NULL,
  disk_path    TEXT NOT NULL,
  bytes        INTEGER NOT NULL,
  width        INTEGER,
  height       INTEGER,
  created_at   INTEGER NOT NULL,
  last_used_at INTEGER NOT NULL,
  hits         INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX image_cache_lru_idx ON image_cache (last_used_at);

CREATE TABLE image_ref (
  id           INTEGER PRIMARY KEY,
  owner_kind   TEXT NOT NULL CHECK (owner_kind IN ('title','season','episode','person','company')),
  owner_id     INTEGER NOT NULL,
  role         TEXT NOT NULL CHECK (role IN ('poster','backdrop','logo','still','profile')),
  provider     TEXT NOT NULL,
  path         TEXT NOT NULL,
  language     TEXT,
  width        INTEGER,
  height       INTEGER,
  vote_average REAL,
  is_primary   INTEGER NOT NULL DEFAULT 0 CHECK (is_primary IN (0,1)),
  blurhash     TEXT,
  UNIQUE (owner_kind, owner_id, role, provider, path, language)
);

CREATE TABLE dataset_state (
  name            TEXT PRIMARY KEY,
  version         TEXT,
  etag            TEXT,
  last_modified   TEXT,
  rows            INTEGER,
  size_bytes      INTEGER,
  last_checked_at INTEGER,
  last_updated_at INTEGER,
  error           TEXT
);

CREATE TABLE notification (
  id                INTEGER PRIMARY KEY,
  event             TEXT NOT NULL,
  severity          TEXT NOT NULL DEFAULT 'normal' CHECK (severity IN ('normal','critical')),
  title             TEXT NOT NULL,
  body              TEXT,
  deep_link         TEXT,
  subject_kind      TEXT CHECK (subject_kind IN ('title','episode','download','device','update')),
  subject_id        INTEGER,
  dedupe_key        TEXT,
  group_key         TEXT,
  sound             TEXT,
  delivery_state    TEXT NOT NULL DEFAULT 'pending'
                    CHECK (delivery_state IN ('pending','delivered','suppressed','failed')),
  suppressed_reason TEXT CHECK (suppressed_reason IN
                    ('disabled','quiet_hours','duplicate','focus_suppressed','rate_limited','permission_denied')),
  native_id         INTEGER,
  delivered_at      INTEGER,
  delivery_error    TEXT,
  clicked_at        INTEGER,
  created_at        INTEGER NOT NULL,
  read_at           INTEGER,
  dismissed_at      INTEGER,
  CHECK (delivery_state <> 'suppressed' OR suppressed_reason IS NOT NULL)
);
CREATE INDEX notification_centre_idx ON notification (created_at DESC) WHERE dismissed_at IS NULL;
CREATE INDEX notification_unread_idx ON notification (created_at DESC) WHERE read_at IS NULL;
CREATE INDEX notification_dedupe_idx ON notification (dedupe_key, created_at DESC)
  WHERE dedupe_key IS NOT NULL;
CREATE INDEX notification_pending_idx ON notification (delivery_state) WHERE delivery_state = 'pending';

CREATE TABLE device (
  id                INTEGER PRIMARY KEY,
  protocol          TEXT NOT NULL CHECK (protocol IN ('chromecast','dlna')),
  device_uid        TEXT NOT NULL,
  name              TEXT NOT NULL,
  model             TEXT,
  address           TEXT,
  port              INTEGER,
  capabilities_json TEXT,
  trusted           INTEGER NOT NULL DEFAULT 0 CHECK (trusted IN (0,1)),
  last_seen_at      INTEGER NOT NULL,
  UNIQUE (protocol, device_uid)
);

CREATE TABLE cast_session (
  id            INTEGER PRIMARY KEY,
  device_id     INTEGER NOT NULL REFERENCES device(id) ON DELETE CASCADE,
  media_file_id INTEGER REFERENCES media_file(id) ON DELETE SET NULL,
  episode_id    INTEGER REFERENCES episode(id) ON DELETE SET NULL,
  state         TEXT NOT NULL CHECK (state IN ('connecting','loading','playing','paused','buffering','ended','failed')),
  stream_url    TEXT,
  delivery      TEXT NOT NULL CHECK (delivery IN ('direct','remux_hls','transcode_hls','burn_in')),
  ffmpeg_args   TEXT,
  position_ms   INTEGER NOT NULL DEFAULT 0,
  started_at    INTEGER NOT NULL,
  ended_at      INTEGER,
  error_message TEXT
);
