-- The account's playback language preference, one row (ADR 0028's web client).
--
-- A singleton rather than a settings table: the desktop Settings screen writes it, the LAN
-- listener reads it on every playback mint, and a preference that could fan out into several rows
-- would make "which one applies" a rule the reader had to guess. The CHECK constraint on id is
-- what makes the table singleton, and the mode CHECK is the three values the web contract names.
--
-- The default row is inserted rather than left absent, so a fresh installation answers the read
-- without a fallback path that a later migration could make disagree with the column default.
-- updated_at is the instant the desktop last wrote the row; the inserted default is 0 so a reader
-- can tell "never chosen" from "chosen at the epoch".

CREATE TABLE playback_preference (
  id                INTEGER PRIMARY KEY CHECK (id = 1),
  audio_language    TEXT    NOT NULL,
  subtitle_language TEXT    NOT NULL,
  subtitles         TEXT    NOT NULL CHECK (subtitles IN ('auto','off','forced')),
  updated_at        INTEGER NOT NULL
);

INSERT INTO playback_preference (id, audio_language, subtitle_language, subtitles, updated_at)
VALUES (1, 'en', 'en', 'auto', 0);
