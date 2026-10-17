-- Migration 0018 — the installation account.
--
-- Every user-facing read and write is scoped to account 1 (`Runtime::installation_scope`), but
-- nothing ever created that row. The gap only showed on the first write that references it: a
-- `watch_progress` insert failed its foreign key, so the in-page player's progress was silently
-- discarded and Continue Watching could never populate. The insert is `OR IGNORE` so a database
-- that already holds row 1 — a test fixture, or a future account screen — is unchanged.

INSERT OR IGNORE INTO account (id, name, kind, is_default, created_at)
VALUES (1, 'You', 'adult', 1, 0);
