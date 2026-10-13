-- Migration 0012 — the patterns a user's corrections become.
--
-- PG-E14-09's acceptance is "second occurrence auto-matches", and this table is what a second
-- occurrence is matched against. A user answering a parked file says which title its name meant;
-- that answer is worth more than the one file, because the same naming habit produced the rest of
-- the folder.
--
-- The pattern is the title the *parse read*, normalized by the caller. It is deliberately not the
-- whole filename: a rule that matched one filename exactly would only ever match the file it was
-- created from. `The Matrix 2049.mkv` and `the matrix.2049.mkv` are the same naming habit, and both
-- parse to a title of `The Matrix`, so both are the same pattern.
--
-- **The table says which title a name means, and nothing about where in it the file sits.** That is
-- not an omission: a rule cannot settle a numbering question, which is why every episode-shaped
-- refusal in the indexer is decided before this is consulted. `Breaking Bad Season 1.mkv` and
-- `Breaking Bad Season 2.mkv` read the same title and are different seasons, so a rule carrying a
-- season would misplace the second one — and a multi-episode file's range is PG-E14-07's question,
-- not a user's habit. What a rule does settle is the case that reaches here with a title: an
-- ambiguous year, an odd suffix, a name the parser refused after reading the title.
--
-- One pattern per rule (`UNIQUE`), because two rules for one pattern could disagree and the read
-- that consults them would have to pick. A user changing their mind edits the row rather than
-- adding a second one, which is what the upsert in `catalogue::insert_match_rule` does.
CREATE TABLE match_rule (
  id         INTEGER PRIMARY KEY,
  pattern    TEXT NOT NULL,
  title_id   INTEGER NOT NULL REFERENCES title(id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  UNIQUE (pattern)
);

-- The lookup the indexer makes before it parks a file: one pattern at a time, on a connection that
-- is already open. `UNIQUE (pattern)` above is the index for it, so this comment only records that
-- the read is the reason the column is unique.
