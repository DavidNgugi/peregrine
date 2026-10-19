-- Migration 0026 — the columns a Usenet job needs, and a protocol-neutral file index.
--
-- `protocol` (0025) decides the completion edge. This migration adds the source columns the
-- engine reads, the post-processing stage a screen draws, the dedupe key that replaces an info
-- hash for a protocol that has none, and renames the file index that was never torrent-specific:
-- an NZB's entries are numbered the same way.
--
-- The server and setting tables land with the settings surface that reads them, and
-- `media_item.origin` gains its Usenet value with the import that writes it, so no column or
-- table here is dead on arrival.

ALTER TABLE download_job ADD COLUMN nzb_sha256 TEXT;
ALTER TABLE download_job ADD COLUMN nzb_path TEXT;
ALTER TABLE download_job ADD COLUMN stage TEXT NOT NULL DEFAULT 'transfer'
  CHECK (stage IN ('transfer','repairing','extracting','importing'));

-- The same live set `download_job_hash_uq` uses: a terminal job is history and a retry is a new
-- row, so only a live job can conflict with another live job for one post.
CREATE UNIQUE INDEX download_job_nzb_uq ON download_job (nzb_sha256)
  WHERE nzb_sha256 IS NOT NULL
    AND state IN ('queued','inspecting','metadata','downloading','paused','seeding');

ALTER TABLE download_file RENAME COLUMN torrent_file_index TO file_index;
