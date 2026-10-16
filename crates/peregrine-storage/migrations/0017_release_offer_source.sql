-- The offers a search found at other qualities, kept so a person can pick one without a
-- second search. release_decision was the audit trail and had no source, so it gains the source
-- the engine needs plus the numbers the chooser shows.
--
-- The source stays server-side: nothing that crosses IPC reads source_json.
ALTER TABLE release_decision ADD COLUMN source_json TEXT;
ALTER TABLE release_decision ADD COLUMN size_bytes INTEGER;
ALTER TABLE release_decision ADD COLUMN seeders INTEGER;
ALTER TABLE release_decision ADD COLUMN dismissed INTEGER NOT NULL DEFAULT 0 CHECK (dismissed IN (0,1));
