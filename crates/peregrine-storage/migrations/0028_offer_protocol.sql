-- Migration 0028 — the protocol a stored release offer transfers over.
--
-- The offer row keeps its source JSON server-side; the protocol is a plain column so the chooser
-- can label a Usenet offer without parsing a source shape it must never see.

ALTER TABLE release_decision ADD COLUMN protocol TEXT NOT NULL DEFAULT 'torrent'
  CHECK (protocol IN ('torrent','usenet'));
