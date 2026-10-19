-- Migration 0025 — the protocol a download job uses.
--
-- A job's transfer engine is now a property of the job rather than of the queue: a torrent
-- seeds after its payload is whole and a Usenet job is finished, and `transition_for` reads
-- this column to choose the completion edge. Existing rows are torrents by definition.
--
-- The rest of the Usenet schema (the NZB source columns, the server and setting tables, the
-- file-index rename, and `media_item.origin`) lands with the engine that writes it, so this
-- migration stays one statement and no column is dead on arrival.

ALTER TABLE download_job ADD COLUMN protocol TEXT NOT NULL DEFAULT 'torrent'
  CHECK (protocol IN ('torrent','usenet'));
