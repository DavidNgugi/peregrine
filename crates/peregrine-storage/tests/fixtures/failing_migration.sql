-- A migration that always fails, used only by the migration runner's tests (PG-E3-13).
--
-- It writes real objects before it fails, so a runner that did not wrap a migration in a
-- transaction would leave `migration_probe` behind and the test would see it. It lives
-- outside `migrations/` so the embedded set and the migration gate never see it.

CREATE TABLE migration_probe (id INTEGER PRIMARY KEY, note TEXT NOT NULL);

INSERT INTO migration_probe (id, note) VALUES (1, 'written before the failure');

INSERT INTO no_such_table (id) VALUES (1);
