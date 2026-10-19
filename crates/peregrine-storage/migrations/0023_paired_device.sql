-- Devices paired to the LAN surface (ADR 0028), and the tokens they present.
--
-- The token is a credential this application mints, so the row stores sha256(token) and never the
-- token: a copy of the database is a list of devices, not a set of working credentials. Revocation
-- is a timestamp rather than a delete, because "which device was cut off, and when" is the question
-- an audit asks and a deleted row cannot answer it.
--
-- Scopes are a comma-separated list (`read,stream`) rather than a table: a device has one row, the
-- vocabulary is closed by ADR 0028 §3, and a scope table for two values would be a join on every
-- authenticated request. The app reads the column and refuses an unknown scope rather than guessing.
--
-- last_address is the address the device last presented, which is what the Desktop devices list
-- shows beside "last seen". It is not an authorization input.

CREATE TABLE paired_device (
  id           INTEGER PRIMARY KEY,
  name         TEXT    NOT NULL CHECK (length(name) BETWEEN 1 AND 64),
  platform     TEXT,
  token_hash   TEXT    NOT NULL UNIQUE CHECK (length(token_hash) = 64),
  scopes       TEXT    NOT NULL CHECK (length(scopes) > 0),
  created_at   INTEGER NOT NULL,
  last_seen_at INTEGER,
  last_address TEXT,
  revoked_at   INTEGER
);
