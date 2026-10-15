-- no-transaction
-- Migration 0014: a title a list answer filled is `listed`, not `partial`.
--
-- A rail or a search page answers with a name, a date, a poster, a score, a synopsis, and a
-- vote count. `record_list_answer` writes those and stamped `partial`, which is the same
-- value a *details* read writes for a fill that got halfway — so nothing could tell a title
-- nobody had read from a title somebody had. The cost was silent: `artwork::refresh_due`
-- decides whether opening a title is worth a provider request, and every title a rail listed
-- looked already filled in, so a details read never happened and the fields only it carries
-- — a runtime, a genre, a cast — never arrived.
--
-- `listed` is that state. DATA_MODEL §2.2 documents it beside the other four.
--
-- # Why this file is not a transaction the runner opened
--
-- `metadata_state`'s CHECK is part of the table definition and SQLite cannot alter a
-- constraint, so the table is rebuilt. Two details of the 12-step rebuild are worth naming
-- here, because both fail silently:
--
--   * `PRAGMA foreign_keys=OFF` is a no-op inside a transaction, so sqlx's runner must not
--     have opened one — which is what `-- no-transaction` on the first line says. With
--     enforcement on, `DROP TABLE title` performs an implicit `DELETE FROM title` and the
--     fifteen `ON DELETE CASCADE` children go with it: the user's whole catalogue, deleted
--     by a schema change that then reports success.
--   * The pragma is restored here rather than relied on. The runner enforces the documented
--     set *before* the migrations and not after, so a migration that left it off would leave
--     the writer connection — the connection every write in the process runs on — enforcing
--     nothing, for the life of the app.
--
-- The rebuild itself is wrapped in an explicit transaction, because the half states are not
-- equivalent: a crash between the drop and the rename leaves no `title` table at all. A
-- failure inside it is not followed by the `COMMIT`, so the transaction is still open when
-- the runner reports the migration — the migration is recorded only after the script
-- returns (`sqlx` inserts into `_sqlx_migrations` afterwards) — and the connection the
-- failing runner drops is what rolls it back. The pre-migration copy `crate::backup` takes
-- is the way back from anything this cannot undo, as it is for every migration
-- (DATA_MODEL §5).

PRAGMA foreign_keys=OFF;
BEGIN;

-- What the foreign-key check reports before the rebuild, so the guard at the end can ask
-- whether this migration made it worse rather than whether the database was already clean.
CREATE TEMP TABLE fk_before AS SELECT COUNT(*) AS violations FROM pragma_foreign_key_check;

CREATE TABLE title_rebuilt (
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
                              CHECK (metadata_state IN ('stub','listed','partial','complete','stale','failed')),
  last_synced_at      INTEGER,
  created_at          INTEGER NOT NULL,
  updated_at          INTEGER NOT NULL
);

INSERT INTO title_rebuilt
  (id, kind, subtype, name, original_name, sort_name, overview, tagline, status,
   release_date, first_air_date, last_air_date, runtime_minutes, episode_runtime_minutes,
   original_language, origin_country, content_rating, imdb_rating, imdb_votes, tmdb_rating,
   tmdb_votes, popularity, adult, in_production, season_count, episode_count,
   poster_provider, poster_path, poster_blurhash, backdrop_provider, backdrop_path,
   backdrop_blurhash, logo_path, trailer_site, trailer_key, trailer_language,
   trailer_published_at, homepage, metadata_state, last_synced_at, created_at, updated_at)
SELECT
   id, kind, subtype, name, original_name, sort_name, overview, tagline, status,
   release_date, first_air_date, last_air_date, runtime_minutes, episode_runtime_minutes,
   original_language, origin_country, content_rating, imdb_rating, imdb_votes, tmdb_rating,
   tmdb_votes, popularity, adult, in_production, season_count, episode_count,
   poster_provider, poster_path, poster_blurhash, backdrop_provider, backdrop_path,
   backdrop_blurhash, logo_path, trailer_site, trailer_key, trailer_language,
   trailer_published_at, homepage, metadata_state, last_synced_at, created_at, updated_at
  FROM title;

DROP TABLE title;
ALTER TABLE title_rebuilt RENAME TO title;

-- The indexes the old table carried. They were dropped with it, and every one of them is
-- named in `DOCUMENTED_INDEX_FRAGMENTS` — the schema test that would otherwise pass while
-- the rebuild quietly lost one.
CREATE INDEX title_kind_popularity_idx ON title (kind, popularity DESC);
CREATE INDEX title_kind_release_idx    ON title (kind, release_date DESC, first_air_date DESC);
CREATE INDEX title_imdb_rating_idx     ON title (imdb_rating DESC) WHERE imdb_rating IS NOT NULL;
CREATE INDEX title_sort_name_idx       ON title (sort_name);
CREATE INDEX title_sync_idx            ON title (metadata_state, last_synced_at);

-- Step 10 of the procedure, as a guard rather than a report: with enforcement off for the
-- length of this rebuild, a violation the rebuild *introduced* is the one outcome that must
-- not be committed. The count before it is what makes the guard usable — the insert below
-- fails on an increase and not on a violation that was already there, because a database
-- that arrived with a dangling row is one this migration can neither have caused nor
-- repair, and refusing to open it for ever is a worse failure than the row itself again.
-- A rebuild that lost rows would leave their children dangling, so "must not increase" is
-- also what catches the botched copy this check exists for.
CREATE TEMP TABLE foreign_key_guard (ok INTEGER NOT NULL CHECK (ok = 1));
INSERT INTO foreign_key_guard (ok)
  SELECT 0 WHERE (SELECT COUNT(*) FROM pragma_foreign_key_check) > (SELECT violations FROM fk_before);
DROP TABLE foreign_key_guard;
DROP TABLE fk_before;

COMMIT;
PRAGMA foreign_keys=ON;
