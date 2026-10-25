-- Integration choices, skip-marker cache, and the health each probe last reported.
--
-- The credentials themselves are never here: they live in the OS keyring under
-- `integration.<id>.<field>` (SECURITY.md T13, PG-E24-05). This table holds only the choice a
-- person made, the order within a category, and whether they consented to submissions — three
-- facts that must survive a restart and must not survive a backup as secrets.
--
-- A marker is a property of a title or episode, not of a file: the same episode has the same
-- intro whether it is played from a 1080p or a 4K copy. `marker_fetch` is the negative cache as
-- well as the positive one, because a provider that answered "nothing" must not be asked again on
-- every play.

CREATE TABLE integration_setting (
  integration        TEXT    PRIMARY KEY,
  enabled            INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0,1)),
  priority           INTEGER NOT NULL DEFAULT 0 CHECK (priority >= 0),
  submission_enabled INTEGER NOT NULL DEFAULT 0 CHECK (submission_enabled IN (0,1)),
  last_test_at       INTEGER,
  last_outcome       TEXT    CHECK (last_outcome IS NULL OR last_outcome IN
                     ('working','unauthorized','rateLimited','unreachable','keychainDenied','notConfigured')),
  last_error_code    TEXT,
  updated_at         INTEGER NOT NULL
);

CREATE TABLE playback_marker (
  id          INTEGER PRIMARY KEY,
  title_id    INTEGER NOT NULL REFERENCES title(id) ON DELETE CASCADE,
  episode_id  INTEGER REFERENCES episode(id) ON DELETE CASCADE,
  kind        TEXT    NOT NULL CHECK (kind IN ('intro','recap','credits','preview')),
  start_ms    INTEGER NOT NULL CHECK (start_ms >= 0),
  end_ms      INTEGER,
  source      TEXT    NOT NULL CHECK (source IN ('chapters','introdb','aniskip','introhater')),
  confidence  INTEGER NOT NULL CHECK (confidence BETWEEN 0 AND 100),
  fetched_at  INTEGER NOT NULL,
  -- A local override, never submitted anywhere. Suppression is how a person says "this marker is
  -- wrong for my copy" without editing a community database (MKR-05).
  suppressed  INTEGER NOT NULL DEFAULT 0 CHECK (suppressed IN (0,1)),
  CHECK (end_ms IS NULL OR end_ms > start_ms),
  CHECK (end_ms IS NOT NULL OR kind = 'credits')
);

-- The natural key includes the COALESCE because SQLite treats NULLs as distinct in a UNIQUE
-- index, and a film's episode_id is NULL. Without it a re-fetch would duplicate every movie marker.
CREATE UNIQUE INDEX playback_marker_natural_uq
  ON playback_marker (title_id, COALESCE(episode_id, -1), kind, source, start_ms);

CREATE INDEX playback_marker_subject_idx
  ON playback_marker (title_id, episode_id, suppressed);

CREATE TABLE marker_fetch (
  title_id    INTEGER NOT NULL REFERENCES title(id) ON DELETE CASCADE,
  -- 0 means the whole title: a primary key cannot hold NULL, and a sentinel is clearer here than
  -- a second table for films.
  episode_id  INTEGER NOT NULL DEFAULT 0,
  source      TEXT    NOT NULL CHECK (source IN ('chapters','introdb','aniskip','introhater')),
  fetched_at  INTEGER NOT NULL,
  expires_at  INTEGER NOT NULL,
  etag        TEXT,
  outcome     TEXT    NOT NULL CHECK (outcome IN ('found','none','failed')),
  error_code  TEXT,
  PRIMARY KEY (title_id, episode_id, source)
);

CREATE INDEX marker_fetch_expiry_idx ON marker_fetch (expires_at);
