# ADR 0008 — Incremental, polite sync; local-first with no accounts

- **Status:** Accepted
- **Date:** 2026-10-09
- **Related:** [0007](./0007-metadata-sources.md), [0006](./0006-url-first-caching.md)

## Context

"IMDb realtime sync" is the ask. Taken literally it would mean continuous polling of a catalogue with tens of thousands of titles, which is both impolite to the provider and pointless: ratings change daily, episodes are announced days ahead, and nothing a user does requires sub-second metadata freshness. At the same time, the app must feel live — a title's data should be current when it matters, and new episodes and releases should appear without the user thinking about it.

## Decision

Define freshness by **what the user can perceive**, and implement it with incremental change detection plus conditional requests.

| Signal | Cadence | Mechanism |
| --- | --- | --- |
| Trending and discovery rails | 30–60 min while the app is in use | Discovery endpoints with ETag; skipped entirely when the app is idle |
| Changed titles | Every 24 h | `/changes` windows with a persisted cursor and a one-day overlap for self-healing. The API reports the past 24 hours, so polling faster cannot observe anything new. With a TVDB key, `/updates?since=<epoch>` is genuinely continuous and becomes the faster path |
| Ratings | Every 24 h | IMDb `title.ratings.tsv.gz` with a HEAD ETag check; a 304 costs nothing |
| A title the user opens | On view, if past its TTL | Stale-while-revalidate: render cached, refresh in the background |
| A title on the watchlist | Every 12 h | Higher priority class; also drives new-episode detection in the automation epic |
| Artwork | Never by TTL | Paths are immutable and non-revalidatable, so images are fetched once per variant and evicted by LRU size only |
| Episode air dates | With the parent show's refresh | No separate schedule |
| Cache compliance sweep | Every 180 days | Refresh or purge cached metadata to stay inside TMDB's six-month ceiling ([0007](./0007-metadata-sources.md)) |

Rules:

1. **The UI never waits for the network.** Every screen renders from local state; a refresh only ever fills in.
2. **Never poll faster than the source can change.** A 24-hour change window polled hourly is 24 wasted requests per day, and it is the fastest way to earn a `429`.
2. **Every fetch is a job** with a dedupe key, a priority class, a resource token, retry with exponential backoff and jitter, and cooperative cancellation.
3. **Conditional requests are mandatory.** A revalidation that returns 304 refreshes the TTL and transfers no body.
4. **Rate limits are respected structurally**, with per-host token buckets that park on `429` or `Retry-After` instead of retrying into the wall. Where a provider documents only a vague ceiling — TMDB says "somewhere in the 40 requests per second range" and TVDB v4 documents nothing — the limiter starts conservative and adapts downward on evidence rather than assuming a number we cannot verify (`SPIKE-10`).
5. **Backoff never becomes a hammer:** a failing provider trips a circuit breaker, is marked degraded, and is surfaced in Diagnostics rather than retried in a loop.
6. **Offline is a first-class state.** No error modals, no job storms; the app says how stale its data is and keeps working.
7. **No accounts and no telemetry.** The only optional outbound identity is a user-configured Trakt token for progress sync, and that is post-1.0.
8. **Nothing user-visible depends on a schedule being met.** If the app was closed for a month, launching it queues one coalesced catch-up window, not thirty.

## Consequences

Positive: provider load is proportional to change rather than to catalogue size; ratings are current within a day, which is what a user actually notices; the app is fully functional offline; no account, no sign-in, no server to run; the sync engine is testable because it is deterministic given a clock and a fixture.

Negative and accepted:

- Data is at most a day stale for ratings and up to six hours stale for a newly changed title. This is a deliberate trade, documented in the UI as "last updated".
- The first run after install is a real workload: rail hydration plus an initial ratings import. Mitigated by making it a visible, cancellable job with progress, and by page-rendering from whatever exists.
- Incremental sync requires CORS-free server-side cursor discipline. Mitigated by persisting the cursor before doing the work and overlapping windows.
- Watchlist monitoring is a scheduler feature with real cost at scale; it is scoped to the automation epic rather than 1.0.

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| Poll the full catalogue on a timer | Impolite, rate-limited, and wasteful; the provider would degrade us or block us |
| An account-based sync service | Contradicts local-first and adds a server the project would have to run and pay for |
| Webhooks or a push feed | No provider offers one for this data at this tier |
| Websocket streaming of metadata | No source exists; would be inventing a protocol |
| Refresh everything on every launch | Slow cold start, provider abuse, and it makes the app feel like it is always waiting on the network |
| A CDN or a proxy maintained by the project | Changes the project from a client into a service, with the legal and operational burden that implies |

## Verification

- Phase 1 tests: crash between cursor advance and upsert; 429 with `Retry-After`; provider 500 storm; schema drift; airplane mode.
- A load test at 5 000 titles with a throttled network must complete within the DB and disk budgets without a UI stall.
