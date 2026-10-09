-- Migration 0002 — the catalogue: titles, aliases, external ids, genres, keywords,
-- companies, people, and credits.
--
-- Transcribed from docs/DATA_MODEL.md §2.2. Journaled by `cargo xtask check-migrations`;
-- immutable after merge, so a schema change is a new migration, never an edit to this one.

CREATE TABLE title (
  id                  INTEGER PRIMARY KEY,
  kind                TEXT    NOT NULL CHECK (kind IN ('movie','show')),
  subtype             TEXT    NOT NULL DEFAULT 'standard'
                              CHECK (subtype IN ('standard','anime','documentary','reality')),
  name                TEXT    NOT NULL,
  original_name       TEXT,
  sort_name           TEXT    NOT NULL,
  overview            TEXT,
  tagline             TEXT,
  status              TEXT    CHECK (status IN ('rumored','planned','in_production','returning',
                                                'ended','canceled','released','upcoming')),
  release_date        TEXT,
  first_air_date      TEXT,
  last_air_date       TEXT,
  runtime_minutes     INTEGER,
  episode_runtime_minutes INTEGER,
  original_language   TEXT,
  origin_country      TEXT,
  content_rating      TEXT,
  imdb_rating         REAL,
  imdb_votes          INTEGER,
  tmdb_rating         REAL,
  tmdb_votes          INTEGER,
  popularity          REAL,
  adult               INTEGER NOT NULL DEFAULT 0 CHECK (adult IN (0,1)),
  in_production       INTEGER NOT NULL DEFAULT 0 CHECK (in_production IN (0,1)),
  season_count        INTEGER,
  episode_count       INTEGER,

  poster_provider     TEXT,
  poster_path         TEXT,
  poster_blurhash     TEXT,
  backdrop_provider   TEXT,
  backdrop_path       TEXT,
  backdrop_blurhash   TEXT,
  logo_path           TEXT,
  trailer_site        TEXT,
  trailer_key         TEXT,
  trailer_language    TEXT,
  trailer_published_at TEXT,
  homepage            TEXT,
  metadata_state      TEXT    NOT NULL DEFAULT 'stub'
                              CHECK (metadata_state IN ('stub','partial','complete','stale','failed')),
  last_synced_at      INTEGER,
  created_at          INTEGER NOT NULL,
  updated_at          INTEGER NOT NULL
);

CREATE INDEX title_kind_popularity_idx ON title (kind, popularity DESC);
CREATE INDEX title_kind_release_idx    ON title (kind, release_date DESC, first_air_date DESC);
CREATE INDEX title_imdb_rating_idx     ON title (imdb_rating DESC) WHERE imdb_rating IS NOT NULL;
CREATE INDEX title_sort_name_idx       ON title (sort_name);
CREATE INDEX title_sync_idx            ON title (metadata_state, last_synced_at);

CREATE TABLE title_alias (
  id        INTEGER PRIMARY KEY,
  title_id  INTEGER NOT NULL REFERENCES title(id) ON DELETE CASCADE,
  alias     TEXT    NOT NULL,
  region    TEXT,
  kind      TEXT NOT NULL DEFAULT 'alternative' CHECK (kind IN ('alternative','working','original')),
  UNIQUE (title_id, alias, region)
);
CREATE INDEX title_alias_alias_idx ON title_alias (alias);

CREATE TABLE external_id (
  id          INTEGER PRIMARY KEY,
  title_id    INTEGER REFERENCES title(id) ON DELETE CASCADE,
  person_id   INTEGER REFERENCES person(id) ON DELETE CASCADE,
  season_id   INTEGER REFERENCES season(id) ON DELETE CASCADE,
  episode_id  INTEGER REFERENCES episode(id) ON DELETE CASCADE,
  provider    TEXT NOT NULL CHECK (provider IN ('tmdb','imdb','tvdb','anidb','mal','trakt','wikidata')),
  value       TEXT NOT NULL,
  is_primary  INTEGER NOT NULL DEFAULT 0 CHECK (is_primary IN (0,1)),
  fetched_at  INTEGER NOT NULL,

  CHECK ((title_id IS NOT NULL) + (person_id IS NOT NULL)
       + (season_id IS NOT NULL) + (episode_id IS NOT NULL) = 1),
  UNIQUE (provider, value)
);
CREATE INDEX external_id_title_idx  ON external_id (title_id);
CREATE INDEX external_id_episode_idx ON external_id (episode_id);

CREATE TABLE genre (
  id   INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE
);

CREATE TABLE title_genre (
  title_id INTEGER NOT NULL REFERENCES title(id) ON DELETE CASCADE,
  genre_id INTEGER NOT NULL REFERENCES genre(id) ON DELETE CASCADE,
  PRIMARY KEY (title_id, genre_id)
);

CREATE TABLE keyword (
  id   INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE
);

CREATE TABLE title_keyword (
  title_id   INTEGER NOT NULL REFERENCES title(id) ON DELETE CASCADE,
  keyword_id INTEGER NOT NULL REFERENCES keyword(id) ON DELETE CASCADE,
  PRIMARY KEY (title_id, keyword_id)
);

CREATE TABLE company (
  id         INTEGER PRIMARY KEY,
  name       TEXT NOT NULL,
  logo_path  TEXT,
  provider   TEXT,
  external_id TEXT,
  UNIQUE (name, provider)
);

CREATE TABLE title_company (
  title_id   INTEGER NOT NULL REFERENCES title(id) ON DELETE CASCADE,
  company_id INTEGER NOT NULL REFERENCES company(id) ON DELETE CASCADE,
  role       TEXT NOT NULL CHECK (role IN ('network','production','distributor','studio')),
  PRIMARY KEY (title_id, company_id, role)
);

CREATE TABLE person (
  id                  INTEGER PRIMARY KEY,
  name                TEXT NOT NULL,
  sort_name           TEXT NOT NULL,
  known_for_department TEXT,
  profile_provider    TEXT,
  profile_path        TEXT,
  profile_blurhash    TEXT,
  updated_at          INTEGER
);
CREATE INDEX person_name_idx ON person (name);

CREATE TABLE credit (
  id          INTEGER PRIMARY KEY,
  title_id    INTEGER NOT NULL REFERENCES title(id) ON DELETE CASCADE,
  episode_id  INTEGER REFERENCES episode(id) ON DELETE CASCADE,
  person_id   INTEGER NOT NULL REFERENCES person(id) ON DELETE CASCADE,
  kind        TEXT NOT NULL CHECK (kind IN ('cast','crew','guest')),
  character   TEXT,
  job         TEXT,
  department  TEXT,
  order_index INTEGER,
  episode_count INTEGER,
  UNIQUE (title_id, person_id, kind, job, character)
);
CREATE INDEX credit_title_order_idx ON credit (title_id, kind, order_index);
CREATE INDEX credit_person_idx      ON credit (person_id);
