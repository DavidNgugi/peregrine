-- Migration 0020: a download a person asked for before it could be fetched.
--
-- # Why it exists
--
-- A title and an episode both carry a date, and the queue has no way to say "not yet". A
-- scheduled row is the request without the attempt: it names the title (or one episode), the
-- date from which it may be fetched, and the state the scheduler has left it in.
--
-- # Why the key is generated
--
-- SQLite treats NULLs in a UNIQUE constraint as distinct, so (title_id, episode_id) would allow
-- two whole-title schedules. entity_key collapses the two shapes into one spelling, the same way
-- watch_progress does.
--
-- The row is deleted when cancelled; 'started' and 'failed' are history the scheduler does not
-- revisit, and the jobs themselves carry the queue's states after that.

CREATE TABLE download_schedule (
  id          INTEGER PRIMARY KEY,
  account_id  INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
  title_id    INTEGER NOT NULL REFERENCES title(id) ON DELETE CASCADE,
  episode_id  INTEGER REFERENCES episode(id) ON DELETE CASCADE,
  entity_key  TEXT GENERATED ALWAYS AS (
                CASE WHEN episode_id IS NULL THEN 't' || title_id ELSE 'e' || episode_id END
              ) STORED,
  release_on  TEXT NOT NULL,
  state       TEXT NOT NULL DEFAULT 'scheduled' CHECK (state IN ('scheduled','started','failed')),
  error       TEXT,
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL,
  UNIQUE (account_id, entity_key)
);

CREATE INDEX download_schedule_due_idx
  ON download_schedule (account_id, release_on)
  WHERE state = 'scheduled';
