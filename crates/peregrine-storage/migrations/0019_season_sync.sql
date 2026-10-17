-- Migration 0019: whether a show's seasons have ever been read, and the two repairs that
-- fact makes possible.
--
-- # Why the column exists
--
-- A series is filled in two reads: the title's own details, then its season list and each
-- season's episodes. Only the second one writes the season and episode tables. Until now
-- nothing recorded that the second read had happened, and artwork::refresh_due read the first
-- one as the whole answer: metadata_state becomes 'partial' as soon as the details land, and a
-- poster in the cache made every later visit say "filled in, nothing to ask". So a show whose
-- details read happened before the seasons slice existed, or whose season read failed after the
-- details were written, was a show with no seasons that no visit would ever ask about again.
--
-- season_synced_at is that missing fact. It is written only after a provider has answered the
-- season step, and it is what lets the rule separate "a show with no seasons" from "a show
-- nobody has asked about the seasons of".
--
-- # The two repairs
--
-- Both are data, not schema, and both are one-time. Neither can be done by a later read,
-- because the state they repair is exactly the state that stops the read from happening.
--
-- 'partial' was stamped by *list* answers before 0014 introduced 'listed', and 0014 copied the
-- table without correcting the rows it invalidated, so a title a rail listed before that
-- migration has claimed a details read ever since. Only a details read writes any of the nine
-- columns below, so a 'partial' row with all nine empty is one no details read produced. A
-- details answer that carried none of them is asked for once more; the alternative is a row
-- that goes on claiming a read nobody did.
--
-- A show that already holds a season row has been answered, so the new column is backfilled from
-- the timestamp of that answer rather than left empty. Otherwise every show the catalogue
-- already knows would be asked about its seasons once more, which is the request spend this
-- column exists to avoid.

ALTER TABLE title ADD COLUMN season_synced_at INTEGER;

UPDATE title
   SET metadata_state = 'listed'
 WHERE metadata_state = 'partial'
   AND original_name IS NULL
   AND tagline IS NULL
   AND runtime_minutes IS NULL
   AND original_language IS NULL
   AND origin_country IS NULL
   AND popularity IS NULL
   AND season_count IS NULL
   AND episode_count IS NULL
   AND homepage IS NULL;

UPDATE title
   SET season_synced_at = COALESCE(last_synced_at, updated_at)
 WHERE id IN (SELECT title_id FROM season);
