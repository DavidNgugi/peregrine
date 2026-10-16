-- The default profile is seeded once, and insert_default only inserts when the table is
-- empty, so an install that predates the 1080p default would keep its 2160p-first ordering
-- forever. Rewrite it in place.
--
-- Safe only because no profile editor exists (PG-E12-07): the row cannot have been user-edited,
-- so it still holds the built-in rules. The guard matches the exact resolutions the old build
-- seeded, so the moment an editor exists a profile it wrote is left alone.
--
-- updated_at is deliberately untouched: a migration has no clock, and the column records when
-- the application last wrote the row.
UPDATE quality_profile
   SET rules_json = json_set(
         json_set(rules_json, '$.resolutions', json('["1080p","720p","576p","480p"]')),
         '$.size_caps.ceiling',
         12884901888
       )
 WHERE is_default = 1
   AND name = 'Default'
   AND json_extract(rules_json, '$.resolutions')
       = json('["2160p","1440p","1080p","720p","576p","480p"]');
