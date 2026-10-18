-- Where a title can be watched, per region, from a metadata provider's availability answer.
--
-- Three tables rather than one denormalized row because the answers have two keys a screen filters
-- on independently: a region (the whole section) and a category (one row of logos). watch_sync is
-- separate so "the provider answered, and this title is not streaming anywhere here" is
-- distinguishable from "nobody has asked", which is the difference between an empty section and a
-- section that has not been fetched.

CREATE TABLE watch_sync (
  title_id  INTEGER PRIMARY KEY REFERENCES title(id) ON DELETE CASCADE,
  synced_at INTEGER NOT NULL
);

CREATE TABLE watch_availability (
  title_id INTEGER NOT NULL REFERENCES title(id) ON DELETE CASCADE,
  region   TEXT NOT NULL CHECK (length(region) = 2 AND region = upper(region)),
  source   TEXT NOT NULL,
  link     TEXT,
  PRIMARY KEY (title_id, region)
);

CREATE TABLE watch_offer (
  title_id        INTEGER NOT NULL,
  region          TEXT NOT NULL,
  category        TEXT NOT NULL CHECK (category IN ('flatrate', 'rent', 'buy', 'free', 'ads')),
  provider_id     INTEGER NOT NULL,
  provider_name   TEXT NOT NULL CHECK (length(provider_name) > 0),
  logo_path       TEXT,
  display_priority INTEGER,
  PRIMARY KEY (title_id, region, category, provider_id),
  FOREIGN KEY (title_id, region) REFERENCES watch_availability(title_id, region) ON DELETE CASCADE
);

CREATE INDEX watch_offer_title_idx ON watch_offer (title_id, region);
