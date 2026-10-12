-- Migration 0011 — the two covering indices the disk-accounting reads go through.
--
-- PG-E3-17 adds two sums whose cost the schema has to carry.
--
-- `media_file_usage_idx` serves DATA_MODEL §4's library storage usage:
--
--   SELECT f.root_id, f.quality_label, COUNT(f.id), COALESCE(SUM(f.size_bytes), 0)
--     FROM media_file f
--     JOIN media_item m ON m.id = f.media_item_id
--     JOIN title t ON t.id = m.title_id
--    WHERE f.missing_since IS NULL
--      AND (?1 = 0 OR (t.adult = 0 AND t.content_rating IS NOT NULL))
--    GROUP BY f.root_id, f.quality_label
--
-- A `media_file` row carries `probe_json`, the gzip-compressed ffprobe document, which is the
-- largest column in the row and is not needed at all to add up sizes. Without this index the
-- aggregate reads every one of those documents off the table's pages. The index covers the
-- four columns the statement needs, so the scan walks leaf entries of roughly forty bytes
-- each, and because its leading columns are the `GROUP BY`'s the groups arrive in order and
-- the query needs no temporary B-tree. `media_item_id` is in the index so the join can start
-- from it; the join and the scope predicate are applied per row and are the reason the plan is
-- a covering index scan *plus* one lookup per file rather than an index-only read. The
-- predicate makes it partial, matching the statement's own `missing_since IS NULL`: a missing
-- file has no bytes on disk, so it is not part of a usage total.
--
-- `provider_cache_size_idx` serves the cache tier of DATA_MODEL §7:
--
--   SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM provider_cache
--
-- `provider_cache`'s pages hold the gzip-compressed response bodies, which DATA_MODEL §7
-- measures at most of the database's 60–300 MB. Adding up one integer column must not read
-- them, so the sum walks a covering index of that column instead of the table.
--
-- Journaled by `cargo xtask check-migrations`; immutable after merge, so a schema change is a
-- new migration, never an edit to this one.

CREATE INDEX media_file_usage_idx ON media_file (root_id, quality_label, media_item_id, size_bytes)
  WHERE missing_since IS NULL;
CREATE INDEX provider_cache_size_idx ON provider_cache (size_bytes);
