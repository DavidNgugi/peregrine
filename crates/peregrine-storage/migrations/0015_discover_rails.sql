-- Migration 0015: the Discover rails' membership, persisted (PG-E7-08).
--
-- `HomeRails` is an in-memory map, and the cost of that was stated when it landed: a cold
-- start drew every rail empty until a fetch answered, because membership lived only in the
-- runtime. Nothing about it is a per-launch question — seven lists a provider publishes
-- change on the provider's clock, not on the user's — so the launch fetched all seven again,
-- and the screen that exists to paint first waited on the network to paint at all.
--
-- This table is that membership and nothing else. Everything a rail *draws* — the name, the
-- year, the score, the synopsis, the poster and backdrop paths — is on the `title` row the
-- list answer already wrote, and the cached blob's path is a `stat` on the cache, so a
-- manifest that copied any of them would be a second copy of the catalogue, free to drift
-- from the first. `position` is the rail's own order, which is the one fact the catalogue
-- cannot express.
--
-- `PRIMARY KEY (rail, position)` makes the order a rail's identity — two titles cannot share a
-- place — and `UNIQUE (rail, title_id)` makes "each title once per rail" the database's, which
-- is what a provider repeating itself in one page would otherwise violate. The index on
-- `title_id` is the one the `ON DELETE CASCADE` walks: deleting a title must not scan every
-- rail to find the places it held.
CREATE TABLE discover_rail (
  rail     TEXT    NOT NULL,   -- a `RailKind` spelling: trending_week, popular_movies, ...
  position INTEGER NOT NULL,   -- the place in the rail, from zero
  title_id INTEGER NOT NULL REFERENCES title(id) ON DELETE CASCADE,
  PRIMARY KEY (rail, position),
  UNIQUE (rail, title_id)
);

CREATE INDEX discover_rail_title_idx ON discover_rail (title_id);
