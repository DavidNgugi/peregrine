# ADR 0002 — The Rust core owns state; the UI is a projection

- **Status:** Accepted
- **Date:** 2026-10-09
- **Related:** [0001](./0001-desktop-shell-tauri.md), [0003](./0003-sqlite-and-fts5.md)

## Context

The default failure mode of a webview-based desktop app is a TypeScript application that believes it owns the library, with the backend as a thin filesystem proxy. That produces two sources of truth, stale caches the UI must reconcile, and a UI that stalls when the backend is busy.

## Decision

1. **The Rust core is the only writer.** Every mutation goes through a typed IPC command and is applied to SQLite inside a transaction. The UI holds no authoritative state.
2. **The UI holds projections and freshness metadata only.** TanStack Query caches query results; Zustand holds ephemeral UI state (open menus, hover, selection, scroll anchors). Neither persists anything authoritative.
3. **Domain logic lives in `peregrine-domain`**, a crate with no IO, no async, and no clock. IDs and time are injected ports, so every rule is deterministic in tests.
4. **Adapters are leaf crates behind traits** (`MetadataProvider`, `TorrentEngine`, `PlayerBackend`, `SubtitleProvider`, `Indexer`). A service composes ports; it never constructs a client.
5. **IPC commands are thin** — parse, call one service, map the result — and anything slower than 100 ms becomes a job that streams progress events.
6. **Change notification is an explicit invalidation contract** (`library://changed` carrying affected keys), not a general "something changed" broadcast that forces a global refetch.
7. **The dependency direction is machine-enforced** by an allowlist checked in `xtask check-arch`, so "the UI does not own state" is not a code-review opinion but a failing build.

## Consequences

Positive: one source of truth; the UI stays responsive because slow work is a job; rules are testable without a GUI or a network; the core can be reused for a CLI or a companion service later.

Negative: more IPC surface to define and version; the binding generation step is mandatory; developers must resist the temptation to cache derived state in the UI for convenience.

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| UI owns state, backend is a thin FS/RPC proxy | Two sources of truth, reconciliation bugs, and a UI that stalls while the backend works |
| A single language (all Rust or all TypeScript) | Rust UI toolkits cannot reach this level of visual polish cheaply; an all-TypeScript backend would put a torrent parser and a download engine in a memory-unsafe runtime that also has to ship Node |
| Shared state through a filesystem watch or a local socket with ad-hoc JSON | Loses typing, transactional integrity, and error structure |
| GraphQL or a REST API between the layers | Adds a second schema language and network semantics to a single-process app |

## Verification

- `xtask check-arch` fails on a deliberately illegal dependency edge.
- `peregrine-domain` has no `tokio`, `sqlx`, `reqwest`, or `tauri` in its dependency graph; a test asserts the allowlist.
- IPC contract tests prove every command's error path, and the payload guard logs any response over 64 KB in development.
