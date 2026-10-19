-- no-transaction
-- Migration 0027 — Usenet servers, the Usenet policy row, and a media origin for Usenet.
--
-- The rebuild of `media_item` is here rather than in 0026 because the import that writes the
-- new origin lands with the post-processing pipeline. The two small tables are read by the
-- settings surface and the engine, so they arrive with the repository that serves them.
--
-- # Why this file is not a transaction the runner opened
--
-- `media_item.origin` is a CHECK constraint and SQLite cannot alter one, so the table is rebuilt.
-- `PRAGMA foreign_keys=OFF` is a no-op inside a transaction, and with enforcement on `DROP
-- TABLE media_item` would cascade through `media_file`. The pragma is restored at the end, because
-- the runner enforces the documented set before the migrations and not after.

PRAGMA foreign_keys=OFF;
BEGIN;

CREATE TABLE usenet_server (
  id              INTEGER PRIMARY KEY,
  name            TEXT NOT NULL UNIQUE,
  host            TEXT NOT NULL,
  port            INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
  tls             INTEGER NOT NULL DEFAULT 1 CHECK (tls IN (0,1)),
  username        TEXT NOT NULL DEFAULT '',
  password_secret TEXT NOT NULL,
  connections     INTEGER NOT NULL DEFAULT 8 CHECK (connections BETWEEN 1 AND 64),
  priority        INTEGER NOT NULL DEFAULT 100,
  enabled         INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0,1)),
  health_state    TEXT NOT NULL DEFAULT 'unknown'
                  CHECK (health_state IN ('unknown','ok','degraded','failing','disabled')),
  last_check_at   INTEGER,
  created_at      INTEGER NOT NULL,
  UNIQUE (host, port, username)
);
CREATE INDEX usenet_server_enabled_idx ON usenet_server (enabled, priority);

CREATE TABLE usenet_setting (
  id                          INTEGER PRIMARY KEY CHECK (id = 1),
  allow_archives              INTEGER NOT NULL DEFAULT 0 CHECK (allow_archives IN (0,1)),
  delete_archives_after_import INTEGER NOT NULL DEFAULT 1
                              CHECK (delete_archives_after_import IN (0,1)),
  updated_at                  INTEGER NOT NULL
);
INSERT INTO usenet_setting (id, updated_at) VALUES (1, 0);

CREATE TABLE media_item_rebuilt (
  id                INTEGER PRIMARY KEY,
  title_id          INTEGER NOT NULL REFERENCES title(id) ON DELETE CASCADE,
  episode_id        INTEGER REFERENCES episode(id) ON DELETE CASCADE,
  state             TEXT NOT NULL CHECK (state IN ('wanted','downloading','available','partial','failed')),
  origin            TEXT NOT NULL CHECK (origin IN ('scan','torrent','manual','usenet')),
  preferred_file_id INTEGER,
  quality_label     TEXT,
  created_at        INTEGER NOT NULL,
  updated_at        INTEGER NOT NULL
);
INSERT INTO media_item_rebuilt
  SELECT id, title_id, episode_id, state, origin, preferred_file_id, quality_label,
         created_at, updated_at
  FROM media_item;
DROP TABLE media_item;
ALTER TABLE media_item_rebuilt RENAME TO media_item;
CREATE UNIQUE INDEX media_item_movie_uq  ON media_item (title_id) WHERE episode_id IS NULL;
CREATE UNIQUE INDEX media_item_episode_uq ON media_item (episode_id) WHERE episode_id IS NOT NULL;

COMMIT;
PRAGMA foreign_keys=ON;
