-- One row per acquisition a paired device asks the LAN surface to start (ADR 0028).
--
-- The desktop queue's own rows say what was downloaded; this says who asked. The device row
-- outlives revocation, so the audit keeps its name after the credential is cut off. The job id
-- stays null when the queue never accepted the request; a refusal is still worth recording.

CREATE TABLE web_audit (
  id         INTEGER PRIMARY KEY,
  device_id  INTEGER NOT NULL REFERENCES paired_device(id),
  action     TEXT    NOT NULL CHECK (action IN ('acquire')),
  title_id   INTEGER NOT NULL,
  episode_id INTEGER,
  job_id     INTEGER,
  created_at INTEGER NOT NULL
);

CREATE INDEX web_audit_device_created ON web_audit(device_id, created_at);
