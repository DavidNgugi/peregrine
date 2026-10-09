# ADR 0007 — Metadata sources: TMDB primary, IMDb datasets for ratings, no scraping

- **Status:** Accepted
- **Date:** 2026-10-09
- **Related:** [0006](./0006-url-first-caching.md), [0008](./0008-incremental-polite-sync.md), [0009](./0009-licensing-and-redistribution.md)

## Context

Discovery needs titles, overviews, genres, keywords, cast and crew, seasons, episodes, episode stills, artwork, trailers, age ratings, and IMDb-style ratings that stay current. The project is open source, must not require a paid API, and must not violate the terms of the services it depends on. Exact endpoint shapes, limits, and response fields are recorded in [research/data-sources.md](../research/data-sources.md).

## Decision

| Data | Source | Role |
| --- | --- | --- |
| Catalogue: titles, seasons, episodes, credits, artwork, trailers, keywords, age ratings, discovery rails | **TMDB API** | Primary and effectively required |
| Ratings and vote counts on an IMDb scale | **official IMDb datasets** (`title.ratings.tsv.gz`), downloaded by the user and joined locally | Primary for ratings; never redistributed |
| Alternative and regional titles | TMDB aliases; IMDb `title.akas` (493 MB compressed, 2.5 GB cap) only if the user opts in | Secondary, disabled by default |
| Anime identity and absolute ordering | TMDB episode groups first; AniList or MyAnimeList as an optional key | Optional |
| Fallback metadata and ratings when a dataset is not yet downloaded | OMDb, user-supplied key | Optional fallback only, never a primary path |
| Additional season and episode data | TVDB v4, user-supplied key | Optional |

Rules that follow, and that are architectural rather than aspirational:

1. **No scraping.** Every provider is a documented API. If a source has no API, it is not a source. This is why there is no IMDb page scraper and no torrent-site scraper in the core.
2. **No provider type escapes its adapter module.** TMDB types stop at the adapter boundary. This is a direct lesson from the seerr reference, where TVDB endpoints internally call the TMDB client, coupling two providers permanently ([research/reference-codebases.md](../research/reference-codebases.md)).
3. **Provider results are normalized into domain types** with explicit capability declarations, rate limits, cache policy, and attribution requirements.
4. **Attribution is a first-class requirement**, not a footer afterthought. TMDB's terms require its logo to identify the use of its APIs and this notice verbatim: "This application uses TMDB and the TMDB APIs but is not endorsed, certified, or otherwise approved by TMDB." It appears in About, in the README, and wherever its data is presented, and any other provider requiring a notice gets one.
5. **A provider can be disabled without breaking the catalogue.** Existing rows keep rendering; only refreshes stop.
6. **Conditional requests on every JSON endpoint**, with `ETag` or `Last-Modified` stored per cached response so revalidation is free when nothing changed. The image CDN is the documented exception: it exposes no usable `ETag` and ignores conditional headers, so image paths are treated as immutable and evicted by LRU size only ([0006](./0006-url-first-caching.md)).
7. **IMDb `title.basics` is deliberately not used.** TMDB's `external_ids` endpoint returns the `imdb_id` directly and `/find/{external_id}` reverses it, so the 1.04 GiB dataset buys nothing for ID mapping. The datasets contain no TMDB ids, so any alt-title import must be re-keyed through the TMDB `imdb_id`.
7. **Stale is a state, not an error.** A failed refresh marks a row stale and keeps serving cached data with an affordance to retry.
8. **No user accounts, no API keys shipped in the binary.** The user supplies keys in Settings; they live in the OS keyring.
9. **TMDB's six-month cache ceiling is a compliance requirement, not a nicety.** Cached metadata is refreshed or purged on a scheduled sweep at or before 180 days, and a documented purge path exists for the termination clause, which requires deleting all cached TMDB content on request.

## Consequences

Positive: a legal, free, well-documented metadata foundation; ratings that update daily without scraping; no vendor lock beyond one required provider; the ability to add a provider (AniList, TVDB) without touching the UI.

Negative and accepted:

- A TMDB API key is required of the user, which is onboarding friction. Mitigated by a clear wizard step with a link to the signup page and an explanation of why.
- IMDb datasets are bulk downloads, so they must be scheduled politely and budgeted on disk. Mitigated by the disk-budget model in [research/data-sources.md](../research/data-sources.md) and by making the large files optional.
- TMDB does not have everything for anime; absolute ordering needs a secondary source for some titles. Mitigated by the ordering overlay model in [DATA_MODEL.md](../DATA_MODEL.md) §2.3, which does not require the primary source to be complete.
- Dataset freshness is daily rather than live. "Realtime" here means incremental change detection, not a streaming feed ([ADR 0008](./0008-incremental-polite-sync.md)).

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| Scrape IMDb or a tracker site for metadata | Violates terms, breaks constantly, and puts the project's legal footing at risk |
| TVDB as primary | Requires a user key and a different commercial posture; weaker artwork and trailer coverage |
| OMDb as primary | Paid tiers for real volume, thin catalogue data (no seasons, episodes, or credits at the depth needed) |
| A self-hosted metadata database maintained by the project | Operational cost and a redistribution/licensing problem the project should not take on |
| Bundling ratings inside the app binary | Redistribution of IMDb datasets is not permitted under their terms; the user downloads them |
| A third-party aggregator API (Simkl, Trakt) as the catalogue | They are progress and calendar services, not a catalogue; using them for metadata would add an account requirement to a local-first app |

## Verification

- Provider conformance suite plus recorded fixtures with a CI leak check proving no keys or user data are committed.
- `SPIKE-4` and `SPIKE-5` validate the placeholder and search consequences of this choice.
- Phase 1 exit criteria include exercising every failure mode in ARCHITECTURE §15 against a fixture.
