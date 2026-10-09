-- Migration 0003 — seasons, episodes, and alternate orderings.
--
-- Transcribed from docs/DATA_MODEL.md §2.3. Journaled by `cargo xtask check-migrations`;
-- immutable after merge, so a schema change is a new migration, never an edit to this one.

CREATE TABLE season (
  id             INTEGER PRIMARY KEY,
  title_id       INTEGER NOT NULL REFERENCES title(id) ON DELETE CASCADE,
  season_number  INTEGER NOT NULL,
  name           TEXT,
  overview       TEXT,
  air_date       TEXT,
  poster_provider TEXT,
  poster_path    TEXT,
  poster_blurhash TEXT,
  episode_count  INTEGER,
  is_specials    INTEGER NOT NULL DEFAULT 0 CHECK (is_specials IN (0,1)),
  updated_at     INTEGER NOT NULL,
  UNIQUE (title_id, season_number)
);

CREATE TABLE episode (
  id                  INTEGER PRIMARY KEY,
  title_id            INTEGER NOT NULL REFERENCES title(id) ON DELETE CASCADE,
  season_id           INTEGER NOT NULL REFERENCES season(id) ON DELETE CASCADE,
  season_number       INTEGER NOT NULL,
  episode_number      INTEGER NOT NULL,
  absolute_number     INTEGER,
  name                TEXT,
  overview            TEXT,
  air_date            TEXT,
  runtime_minutes     INTEGER,
  still_provider      TEXT,
  still_path          TEXT,
  still_blurhash      TEXT,
  vote_average        REAL,
  vote_count          INTEGER,
  episode_type        TEXT CHECK (episode_type IN ('standard','special','finale','mid_season','premiere')),
  production_code     TEXT,
  updated_at          INTEGER NOT NULL,
  UNIQUE (title_id, season_number, episode_number)
);
CREATE INDEX episode_air_date_idx ON episode (title_id, air_date);
CREATE INDEX episode_absolute_idx ON episode (title_id, absolute_number);

CREATE TABLE episode_ordering (
  episode_id    INTEGER NOT NULL REFERENCES episode(id) ON DELETE CASCADE,
  scheme        TEXT NOT NULL CHECK (scheme IN ('aired','dvd','absolute','anime')),
  season_number INTEGER NOT NULL,
  episode_number INTEGER NOT NULL,
  PRIMARY KEY (episode_id, scheme)
);
CREATE INDEX episode_ordering_lookup_idx ON episode_ordering (scheme, season_number, episode_number);
