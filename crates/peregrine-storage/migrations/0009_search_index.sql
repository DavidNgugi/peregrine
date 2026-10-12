-- Migration 0009 — the full-text search index over `title` (`DATA_MODEL` §2.9).
--
-- Contentless (`content=''`), so the corpus is not stored a second time: the index holds
-- terms and the rowid they came from, and nothing else. The maintenance path therefore has
-- to be explicit, because no trigger can read back the text to re-index it.
--
-- `contentless_delete=1` is the one deviation from §2.9 as first written, and it is
-- load-bearing rather than cosmetic. Without it `SQLite` refuses both statements outright —
-- `cannot DELETE from contentless fts5 table` and `cannot UPDATE contentless fts5 table` —
-- and the `'delete'` special command silently affects zero rows unless it is handed the
-- original column values, which a contentless table no longer has. A title could then only
-- ever be added to the index, and removing one would mean rebuilding the whole index.
-- Verified against the bundled SQLite 3.51.3; `DATA_MODEL` §2.9 records the same.
--
-- The index contains the text the search paths match on: the title's name, its original
-- name, its aliases, its overview, its keywords, and its cast names. It contains no
-- row that `title` does not hold: every maintenance statement selects from `title`, and
-- every read joins back to it.
--
-- Journaled by `cargo xtask check-migrations`; immutable after merge, so a schema change is
-- a new migration, never an edit to this one.

CREATE VIRTUAL TABLE title_fts USING fts5 (
  name,
  original_name,
  aliases,
  overview,
  keywords,
  cast_names,
  content='',
  contentless_delete=1,
  tokenize="unicode61 remove_diacritics 2 tokenchars '''&'",
  prefix='2 3 4'
);
