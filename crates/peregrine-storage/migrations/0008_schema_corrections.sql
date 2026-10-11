-- Migration 0008: the three schema defects that PG-E3-06, PG-E3-08, and PG-E3-09 recorded.
--
-- Migrations 0001-0007 transcribed DATA_MODEL §2 literally, including three places where
-- the document itself was wrong or ambiguous. Those files are immutable, so the
-- corrections live here and the document is corrected in the same change (PG-E3-19):
--
--   1. `media_file.video_account` was ffprobe's `profile`, mistyped in §2.3.
--   2. `download_job_hash_uq` was a plain index whose name claimed uniqueness.
--   3. `image_ref`'s UNIQUE constraint was defeated by a nullable `language`, because
--      SQLite treats NULLs as distinct.

-- 1. A probe reports a video `profile` — "High", "Main 10", "Baseline" — and there is no
-- such thing as a video account. RENAME COLUMN carries the data and rewrites anything
-- that named the old column, so no row is touched and nothing has to be re-probed.
ALTER TABLE media_file RENAME COLUMN video_account TO video_profile;

-- 2. The index finally means what its name says: at most one *live* job per info hash,
-- where live is every state that is not a terminal outcome. History may repeat, because
-- re-downloading a release after a removal or a failure is ordinary — the same reasoning
-- as `job_dedupe_uq`, which is unique only among non-terminal jobs.
--
-- A database written under the old index could hold two live jobs for one hash, and the
-- unique index cannot be created over that. The alternative to reconciling them is
-- refusing to open the database at all, so the later ones are marked failed. No row is
-- deleted: each keeps its engine state, its file list, and its history, and the code says
-- what happened. The superseded set is materialised first because an UPDATE whose
-- subquery reads the table it is writing to can observe its own partial writes.
CREATE TEMP TABLE superseded_info_hash AS
  SELECT id FROM download_job
   WHERE info_hash IS NOT NULL
     AND state IN ('queued','inspecting','metadata','downloading','paused','seeding')
     AND id NOT IN (
       SELECT MIN(id) FROM download_job
        WHERE info_hash IS NOT NULL
          AND state IN ('queued','inspecting','metadata','downloading','paused','seeding')
        GROUP BY info_hash
     );

UPDATE download_job
   SET state = 'failed',
       error_code = 'DUPLICATE_INFO_HASH',
       error_message = 'Superseded by an earlier live job for the same torrent when migration 0008 made the info-hash index unique.'
 WHERE id IN (SELECT id FROM superseded_info_hash);

DROP TABLE superseded_info_hash;

DROP INDEX download_job_hash_uq;
CREATE UNIQUE INDEX download_job_hash_uq ON download_job (info_hash)
  WHERE info_hash IS NOT NULL
    AND state IN ('queued','inspecting','metadata','downloading','paused','seeding');

-- 3. `language` becomes NOT NULL with the empty string meaning "the provider gave none".
-- A sentinel is the only fix available: SQLite cannot drop the inline UNIQUE constraint,
-- so the table has to be rebuilt, and a rebuilt table can make the column NOT NULL.
--
-- Rows that were identical under the agreed key collapse to the earliest one. They are the
-- same artwork from the same provider — that is what the key means — so nothing is lost,
-- and keeping the lowest id keeps the row the rest of the database already refers to.
CREATE TABLE image_ref_rebuilt (
  id           INTEGER PRIMARY KEY,
  owner_kind   TEXT NOT NULL CHECK (owner_kind IN ('title','season','episode','person','company')),
  owner_id     INTEGER NOT NULL,
  role         TEXT NOT NULL CHECK (role IN ('poster','backdrop','logo','still','profile')),
  provider     TEXT NOT NULL,
  path         TEXT NOT NULL,
  language     TEXT NOT NULL DEFAULT '',
  width        INTEGER,
  height       INTEGER,
  vote_average REAL,
  is_primary   INTEGER NOT NULL DEFAULT 0 CHECK (is_primary IN (0,1)),
  blurhash     TEXT,
  UNIQUE (owner_kind, owner_id, role, provider, path, language)
);

INSERT INTO image_ref_rebuilt
  (id, owner_kind, owner_id, role, provider, path, language, width, height, vote_average,
   is_primary, blurhash)
SELECT id, owner_kind, owner_id, role, provider, path, COALESCE(language, ''), width, height,
       vote_average, is_primary, blurhash
  FROM image_ref
 WHERE id IN (
   SELECT MIN(id) FROM image_ref
    GROUP BY owner_kind, owner_id, role, provider, path, COALESCE(language, '')
 );

DROP TABLE image_ref;
ALTER TABLE image_ref_rebuilt RENAME TO image_ref;
