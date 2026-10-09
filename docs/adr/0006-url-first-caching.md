# ADR 0006 — Cache URLs and identifiers, never bytes we can refetch

- **Status:** Accepted
- **Date:** 2026-10-09
- **Related:** [0003](./0003-sqlite-and-fts5.md), [0008](./0008-incremental-polite-sync.md)

## Context

A media catalogue means thousands of posters, backdrops, and episode stills, plus provider JSON, subtitle files, and probe results. The obvious implementation stores the images in the database or in a flat directory and treats them as data. That is how these apps grow to tens of gigabytes of cache, break when a provider changes a path, and become impossible to back up or move.

## Decision

Five cache tiers with a strict rule: **the database stores references and lifecycle metadata; bytes live in a disposable store.**

| Tier | Holds | Lifetime |
| --- | --- | --- |
| Memory LRU | Hot DTOs and provider responses | Process |
| SQLite | Normalized entities, `provider_cache` rows with ETag and TTL, `image_cache` index, job journal, resume points | Until pruned |
| Disk image cache | Poster, backdrop, and still bytes at the variants actually displayed, content-addressed by `(provider, path, variant)` | LRU to a user-set cap, default 1 GB |
| Disk dataset | IMDb ratings only: 8.7 MB compressed, ~45 MB in SQLite | Replaced wholesale |
| HTTP conditional | `If-None-Match` on every provider call | Per response |

Rules that follow from the decision:

1. Artwork rows store provider, path, dimensions, language, vote metadata, and a ~40-byte blurhash. The blurhash is the **only** image-derived artifact kept permanently, because the first paint cannot wait for a network round trip.
2. **Image paths are immutable and are never revalidated.** Verified against TMDB's CDN: it returns no usable `ETag`, ignores `If-None-Match`, and answers `If-Modified-Since` with `200` for both past and future timestamps, while serving `max-age` of roughly 369 days. A new asset gets a new `file_path`. So image cache eviction is purely **LRU by size budget**, never TTL, and there is no conditional-request path to optimize. JSON endpoints are the opposite: they revalidate properly, and storing their `ETag` or `Last-Modified` is mandatory.
3. Cache only the variants that are displayed — `w342` posters (about 64 KB) and `w780` backdrops (about 291 KB) — and resize once on ingest rather than storing originals.
4. Cache keys are content-addressed, so a provider renaming a path orphans nothing and a cache wipe is always safe.
5. Every cache is independently droppable with no user-data loss — this is an asserted invariant (DATA_MODEL I9), not a hope.
6. Resizing happens once, on ingest, into the variant the UI asked for. A 1280 px backdrop is never downloaded for a 300 px tile. TMDB exposes no imgix-style query parameters — the size lives in the path segment only.
7. Disk-budget guardrails are explicit: image cache capped and user-visible; any optional dataset states its measured size before it can be enabled; the free-space floor blocks downloads rather than silently filling the disk.
8. Trailers are metadata plus an embedded player on the privacy-enhanced `youtube-nocookie.com` domain, discovered from TMDB's `videos` payload. Video bytes are never stored, and the YouTube Data API is not used at all: `search.list` is capped at 100 calls per day, which makes search-driven trailer lookup architecturally unacceptable. An offline-trailer option, if it ever ships, requires a **user-installed** `yt-dlp` invoked as a separate process — the PyInstaller bundles are GPLv3+, so bundling one is forbidden by AGENTS R25 — and it is off by default, documented, and outside the app's cache budget.

## Consequences

Positive: a library of 5 000 titles browses fully with roughly 200 MB of cached images; the database stays in the tens to low hundreds of megabytes; backups are small; a cache wipe is a supported operation rather than a data-loss event; disk usage is explainable per tier in Diagnostics.

Negative and accepted:

- Cache misses cost a round trip, so hover and prefetch policy matter. Mitigated by bounded idle-time prefetch and by never prefetching during scroll or while a download saturates the link if the user disables it.
- Provider path changes invalidate cached bytes. That is correct behaviour and cheap, because the placeholder is already on screen.
- The blurhash generation step adds work at catalogue ingest. It is sub-millisecond for a thumbnail and happens on a blocking thread.

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| Store image bytes as SQLite BLOBs | Bloats the database and its WAL, makes backup and portable mode painful, and gains nothing over the filesystem for files with one writer |
| Cache only in memory | Every cold start re-downloads thousands of images; unusable offline |
| Never cache images, always fetch | Terrible perceived performance and rude to provider CDNs, which risks rate limiting or blocking |
| A generic HTTP proxy cache for everything | Loses the ability to wipe one tier, and makes "what is this 4 GB?" unanswerable for the user |
| Download and store full-size artwork for fidelity | Wastes disk and bandwidth for pixels no screen displays |

## Verification

- Invariant test: wipe both cache tiers, relaunch, assert the app renders from the catalogue and repopulates.
- Diagnostics totals must match a filesystem measurement within 2%.
- Budget assertions in CI for cache-disk usage per tier.
