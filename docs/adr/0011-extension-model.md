# ADR 0011 — Extensibility by compile-time traits, not a plugin runtime

- **Status:** Accepted
- **Date:** 2026-10-09
- **Related:** [0002](./0002-rust-core-owns-state.md), [0005](./0005-librqbit-torrent-engine.md), [0009](./0009-licensing-and-redistribution.md)

## Context

The strongest argument against a plugin runtime is a working example. qBittorrent's search engine loads Python plugins fetched over plain HTTPS from a GitHub raw URL with no signature, no hash, and no pinning — only a `.py` extension check — and runs them by shelling out to whatever system Python is on `PATH`. Module-level code in a plugin therefore executes with the user's full privileges, triggered by an ordinary action like "check for updates". That is an unauthenticated remote-code-execution channel living inside the same process tree as the magnet parser and the torrent metadata decoder ([research/reference-codebases.md](../research/reference-codebases.md) §5). We are not going to reproduce it.

Users will want new metadata providers, new subtitle providers, new indexers, and alternative torrent engines. Projects in this space typically answer with a plugin system: a script directory, a JavaScript runtime, or a dynamic library interface. That answer is popular and it is the single largest security liability available to an application that already downloads and executes-adjacent hostile data.

## Decision

**Extensibility is compile-time.** Five ports, each a trait in the core with a shared conformance test suite:

| Port | Implemented by |
| --- | --- |
| `MetadataProvider` | TMDB, TVDB, OMDb, AniList |
| `SubtitleProvider` | OpenSubtitles, Subdl (Podnapisi is rejected: its host no longer resolves) |
| `Indexer` | Prowlarr, Jackett, built-in adapters, local files and magnets |
| `TorrentEngine` | librqbit, with libtorrent as a possible second adapter |
| `PlayerBackend` | mpv, with a fake for tests |

Rules:

1. A new adapter is a **pull request**, not a dropped-in file: adapter, fixture directory, and a provider registry entry. Three files, one review.
2. Every adapter must pass the shared conformance suite before it can be registered. "Behaves like the others" is a test, not a review opinion.
3. **User-configurable and untrusted external systems are reached over documented network APIs** (Prowlarr, Jackett, subtitle providers) rather than by loading code.
4. **No `unsafe` dynamic loading, no script evaluation, no remote code** in the main window or the core, ever. CSP forbids `unsafe-eval`; the trailer webview is sandboxed and has no IPC access.
5. **Indexers ship disabled by default.** Enabling one is an explicit user action with a plain-language risk statement. The project does not curate a default list.
6. If a plugin system is ever revisited, it must be a WASM sandbox with an explicit capability model and no filesystem or network access beyond declared capabilities. That is a separate ADR and an explicit product decision, not a refactor.

## Consequences

Positive: the security surface is bounded and reviewable; performance is identical to hand-written code; the conformance suite gives uniform behaviour and cheap third-party contributions; no runtime dependency on Node, Python, or a scripting engine; the architecture rules in `check-arch` keep adapters at the leaves.

Negative and accepted:

- Adding a provider requires a rebuild, so users cannot add one themselves without compiling. Mitigated by making the guide excellent and the conformance suite complete, so a contributor's first PR is small.
- A provider that breaks due to an upstream change requires a release. Mitigated by shipping provider behaviour as data where possible (endpoints, categories, rate limits in a registry manifest) so many changes are configuration rather than code.
- No third-party ecosystem of extensions. Accepted: this application's threat model does not permit it.

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| A JavaScript or Lua plugin runtime | Executing third-party code inside an application that handles magnets, parsed torrent metadata, and user files is an unacceptable risk, and it makes every sandbox escape a full compromise |
| Dynamic library loading (`.dylib`, `.dll`, `.so`) | ABI instability across releases, no sandbox, and arbitrary native code — worse than scripts |
| A Python sidecar for providers (the Jellyseerr-adjacent pattern) | A second runtime to ship and keep updated, plus an IPC hop and a new failure surface, for providers that are usually a few hundred lines |
| HTTP webhooks or an external provider service | Requires the user to run a service to get metadata; contradicts local-first |
| A WASM plugin system in 1.0 | The right long-term answer if extensibility proves necessary, but a sandbox, a capability model, and an ABI is a project of its own |

## Verification

- Conformance suites exist per port and are required for registration.
- `xtask check-arch` prevents an adapter from being imported anywhere but the composition root.
- Security review asserts the absence of `unsafe` dynamic loading and of any remote code path in the webview (CSP test in Phase 4).
