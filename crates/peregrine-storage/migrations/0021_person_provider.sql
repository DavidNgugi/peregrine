-- Migration 0021: person identity from a provider, and the cached portrait.
--
-- The person table had no provider identifier, so the same actor arriving with every film could
-- only be matched by name, which splits on spelling and merges on coincidence. provider_id is the
-- provider's own stable id for the person; profile_cache_path is where the cached portrait lives,
-- separate from profile_path (the provider-relative path) so a cache eviction does not lose the
-- path the image can be fetched from again.

ALTER TABLE person ADD COLUMN provider_id TEXT;
ALTER TABLE person ADD COLUMN profile_cache_path TEXT;

CREATE UNIQUE INDEX person_provider_idx
  ON person (profile_provider, provider_id)
  WHERE provider_id IS NOT NULL;
