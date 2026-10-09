# ADR 0003 — SQLite with FTS5 for storage and search

- **Status:** Accepted
- **Date:** 2026-10-09
- **Related:** [0002](./0002-rust-core-owns-state.md), [0006](./0006-url-first-caching.md)

## Context

Peregrine stores a relational catalogue (titles, seasons, episodes, credits, external IDs), a library (items, files, subtitles), acquisition state (jobs, files, decisions), user state (progress, watchlist), and a large amount of disposable cache. It also needs instant local search over 10 000–100 000 titles. The whole thing must be backup-able, inspectable in a bug report, and fast enough to render a grid in under 5 ms per page.

## Decision

One **SQLite** database in WAL mode accessed through `sqlx`, with:

- Catalogue and library in normalized tables with partial and covering indices for the hot screens ([DATA_MODEL.md](../DATA_MODEL.md) §4).
- Search in a **contentless FTS5** table maintained explicitly inside catalogue write transactions, ranked with `bm25` weights plus popularity as a tiebreak.
- Disposable data in the same file but clearly separated tables (`provider_cache`, `image_cache`) so it can be dropped or wiped without touching user data.
- Exactly one writer connection serialized through a channel; readers use a small pool; `busy_timeout`, `synchronous=NORMAL`, `foreign_keys=ON`.
- Append-only numbered migrations with expand-contract for breaking changes, and fixture databases from every prior release in the migration test suite.

## Consequences

Positive: relational queries with real joins; built-in ranked search with no second process; readers never block the writer; one file to back up, attach to a bug report, or copy for portable mode; any maintainer can inspect it with standard tooling.

Negative and mitigated:

- Single-writer serialization is a real constraint for a queue writing progress from 20 jobs. Mitigated by coalescing progress writes to ≤1 Hz per job and keeping network calls out of write transactions. A Phase 2 load test with 20 concurrent jobs is the gate.
- FTS5 must be maintained by hand because triggers cannot cheaply aggregate aliases, keywords, and cast names. Mitigated by a single repository path that owns catalogue writes, plus a `rebuild-fts` maintenance command verified to reproduce identical results.
- Schema migrations are forever-applied. Mitigated by immutability checks, fixture upgrades, and refusing to open a newer schema rather than corrupting it.

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| `sled` / `redb` (embedded KV) | No query language and no full-text search; the access patterns are relational, so joins would move into application code |
| RocksDB | A C++ dependency and LSM tuning burden at a scale where SQLite is comfortable |
| DuckDB | Analytics-optimized; weak at the many small transactional writes that dominate here |
| PostgreSQL or a local server | A service to install, run, and secure for a single-user desktop app |
| Tantivy or Meilisearch for search | A second index to keep consistent with the database, and a rebuild path that can silently diverge |
| A JSON or TOML file for the library | The standard route to a corrupted library with no transactional recovery |

## Verification

- `SPIKE-5` measures FTS5 ranking quality and p95 latency at 100 k titles; if it fails, the trigram tokenizer replaces the fallback table.
- Benches gate the hot queries in CI at their budgets.
- Migration tests upgrade from every prior release fixture.
