-- Migration 0013 — the pin an in-use image carries, and the index the eviction scan reads.
--
-- PG-E6-10's acceptance is "cache never exceeds the cap; in-use images never evicted", and the second
-- half needs somewhere to say an image is in use. The first version of the table had no such column,
-- and no way to derive one: `last_used_at` says when an image was last *read*, not whether something
-- is holding it now, and a policy that guessed from recency would evict the poster on screen the
-- moment the user stopped scrolling.
--
-- `pinned` is a flag rather than a reference count. A count would have to be decremented by every
-- holder on every path, including the ones that end in a panic or a cancelled download, and a leaked
-- count is an image that can never be evicted — a worse failure than evicting one that had just
-- stopped being used. The pin is held by whoever displays or is fetching an image and cleared when
-- they are done; the eviction policy refuses to touch a pinned row at all.
ALTER TABLE image_cache ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1));

-- The eviction scan, which reads exactly this: the evictable entries in the order the policy takes
-- them.
--
--   SELECT key, bytes, last_used_at, pinned FROM image_cache
--    WHERE pinned = 0 ORDER BY last_used_at ASC, key ASC
--
-- **Partial, and deliberately not the index 0007 already has.** `image_cache_lru_idx` covers
-- `last_used_at` alone and cannot serve this order — a plan that used it would still have to sort,
-- because the tie-break is `key` and the filter is `pinned`. This one is over the four columns the
-- scan needs, restricted to the rows it may touch, so the pinned rows an in-use image leaves behind
-- cost nothing to carry: the entries are walked in the order they are evicted, and a plan that needs
-- no temporary b-tree is the whole reason it exists.
CREATE INDEX image_cache_eviction_idx ON image_cache (last_used_at, key) WHERE pinned = 0;
