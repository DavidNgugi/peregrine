# ADR 0005 — librqbit in-process behind a TorrentEngine port

- **Status:** Accepted, contingent on `SPIKE-3`
- **Date:** 2026-10-09
- **Related:** [0011](./0011-extension-model.md), [0012](./0012-streaming-readahead.md), [0009](./0009-licensing-and-redistribution.md)

## Context

The acquisition feature needs a BitTorrent engine with: magnet and `.torrent` intake, DHT and peer exchange, per-file selection at add time, sequential download with a read-ahead window we can steer from the player's read position, bandwidth and connection limits applied at runtime, proxy or interface binding for leak safety, clean shutdown with resumable state, and a licence that permits distribution inside an MIT/Apache application.

Three shapes were considered: a Rust engine in-process, C++ libtorrent bindings, or a separate daemon process (aria2c, rtorrent, Transmission) driven over its API.

## Decision

Use **librqbit in-process**, behind a `TorrentEngine` trait in the core, with the adapter as a leaf crate.

- The trait covers session lifecycle, add with file selection, sequential streaming with a steerable priority window, statistics, limits, pause and resume, and removal.
- The safety inspection gate runs **before** the engine is given the torrent, on metadata only, so a hostile torrent never results in a written byte.
- Resume state is persisted per job (`engine_state_json`) so a restart reattaches instead of re-downloading.
- The engine never receives a path it has not been given through `PathGuard`.
- libtorrent bindings remain a legitimate second adapter if `SPIKE-3` finds a hard gap (controllable read-ahead is the one to watch); the trait exists so that swap is a leaf-crate change, not a rewrite.

## Consequences

Positive: memory safety in the component parsing hostile input; no C++ toolchain or Boost per platform; per-file selection and piece priorities adjustable from the same process that reads the player's position, which is the entire point of the streaming design; one less IPC hop and one less process to supervise.

Negative and mitigated:

- Engine capability gaps are our problem. Mitigated by the trait boundary and by `SPIKE-3` running first, in Phase 0, so a gap surfaces before the UI is built on it.
- Indexer and search features beyond BitTorrent (RSS, magnet resolution services) are partly ours to build. Mitigated by scoping RSS ingest into the automation epic, not 1.0.
- Seeding policy and ratio accounting are our responsibility. Mitigated by making them domain rules with tests rather than engine pass-through.

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| libtorrent-rasterbar via FFI | Mature and fast, but a C++ build per platform, `unsafe` FFI across a large surface, more complex bindings generation, and it does not reduce our work on read-ahead steering, which happens in our process either way |
| aria2c sidecar | What the Peeri reference does. Buys maturity, loses control: no piece-level priority steering from the player's read position, which is exactly the differentiator here. Also GPL-2.0, so it must stay out of process with a source offer |
| rtorrent or Transmission daemon over RPC | Same control loss plus a daemon to install, configure, and supervise, with its own resource footprint |
| WebTorrent (as the torlink reference uses) | Requires a JavaScript runtime in the backend and cannot express the priority control we need |
| Writing a BitTorrent engine | Not a reasonable use of the project's time |

## Verification

`SPIKE-3` (3 days) must confirm on the pinned version, on all three platforms: per-file streaming with a controllable read-ahead window; SOCKS5 and interface binding; clean shutdown and restart reattachment; and per-file selection changes after add. A failure on read-ahead steering triggers the second adapter, not a redesign.
