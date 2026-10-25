# Peregrine — System Architecture

Peregrine is a cross-platform desktop application for media discovery, library management, torrent acquisition, and playback. This document is the architectural source of truth. Data structures live in [DATA_MODEL.md](./DATA_MODEL.md), dynamic behaviour in [FLOWS.md](./FLOWS.md), decisions and their rejected alternatives in [adr/](./adr/), the backlog in [TASKS.md](./TASKS.md).

Status: target architecture, partly built. What works today is in the [README](../README.md); [TASKS.md](./TASKS.md#implementation-status) records what each task still owes. The behaviours below are requirements, not claims that every subsystem is available. Every load-bearing claim that still needs empirical confirmation is tagged `SPIKE-n` and listed in [§21](#21-open-spikes).

---

## 1. Scope

### In scope

- Discovery of movies, series, and anime from free metadata providers, with cached, instant, filterable browsing.
- Local library: scan, match, organize, play, delete.
- Torrent and Usenet acquisition with quality profiles, a queue, and progress surfaced per card and in a dedicated screen.
- Playback through mpv with resume, next-up, subtitles, audio-track and language management.
- Watchlist, watch progress, collections.
- Casting to Chromecast and DLNA renderers.
- Settings, first-run setup, permissions, themes.
- macOS, Windows, Linux (X11 and Wayland), packaged and auto-updating.

### Explicit non-goals

| Non-goal | Why |
| --- | --- |
| Hosting or indexing content ourselves | Legal exposure and operational cost. Any indexer integration is user-configured — including its URL, with no default domain list shipped — and disabled by default. |
| User accounts, cloud sync of library contents | The app is local-first. Optional third-party progress sync (Trakt) is the only outbound identity. |
| A plugin runtime that executes third-party code | Remote code execution inside a torrent client is the single worst security decision available to us. Extensibility is compile-time traits, not loaded scripts. |
| Telemetry by default | Opt-in crash reports only, scrubbed, self-hostable. |
| Transcoding on the desktop playback path | mpv plays essentially every container and codec directly. Transcoding burns CPU for no benefit. Transcode exists only for casting, where the renderer forces it. |
| A media server for other devices **in 1.0** | Not in scope for 1.0: the desktop app stays a client. A server mode is planned for Phase 7 and specified in [adr/0018](./adr/0018-media-server.md) — folder export and DLNA first, then a Jellyfin-compatible API subset and a headless service. It is deliberately after 1.0 because a long-lived listener is a different security posture and depends on the core being stable. The first slice of that client — the TV web surface — is served by the running desktop app under [adr/0028](./adr/0028-tv-web-client.md): off by default, bound to one address, read-only unless an approval grants the separate acquisition scope, and reachable only on the local network |

---

## 2. Design principles

| # | Principle | Enforcement |
| --- | --- | --- |
| P1 | **The Rust core owns truth.** The UI is a projection of core state, never the origin of it. | All mutations flow through typed IPC commands. No business logic in TypeScript. |
| P2 | **Domain is pure.** Entities, invariants, scoring, naming parsers, and state machines live in a crate with no IO, no async, no clock. | `peregrine-domain` forbids `tokio`, `sqlx`, `reqwest`, `tauri`, `std::fs` by dependency allowlist check in `xtask`. |
| P3 | **Ports and adapters.** Every external system (TMDB, librqbit, mpv, ffmpeg, keyring, indexer) sits behind a trait in the core and an adapter in a leaf crate. | Adapters are the only crates allowed to name third-party client types. |
| P4 | **URLs and IDs in the database, bytes in a disposable cache.** Never store an image we can refetch. | Poster rows store provider path, dimensions, and a ~40-byte blurhash. Only the LQIP placeholder is durable; image CDN paths are immutable and never revalidated. |
| P5 | **Cache-first, always renderable, never blank.** Every screen has a cached state; the network only ever fills in. | TanStack Query with persisted cache; skeletons require a cached placeholder, not a spinner. |
| P6 | **Local-first, offline-tolerant.** With the network unplugged, library, playback, subtitles already stored, and downloads in progress all keep working. | Offline mode degradation matrix in [§15](#15-degradation-and-error-model). |
| P7 | **No unsafe.** | `#![forbid(unsafe_code)]` in every crate. A single FFI crate may opt out with a written justification; there is currently no such crate. |
| P8 | **No surprise network egress.** Every host we contact is enumerable and user-inspectable. | Provider registry is a single manifest; a diagnostics screen lists every endpoint contacted. |
| P9 | **Latency is a feature.** Budgets are enforced in CI, not aspirational. | [§14](#14-performance-architecture) budgets wired into benches and E2E assertions. |
| P10 | **Destructive actions are reversible or confirmed.** | Deleting media requires typed confirmation listing files and bytes; nothing else deletes user data. |
| P11 | **Every module is testable without a GUI or a network.** | Fakes for every port; `wiremock` fixtures for HTTP; loopback seeder for torrents. |
| P12 | **Zero comments that restate code.** Names and types carry intent; comments carry only non-obvious *why*. | Review rule in `AGENTS.md`. |

---

## 3. System context

Who and what Peregrine talks to.

```mermaid
graph TB
    User["User<br/>keyboard + mouse"]
    subgraph App["Peregrine desktop app"]
        UI["Discovery / Library / Player UI"]
        Core["Rust core"]
    end

    subgraph External["External systems"]
        TMDB["TMDB API<br/>metadata, images, trailers"]
        TVDB["TVDB v4<br/>optional, user key"]
        OMDB["OMDb<br/>optional fallback"]
        IMDB["IMDb datasets<br/>daily ratings + id map"]
        Subs["Subtitle providers<br/>OpenSubtitles, Subdl"]
        YT["YouTube<br/>embedded trailers"]
        Trackers["BitTorrent swarm<br/>DHT, trackers, peers"]
        Indexers["User-configured indexers<br/>Prowlarr / Jackett / built-ins off"]
        Trakt["Trakt<br/>optional progress sync"]
        Cast["Chromecast / DLNA renderers<br/>LAN"]
        FS["Local filesystem<br/>library roots, download dir"]
    end

    User --> UI --> Core
    Core --> TMDB
    Core --> TVDB
    Core --> OMDB
    Core --> IMDB
    Core --> Subs
    Core --> YT
    Core --> Trackers
    Core --> Indexers
    Core --> Trakt
    Core --> Cast
    Core --> FS
```

### Trust boundaries

| Boundary | Trust | Controls |
| --- | --- | --- |
| Webview ↔ Rust core | Semi-trusted | Tauri v2 capabilities per window, strict CSP, typed IPC, no `eval`, no remote content loaded into the app window. Trailer playback happens in a separate sandboxed child webview limited to `youtube-nocookie.com`. |
| Core ↔ metadata providers | Untrusted input | Every response deserialized into a validating model; oversized payload caps; no HTML rendering of provider strings; image bytes sniffed and decoded only through the image crate with dimension limits. |
| Core ↔ swarm | Hostile | Torrent inspection gate, extension allowlist, path traversal rejection, size and file-count caps, never execute or open with system handler, never symlink. |
| Core ↔ local HTTP server | Local processes | Loopback bind, ephemeral port, per-session bearer token in path, no directory listing, no upload routes. |
| Core ↔ LAN (casting) | LAN peers | Off by default, explicit opt-in, per-session token, single-file scoped routes, optional PIN, bind restricted to the selected interface. |
| Core ↔ paired LAN devices | Paired LAN peers | Off by default; one explicit address, never the unspecified one; eight-digit one-use pairing approved on the desktop; SHA-256 token in an `HttpOnly`, `SameSite=Strict` cookie; `read,stream` scopes checked per route, with `acquire` only when the approval granted it; a `read` device may write the account's viewer state (progress, watchlist, watched marks) but never delete, change settings, or read a path, and sidecar subtitles are converted to WebVTT at the edge ([adr/0031](./adr/0031-browser-client-parity.md)); `Host`/`Origin` validation; 4 KiB body cap; `no-store` API and CSP; revocation checked on every request; HLS sessions capped and killed on stop, revoke, or expiry ([adr/0028](./adr/0028-tv-web-client.md)). |
| Core ↔ filesystem | User data | `PathGuard` canonicalization, roots allowlist, symlink escape rejection, unlink only for files recorded in the DB. |
| Core ↔ OS keyring | Trusted | API keys and Trakt tokens only ever in the platform keyring; redacted in logs and diagnostics. |

---

## 4. Container and runtime view

Peregrine is one process tree, not a client/server system. There is no daemon, no background service, and no port that stays open unless casting, streaming, or the optional LAN surface is enabled. That surface is the one deliberate exception: it is off by default, bound to one explicit address, and stops with the app ([adr/0028](./adr/0028-tv-web-client.md)).

```mermaid
graph LR
    subgraph Proc["Peregrine process"]
        direction TB
        TauriMain["Tauri main process<br/>Rust, tokio runtime"]
        Webview["Webview renderer<br/>WKWebView / WebView2 / WebKitGTK<br/>React 19 UI"]
        TorrentTasks["Torrent engine tasks<br/>librqbit session"]
        StreamSrv["Embedded HTTP server<br/>axum, loopback + optional LAN"]
        DB[("SQLite (WAL)<br/>1 writer, N readers")]
        ImgCache[("Image cache<br/>content-addressed LRU")]
    end

    subgraph Children["Child processes"]
        Mpv["mpv<br/>video surface + decode"]
        FFmpeg["ffmpeg / ffprobe<br/>probe, remux, subs, cast"]
    end

    Webview <-->|"IPC commands + events"| TauriMain
    TauriMain --> TorrentTasks
    TorrentTasks --> StreamSrv
    TauriMain --> DB
    TauriMain --> ImgCache
    TauriMain -->|"JSON IPC over stdin/stdout"| Mpv
    TauriMain -->|"spawn, progress on stdout"| FFmpeg
    Mpv -->|"HTTP Range"| StreamSrv
    Webview -->|"asset protocol: posters, thumbs"| ImgCache
```

| Runtime element | Technology | Why this shape |
| --- | --- | --- |
| Shell | Tauri v2 | ~10–15 MB installers, native webview per OS, real permission model, Rust backend. |
| UI | React 19 + TypeScript + Vite, static bundle | Fast iteration on a visually rich, animation-heavy UI without rebuilding a native toolkit's primitives. |
| Media playback | **mpv as a child process**, driven by JSON IPC, with a per-platform video surface | Crash isolation (a decoder fault kills the video surface, not the app), no copyleft in our process (a separate executable is an aggregate), and mpv's own huge format/codec coverage. Surface specifics in [adr/0013](./adr/0013-mpv-distribution-and-video-surface.md); `SPIKE-1`, `SPIKE-2` |
| Torrent engine | librqbit in-process | Pure Rust, memory-safe, per-file selection, sequential streaming, magnet + DHT. In-process avoids a second IPC hop and lets piece priorities be driven directly. `SPIKE-3` |
| Probe / remux / subtitles / cast transcode | ffmpeg child process | Never linked, never in the app's address space; licence boundary is a `NOTICE` file plus a source offer. |
| Persistence | SQLite via `sqlx` in WAL mode | Single-file, transactional, FTS5 for search, no server. |
| Secrets | OS keyring via `keyring` | Keychain / Credential Manager / Secret Service. Never a plaintext config file. |

### Process lifetime

```mermaid
stateDiagram-v2
    [*] --> Booting: process start
    Booting --> Setup: first run (no config DB)
    Booting --> Ready: config present
    Setup --> Ready: wizard complete + permissions granted
    Ready --> Playing: play file or stream
    Playing --> Ready: stop
    Ready --> Acquiring: torrent added
    Acquiring --> Ready: all jobs terminal
    Ready --> Casting: device session started
    Casting --> Ready: session stopped
    Ready --> Quitting: quit (graceful)
    Quitting --> [*]: journal checkpoint + mpv/ffmpeg reaped + engine shutdown
```

Quit is always graceful. Close-to-tray and "keep seeding" are opt-in settings; with seeding enabled, quit shows the active-seed count and offers *Stop and quit* / *Keep seeding in background* (the latter keeps the process alive with a tray icon).

---

## 5. Module map

Cargo workspace at the root, pnpm workspace for the frontend. Dependency direction is one-way and mechanically enforced.

```mermaid
graph TD
    subgraph Frontend["packages/"]
        WebApp["apps/desktop/src<br/>React app"]
        UIPkg["packages/ui<br/>design system"]
        ThemePkg["packages/theme<br/>tokens + themes"]
        ClientPkg["packages/ipc-client<br/>generated bindings"]
        PlayerWeb["packages/player-ui<br/>overlay + controls"]
    end

    subgraph Backend["crates/"]
        IPC["peregrine-ipc<br/>commands, events, DTOs"]
        App["peregrine-app<br/>services, orchestration"]
        Domain["peregrine-domain<br/>pure types + rules"]
        Storage["peregrine-storage<br/>SQLite, migrations, FTS"]
        Providers["peregrine-providers<br/>TMDB, TVDB, OMDb, IMDb, subs"]
        Sync["peregrine-sync<br/>scheduler, cache, images"]
        Torrent["peregrine-torrent<br/>engine + inspection"]
        Indexers["peregrine-indexers<br/>Prowlarr, Jackett, built-ins"]
        Monitor["peregrine-monitor<br/>watches + availability"]
        Mcp["peregrine-mcp<br/>stdio bridge, tool schemas"]
        Playback["peregrine-playback<br/>mpv, probe, sessions"]
        Cast["peregrine-cast<br/>mDNS, SSDP, castv2, DLNA"]
        Stream["peregrine-stream<br/>loopback + LAN HTTP"]
        Secrets["peregrine-secrets<br/>keyring, redaction"]
        Platform["peregrine-platform<br/>paths, permissions, tray, badges"]
        Xtask["xtask<br/>checks, codegen, release"]
    end

    WebApp --> UIPkg
    WebApp --> ThemePkg
    WebApp --> ClientPkg
    WebApp --> PlayerWeb
    IPC --> App
    App --> Domain
    App --> Storage
    App --> Providers
    App --> Sync
    App --> Torrent
    App --> Indexers
    App --> Monitor
    App --> Playback
    App --> Cast
    App --> Stream
    App --> Secrets
    App --> Platform
    Providers --> Domain
    Sync --> Domain
    Torrent --> Domain
    Indexers --> Domain
    Monitor --> Domain
    Mcp -.->|"local socket, post-1.0"| App
    Playback --> Domain
    Cast --> Domain
    Storage --> Domain
    Stream --> Domain
    Xtask -.->|"generates"| ClientPkg
```

| Crate | Responsibility | May depend on | Hard rules |
| --- | --- | --- | --- |
| `peregrine-domain` | Entities, value objects, invariants, quality-profile scoring, release-name and library-name parsing, subtitle-timing math, state machines, domain events. | `serde`, `jiff`, `ulid`, `thiserror` only | No IO, no async, no clock, no randomness. All time and IDs are injected. |
| `peregrine-storage` | Migrations, **account-scoped** repositories (ports from `app`), FTS5 search, KV settings, job journal, cache tables. | `domain`, `sqlx` | Repositories never contain business rules. Every catalogue and library read takes an `AccountScope`. All writes go through a single writer connection. |
| `peregrine-providers` | `MetadataProvider`, `SubtitleProvider`, and `MarkerProvider` adapters; response normalization; matching/scoring; rate limiting; conditional requests. The marker port is separate from metadata because its question is a playback subject and a runtime, not a catalogue row. | `domain`, `reqwest`, `governor` | No provider type escapes the adapter module. Every response passes through a size-capped, validating decode. A marker adapter returns domain markers and a closed error; it never returns a provider shape. |
| `peregrine-sync` | Job scheduler, priority queues, resource token buckets, incremental change sync, IMDb dataset ingestion, image cache manager, blurhash generation. | `domain`, `storage`, `providers`, `image` | Cooperative cancellation on every job; no job may hold a write transaction across a network call. |
| `peregrine-torrent` | `TorrentEngine` port + librqbit adapter; torrent/magnet ingestion; the safety inspection pipeline; piece-priority policy for streaming; per-file selection; seeding policy. | `domain`, `librqbit` | Never touches the filesystem outside the configured download root. Inspection runs before any byte is written. |
| `peregrine-usenet` | `UsenetEngine` port + NNTP adapter; bounded NZB/yEnc parsing; article assembly; per-provider failover; pause/resume/statistics. | `domain`, `quick-xml`, `tokio-rustls`, `crc32fast` | Implicit TLS except loopback; a message-id with a line break is refused; no byte is written outside the guarded job directory. |
| `peregrine-indexers` | `Indexer` port + Prowlarr, Jackett, local `.torrent`/magnet, and built-in adapters (disabled by default, no bundled domain list); result normalization and info-hash de-duplication; release-name and quality scoring feeds. | `domain`, `providers` | Never polls a rate-limited endpoint on a schedule. No bundled default domains. Adapters ship disabled. |
| `peregrine-monitor` | `Monitor` port plus the watching policy: per-scope monitors, the availability state machine, evidence-based transitions, dedupe keys, per-source cadence, and the daily digest. Enqueues jobs rather than scheduling its own polls. Post-1.0 ([adr/0016](./adr/0016-monitoring-and-availability.md)). | `domain`, `sync`, `providers` | A state transition without recorded evidence is a bug. Never polls a source faster than it can change. |
| `peregrine-mcp` | The stdio MCP server that agent clients launch: JSON-RPC framing, protocol-version negotiation, tool schemas, and a bridge to the running app over a per-user local socket. Contains no business logic and no database access. Post-1.0 ([adr/0017](./adr/0017-mcp-server.md)). | `domain` | Never opens the database. Every call goes through the app's services so scopes, approvals, and audit cannot be bypassed. |
| `peregrine-playback` | `PlayerBackend` port + mpv adapter; drives the session state machine that lives in `peregrine-domain::session`; probe cache; resume policy; next-up; watch-progress reporting; ffmpeg sidecar orchestration. **Landed so far: `PlaybackSession`** — one file, its loopback listener, and the two URLs a renderer needs (ADR [0023](./adr/0023-in-app-video-surface.md)); the mpv adapter and everything else in this row is still owed. | `domain`, `sync`, `stream` | No decode in-process. Position writes are throttled and coalesced. |
| `peregrine-stream` | axum router serving byte-range media, subtitle sidecars, and cast streams; token auth; per-session lifetime; bandwidth accounting. **Landed so far: the loopback half** — one file per session, byte ranges, a token in the request path, `Host` validation, and the route a captured poster frame is posted to. **Still owed: the LAN listener, cast streams, subtitle sidecars, and bandwidth accounting.** | `domain`, `torrent`, `axum` | Loopback and LAN listeners are separate bind addresses with separate token scopes. The cast LAN listener only exists while casting; the TV web surface's listener is separate, in `peregrine-app`'s `web` module. |
| `peregrine-cast` | Discovery (`mdns-sd` for Chromecast, `rupnp` for DLNA), renderer capability model, Chromecast CASTV2 and DLNA AVTransport adapters, delivery-mode decisions, playback-state reconciliation. Reuses `playback`'s sidecar supervisor for remux and transcode. | `domain`, `stream`, `playback` | Off by default. Never scans the whole subnet beyond the standard discovery protocols. |
| `peregrine-secrets` | Keyring read/write, in-memory `Secret` wrappers, log redaction filter. | `domain`, `keyring`, `secrecy` | Secret values are never `Debug`-printed, never serialized to the DB, never included in error strings. |
| `peregrine-platform` | Path resolution per OS, the production `Clock` and `IdGenerator`, permission requests, tray, dock/taskbar badges, autostart, notifications, power-save inhibition during playback, and the `OsAuthenticator` adapters (LocalAuthentication, Windows Hello, polkit and fprintd). | `domain` | One `cfg`-gated module per OS behind a common interface. It is the only shipping crate allowed to read a wall clock or a random source; `xtask check-determinism` enforces that. Authentication adapters report which mechanism answered, and report unavailability rather than faking success. The pieces that need a Tauri `AppHandle` are the exception and live where that handle is: the tray (PG-E1-09), the login item (PG-E1-13), the native pickers (PG-E1-12), and native notification delivery with its startup click handler (PG-E1-14) are plugins the desktop shell registers, and this crate holds the OS work that needs no running application. |
| `peregrine-app` | The runtime that composes the above — logging, the database, the scheduler, and the event bus — plus the services that use them: `LibraryService`, `DiscoveryService`, `AcquisitionService`, `PlaybackService`, `SubtitleService`, `SettingsService`, `CastService`, `NotificationService`, `AccountService`, `MaintenanceService`, and the LAN web surface (`web`: pairing, device grants, read routes, and HLS sessions). | everything above | Services are the only place where multiple ports are combined, and `Runtime` is the only place they are assembled. |
| `peregrine-ipc` | Tauri commands, event emitters, DTO mapping, error mapping, `specta` type export, and the one place the runtime is served over Tauri. | `app` | Commands are ≤20 lines: parse, call one service, map result. No logic. |
| `xtask` | Dependency-edge check, migration lint, IPC binding generation, release notes, bench harness, performance budget gate. | dev tooling | Runs in CI, never shipped. |

### Enforced rules

- `xtask check-arch` parses every `Cargo.toml` and fails the build on a disallowed edge (for example `storage → app`, or `domain → tokio`). The allowlist is a TOML file checked into the repo, so the architecture is reviewable in a diff.
- `xtask check-migrations` fails if a migration is edited after being merged, or if a new migration is not listed in the journal.
- `xtask gen-ipc` regenerates TypeScript bindings from Rust types; CI fails if the regenerated file differs from the committed one.

---

## 6. Frontend architecture

```mermaid
graph TB
    subgraph Shell["App shell"]
        Chrome["Window chrome<br/>platform titlebar, menus, shortcuts"]
        Route["TanStack Router<br/>typed routes + loaders"]
        Query["TanStack Query<br/>server-state cache"]
        UIState["Zustand<br/>ephemeral UI state"]
        Cmd["Command palette<br/>cmdk registry"]
    end
    subgraph Screens["Feature screens"]
        Home["Discover<br/>rails, hero, filters"]
        Grid["Browse / Filter<br/>virtualized grid"]
        Detail["Title detail<br/>seasons, episodes, cast"]
        Library["Library<br/>local files, unmatched"]
        Queue["Downloads<br/>queue + history"]
        Player["Player<br/>mpv surface + overlay"]
        CastScreen["Cast<br/>device picker"]
        Settings["Settings<br/>general, playback, torrent, subs, appearance, advanced"]
        Diag["Diagnostics<br/>providers, disk, logs"]
    end
    Chrome --> Route --> Screens
    Screens --> Query
    Screens --> UIState
    Query -->|"typed IPC"| Core["Rust core"]
    UIState --> Cmd
```

`apps/web` is a second client of the same core, not a second UI of the desktop app: a Vite/React SPA served as static assets by the LAN surface, talking the HTTP API rather than Tauri IPC. It has no TanStack Router or Query, no Tauri-bound hooks, and its own spatial-focus module; it shares `packages/theme`'s tokens, and its copy is local until it is consolidated with the console's catalogue. Its screens are the rotating hero-and-rails home, Browse with search and kind/sort/genre filters, My List, Downloads, the library grid, the title page with seasons, episodes, and More Like This, and the player ([adr/0028](./adr/0028-tv-web-client.md)). Each is a URL route, so refresh, Back, and a phone's edge swipe work without a client-side-only screen state. Rails, episode lists, and the More Like This feed are vertical grids on every viewport, the top navigation and a Back control sit on every screen but the player, a title opens at the top, and a pointer-fine device gets the desktop tile's hover preview. The title page writes the account's viewer state — the watchlist, watched marks, and a season or episode download — through the API, and the player converts the library's sidecar subtitles to WebVTT, defaults to the preferred language, and autoplays the next playable episode ([adr/0031](./adr/0031-browser-client-parity.md)).

### State ownership

| State kind | Owner | Never |
| --- | --- | --- |
| Library, metadata, downloads, progress, settings | Rust core + SQLite | Duplicated as a mutable source of truth in the UI |
| Server-state cache (queries, optimistic overlays) | TanStack Query | Persisted to `localStorage` |
| Ephemeral UI state (open menus, hover, selection, scroll anchors) | Zustand slices | Sent over IPC |
| Window geometry, sidebar width, rail collapse | `tauri-plugin-window-state` + settings | Reimplemented |

Rules that keep it fast:

1. **One query per entity, keyed by domain ID.** No derived caches; selectors compute.
2. **Rails render from a single paginated query per rail**, hydrated from cache first (`staleTime` long, `refetchOnMount: false`), then revalidated in the background.
3. **Virtualize everything longer than ~60 items** with TanStack Virtual, fixed tile aspect ratios so the scrollbar is stable and there is no layout shift.
4. **Images:** `<img>` with `src` pointing at the asset protocol, `loading="lazy"`, `decoding="async"`, explicit `width`/`height`, `fetchpriority="high"` only for the hero, and a blurhash CSS background that is replaced on `load` (cross-fade 160 ms). No spinners for images.
5. **Animations touch only `transform` and `opacity`.** `will-change` is set on interaction start and removed on settle. Layout-affecting animation is a review reject.
6. **No page-level route transition that unmounts the shell.** The chrome, sidebar, and player surface persist across navigation.
7. **Progress events are coalesced in Rust to ≤4 Hz per job**, and the UI animates between samples rather than re-rendering per event.

### Routing map

| Route | Screen | Notes |
| --- | --- | --- |
| `/` | Discover | Hero + rails: top 10 today, on the air, upcoming, top rated, continue watching, because-you-watched. |
| `/browse/:type?` | Browse | Full filter surface + saved smart collections. |
| `/search` | Search | Local FTS5 instant results, remote fallback after 250 ms of idle. |
| `/title/:kind/:id` | Detail | Movie or show; seasons, episodes, cast, availability, actions. |
| `/person/:id` | Person | One credited person, with the titles in this library that cast them and the rest of their credits from the provider. |
| `/library` | Library | All local media, unmatched files, storage usage, re-scan. |
| `/downloads` | Queue | Active, queued, seeding, completed, failed; global limits. |
| `/player/:id?season=&episode=` | Player | Own window/fullscreen; mpv child surface + HTML overlay. The id names the *title*; an optional season/episode pair names the episode to open, and both travel together or not at all. `PG-E16-02`'s session registry is what makes a session addressable in its own right (ADR [0023](./adr/0023-in-app-video-surface.md)) |
| `/cast` | Cast | Devices, session control, capability report. |
| `/settings/:section?` | Settings | `Cmd/Ctrl+,`; sections mirror the settings registry. |
| `/diagnostics` | Diagnostics | Provider health, disk budget, log tail, endpoint list. |

The profile selector is a full-window modal rather than a route: it is reachable from every screen, and choosing a profile returns to the screen underneath. The profiles are `account` rows ([ADR 0032](./adr/0032-shared-profiles.md)): the desktop and every paired browser read the same list, the active choice is one installation-wide setting, and the runtime resolves every scoped read through it. The library is shared; watch progress, the watchlist, history, and notifications are keyed by the active account.

### Feature folder convention

```
apps/desktop/src/
  app/            shell, router, providers, error boundaries
  features/
    discover/     components, hooks, queries, types (colocated)
    title/
    library/
    downloads/
    player/
    settings/
  lib/            ipc client wiring, formatters, keyboard, theme runtime
```

A feature never imports another feature's internals. Cross-feature reuse is promoted into `packages/ui` or `lib/`, which keeps the dependency graph reviewable.

---

## 7. IPC contract

Rust types are the single source of truth. `specta` + `tauri-specta` generate `packages/ipc-client/src/generated/bindings.ts`; the UI never hand-writes a DTO.

```mermaid
graph LR
    TS["TS call site"] -->|"invoke cmd"| Bridge["tauri-specta bridge"]
    Bridge --> Cmd["#[tauri::command] handler"]
    Cmd --> Svc["app service"]
    Svc --> Ports["ports + adapters"]
    Ports -->|"domain events"| Bus["event bus"]
    Bus -->|"batched, coalesced"| Emit["window.emit"]
    Emit --> TS
```

### Command surface (initial)

| Namespace | Commands | Shape |
| --- | --- | --- |
| `discover` | `get_home_rails`, `browse`, `search`, `get_recommendations` | Paged request → paged DTO |
| `title` | `get_title`, `get_season`, `get_episode`, `get_cast`, `refresh_metadata` | ID → DTO |
| `library` | `scan_library`, `list_library`, `get_unmatched`, `match_file`, `delete_media`, `reveal_in_file_manager`, `get_storage_usage` | ID lists → job ID + progress events |
| `acquire` | `search_releases`, `inspect_torrent`, `start_download`, `pause`, `resume`, `remove_download`, `set_file_selection`, `list_jobs` | Job-scoped |
| `playback` | `start_session`, `stop_session`, `seek`, `set_track`, `set_speed`, `report_position`, `get_resume_point`, `play_next` | Session-scoped |
| `subtitles` | `search_subtitles`, `download_subtitle`, `set_subtitle_offset`, `extract_embedded`, `get_language_prefs` | Title/episode-scoped |
| `cast` | `discover_devices`, `start_cast`, `stop_cast`, `get_cast_capabilities` | Device-scoped |
| `watchlist` | `add`, `remove`, `list`, `reorder` | Optimistic |
| `integrations` | `list_integrations`, `set_integration_enabled`, `set_integration_priority`, `set_integration_submission`, `test_integration`, `set_integration_credentials`, `clear_integration_credentials` | Manifest-driven; the three credential actions are jobs, and no command reads a key back |
| `markers` | `markers_for_playback`, `refresh_playback_markers` | Cache read, then a background refresh; no provider call from a renderer |
| `settings` | `get_schema`, `get_values`, `set_value`, `reset_section`, `export_config`, `import_config` | Schema-driven; the UI renders from the registry |
| `jobs` | `start_hello`, `cancel`, `list` | Job-scoped: a start returns an ID and streams `job://progress` and `job://state`; `cancel` records the request before it trips the token; `list` is a capped page of the journal |
| `backup` | `create_backup`, `restore_backup` | One file per call. A restore validates and stages the file, then restarts the app; the swap happens before the next launch opens the database ([ADR 0033](./adr/0033-account-backup-and-restore.md)) |
| `system` | `get_permissions`, `request_permission`, `open_path`, `get_disk_budget`, `health`, `log_tail`, `export_diagnostics` | `health` reports the scheduler's and the event bus's counters, `log_tail` the newest lines of the redacted log |

### Rules

1. **Payload small.** Commands return IDs and DTOs, never blobs. Images and media bytes travel over the asset protocol or the loopback HTTP server. Any response over 64 KB is a design error.
2. **Long work returns a job ID** and streams `job://progress` events. No command blocks longer than 100 ms; anything slower is a job.
3. **Errors are structured**: `{ code, message, detail?, retryable, cause? }` where `code` is a stable enum shared with the UI. `detail` never contains secrets or absolute home paths.
4. **Events are versioned and namespaced**: `job://progress`, `job://state`, `download://stats`, `library://changed`, `player://state`, `player://markers`, `cast://state`, `sync://health`, `notification://raised`. `player://markers` carries a count, never the markers: the player re-reads through `markers_for_playback`, so an event cannot become a second, unvalidated cache.
5. **High-frequency streams are coalesced** in a `tokio::time::interval` batcher with a max rate per stream and a dropped-update counter so the UI can detect starvation.
6. **Cancellation is explicit**: every job-scoped command has a `cancel` counterpart; dropping the last subscriber does not silently cancel user-visible work.

---

## 8. Data model

Full schema, indices, and invariants: [DATA_MODEL.md](./DATA_MODEL.md). Summary of the shape:

```mermaid
erDiagram
    TITLE ||--o{ EXTERNAL_ID : has
    TITLE ||--o{ SEASON : contains
    SEASON ||--o{ EPISODE : contains
    TITLE ||--o{ CREDIT : credits
    PERSON ||--o{ CREDIT : in
    TITLE ||--o{ GENRE_LINK : tagged
    GENRE ||--o{ GENRE_LINK : used_by
    TITLE ||--o{ MEDIA_ITEM : "realized by"
    EPISODE ||--o{ MEDIA_ITEM : "realized by"
    MEDIA_ITEM ||--o{ MEDIA_FILE : "one or more files"
    MEDIA_FILE ||--o{ SUBTITLE_TRACK : has
    TITLE ||--o{ WATCH_PROGRESS : tracked
    EPISODE ||--o{ WATCH_PROGRESS : tracked
    TITLE ||--o{ WATCHLIST_ENTRY : saved
    TITLE ||--o{ DOWNLOAD_JOB : requested
    TITLE ||--o{ DOWNLOAD_SCHEDULE : scheduled
    EPISODE ||--o{ DOWNLOAD_SCHEDULE : scheduled
    DOWNLOAD_JOB ||--o{ DOWNLOAD_FILE : selects
    MEDIA_ITEM ||--o{ DOWNLOAD_JOB : sourced
    TITLE ||--o{ SMART_COLLECTION_MEMBER : grouped
```

Key modelling decisions:

- **`title` is one table for movies and shows** (`kind` discriminator) so discovery, watchlist, and progress code paths are uniform; season/episode detail hangs off it.
- **`external_id` is a separate table** keyed by `(provider, value)` so a title can carry TMDB, IMDb, TVDB, and AniDB IDs simultaneously and resolution is a join, not a column guess.
- **`media_item` is the library's notion of "we have this"**, distinct from the metadata title. One episode can have several `media_file` rows (a 1080p and a 4K copy); the player picks by preference and the library counts it once.
- **Everything cached from a provider is `provider_cache` or `image_ref`.** JSON resources carry TTL plus `ETag`/`Last-Modified` for revalidation; artwork carries an immutable path and is evicted by LRU size alone. Either way the cache is droppable at any time without data loss.
- **Watch progress is per `(account, title|episode)`**, so multi-account support (phase 2) is a schema-compatible addition rather than a migration of user data. A player session carries the episode id it opened, which is what lets the progress row and the episode bar agree.
- **`download_schedule` is a request, not an attempt.** It stores the title or episode and the first date it may be fetched; the runtime's ticker converts a due row into the same acquisition the Download button starts. Cancelling deletes the request; a job it already started is cancelled from the queue.
- **`job` is a durable journal**, so the queue screen can show history and a crash mid-download leaves a record rather than nothing. **No job outlives the process that was running it**, and the journal cannot tell the rows of a process that died from the rows of one still working — so every non-terminal row is closed as `INTERRUPTED` when the database is opened (`peregrine_storage::jobs::fail_unfinished`). That is not bookkeeping: `job_dedupe_uq` covers the non-terminal rows only, so a row left `queued` holds its dedupe key against every later submission, and a caller that joins a live row — as the Discover fetch does — would join a job that can never run.

---

## 9. Core flows

Twelve end-to-end sequence diagrams — onboarding, discovery hydration, search, incremental sync, release resolution, acquisition with the safety gate, streaming playback, local scan/match, subtitles, watch progress and next-up, casting, and deletion — live in [FLOWS.md](./FLOWS.md), together with the download, availability, playback-session, and job state machines.

The three that constrain the most design decisions, summarised:

| Flow | Constraint it imposes |
| --- | --- |
| **Incremental sync** ([FLOWS F4](./FLOWS.md#f4--incremental-metadata-sync)) | The UI must never wait for the network. Sync writes into `provider_cache` and the job appends a `library://changed` invalidation; the UI refetches only affected keys. Polling `/changes` windows is cheap; full catalogue walks are not. |
| **Acquisition** ([FLOWS F6](./FLOWS.md#f6--acquisition-with-the-safety-gate)) | Inspection is a hard gate *before* the engine is handed the torrent, so a hostile torrent never produces a file on disk. The gate result is persisted with the job, not recomputed. |
| **Streaming playback** ([FLOWS F7](./FLOWS.md#f7--streaming-playback)) | The player is dependency-inverted around a byte source. Local file and torrent stream paths are the same `MediaSource` trait, so the session state machine, resume logic, and subtitle pipeline are written once. |

---

## 10. Concurrency and resource model

```mermaid
graph TB
    subgraph Tokio["tokio multi-thread runtime"]
        IPCQ["IPC commands<br/>short-lived tasks"]
        Jobs["Job scheduler<br/>priority queue, N workers"]
        Net["Provider IO<br/>reqwest pool"]
        Events["Event batcher"]
    end
    subgraph Blocking["spawn_blocking pool"]
        Hash["Hashing / hashing for subs"]
        Img["Image decode + resize + blurhash"]
        Scan["Library scan / ffprobe parse"]
        DBW["SQLite writer"]
    end
    subgraph Engine["Torrent runtime"]
        Session["librqbit session<br/>own task group"]
        Priorities["Piece priority driver"]
    end
    subgraph Procs["Child processes"]
        MpvP["mpv"]
        FFP["ffmpeg / ffprobe"]
    end
    Jobs --> Net
    Jobs --> Blocking
    Priorities --> Session
```

| Resource | Policy |
| --- | --- |
| DB writers | Exactly one write connection, serialized through a channel. Readers use a pool sized to `min(4, cores)`. `busy_timeout = 5s`, WAL, `synchronous = NORMAL`. |
| Network | Global concurrency cap (default 8) plus per-host token buckets (metadata 8 rps, images 12 rps, subtitle 1 rps). A 429 or `Retry-After` parks the bucket; it never retries into a wall. |
| CPU | `desired_cpu_budget` setting (default: leave one core free) drives hashing threads, image decode concurrency, torrent hashing threads, and ffmpeg `-threads`. |
| Disk | Free-space floor (default 5 GiB) blocks new downloads and pauses image-cache growth. Writes are `fsync`-batched by SQLite itself; torrent piece flushing uses librqbit's own policy. |
| Bandwidth | Global down/up caps, per-job caps, and a "pause all when playing" setting that reserves headroom for streaming. |
| Child processes | Reaped on drop via a guard; orphans killed at startup by PID+start-time match recorded in the journal (never `pkill` by name). |
| Cancellation | Every job carries a `CancellationToken`; services check it between steps; adapters accept it in their IO calls. |
| Backpressure | The event batcher drops intermediate progress frames and counts drops; the UI shows a "live" indicator degraded rather than stalling. |

Long tasks never run on the main thread, and no `await` is held across an SQLite write transaction. The two rules together are what keep the UI at 60/120 fps while downloading and seeding.

---

## 11. Caching architecture

Five tiers. Only tier 3 costs real disk, and every tier is disposable.

| Tier | Store | Contents | Eviction | Budget |
| --- | --- | --- | --- | --- |
| 1 | In-memory LRU (`moka`) | Hot title DTOs, search results, provider responses | Size-bounded (default 64 MB) | RAM |
| 2 | SQLite | Normalized entities, `provider_cache` with ETag + TTL, job journal, FTS5 index, resume points | TTL per resource class; rows pruned when unreferenced | DB ≤ 300 MB typical |
| 3 | Disk image cache | Poster/backdrop/still bytes, downloaded subtitles, ffprobe JSON results | LRU by bytes, hard cap, size-variant aware | Default 1 GB, user-set |
| 4 | Disk dataset | IMDb ratings only: 8.7 MB compressed, ~45 MB in SQLite | Replaced wholesale on refresh | 60 MB cap; alt titles are a separate opt-in with a 2.5 GB cap, disabled by default |
| 5 | HTTP conditional | `If-None-Match` / `If-Modified-Since` on every provider call | 304 refreshes TTL without transferring bytes | — |

Rules:

- **Tier 3 stores bytes keyed by content hash of `(provider, path, variant)`**, so renaming a provider path does not leak old files, and a cache wipe is always safe.
- **Treat every image path as immutable.** TMDB's CDN returns no usable `ETag`, ignores `If-None-Match`, and answers `If-Modified-Since` with `200` regardless, so there is nothing to revalidate and eviction is LRU by size. Conversely, a JSON cache row without its `ETag` is a bug: that revalidation would be a full transfer.
- **`provider_cache` rows record `fetched_at`, `expires_at`, `etag`, and `schema_version`.** A schema version bump invalidates the row rather than crashing the deserializer.
- **Prefetch is bounded and idle-driven**: the next rail's posters (max 12) prefetch on `requestIdleCallback`, never during scroll, never while a download saturates the link and "don't prefetch while streaming" is on.
- **Disk budget guardrail**: Settings → Diagnostics shows per-tier usage with one-click clear. Exceeding the cap evicts immediately; the DB is never evicted to reclaim space.

---

## 12. Security architecture

Full threat model, controls, and test plan: [../SECURITY.md](../SECURITY.md). Architectural commitments:

| Area | Commitment |
| --- | --- |
| Usenet ingestion | Parse the NZB under file/segment caps → inspect the entry list → store the NZB beside the payload → download into the job directory → list an archive before extraction → walk the extracted tree for links, escapes, and caps. The import is the enforced boundary (ADR 0030). |
| Torrent ingestion | Sniff metadata → validate every file path → extension allowlist → cap file count (default 500) and total size (the volume's free space above the floor) → reject symlinks, absolute paths, `..`, Windows reserved names, and unicode-spoofed executable extensions → produce a `SafetyReport`. Executables and scripts are blocked with no override; nested archives and disk-image files are blocked by default with an explicit advanced opt-in. |
| Content execution | Downloaded media is never opened with the OS default handler. Playback goes through mpv with a hardened flag set (`--no-config`, `--load-scripts=no`, `--no-ytdl`, `--sub-auto=no`, empty `--script-opts`). |
| Path handling | All filesystem operations go through `PathGuard::resolve`, which canonicalizes, rejects symlink escapes, requires containment within a configured root, and returns a typed `SafePath` that the fs layer accepts. There is no other way to obtain a writable path. |
| Secrets | OS keyring for all API keys and tokens; `Secret<T>` wrapper with no `Debug`/`Display`; a `tracing` layer that redacts known key shapes and any value registered as a secret. |
| Webview | Strict CSP with no `unsafe-eval`, no remote origin in the main window, `asset:` protocol scoped to cache and media roots, and a dedicated sandboxed child webview for YouTube trailers. Tauri capabilities are per window and minimal; the player window gets no filesystem commands. |
| Local HTTP | Loopback listener: ephemeral loopback port, a per-session token as the only variable path segment, `Host` validated against loopback **before** the token is read, routes scoped to one file, no listing, `no-store` and `nosniff` on every response, and a socket that dies with the session (ADR [0023](./adr/0023-in-app-video-surface.md), SECURITY.md T10). One write route — a poster frame the renderer captured — and one CORS policy, which is what lets a canvas read a frame it captured. LAN listener: exists only during casting, bound to the chosen interface, separate token, optional PIN, and rate-limited. |
| Deletion | Two-step confirmation that enumerates the exact file paths and byte counts, plus a checkbox for permanent delete (default: move to OS trash for library files; torrent payloads go to trash too when the trash is available). A typed phrase is required only for bulk deletion of more than 10 items or more than 20 GB. |
| Dependencies | `cargo-deny` (licences, ban list, advisories), `cargo-audit`, `osv-scanner`, `pnpm audit`, `knip` for dead deps, `cargo-machete` for unused crates, and a CycloneDX SBOM attached to every release. |
| Updates | Signed update manifests verified with a public key embedded in the binary; the updater refuses downgrades outside an explicit beta channel. Optional component packs (for example the full ffmpeg build) download only on an explicit user action, are verified against a pinned checksum and signature, and never run with elevated privileges. |
| Notifications | The user is asked before we notify them, and the answer is respected. Toast content is treated as public: media titles appear only while "show titles" is on, adult-flagged titles never appear, and no notification carries a filesystem path, secret, magnet URI, tracker URL, or infohash. Every event is also written to the in-app centre, so nothing depends on the OS delivering it. |
| Accounts and gating | Content scope is enforced in the query layer, not the UI, and fails closed: unrated content is hidden from a restricted account. A blocked title is indistinguishable from a nonexistent one. Capability checks live in the services, so no UI path, deep link, agent call, or media-server request can make a child account acquire media. Account switching is gated both ways by the OS credential where one exists ([adr/0019](./adr/0019-local-accounts-and-parental-gating.md)). |
| Agent access (MCP) | Off by default and loopback-only. Clients are paired, named, scoped, and revocable; `read` is the only default scope; anything that spends bandwidth, disk, or privacy requires an in-app approval dialog naming the requesting client; every call is audited. Externally sourced strings are returned to agents inside a labelled `untrusted` wrapper, and no tool output can trigger a tool call. |
| Permissions | First-run wizard requests folders, network, notifications, and (optionally) autostart, each with a plain-language explanation and a "not now" path. macOS entitlements are minimal and notarized; Windows signing documented; Linux provides AppImage, deb/rpm, and a Flatpak manifest with a documented portal-based file access story. |

---

## 13. Extensibility

Five extension points, all compile-time traits with a conformance test suite:

| Port | Trait | Add a new one by | Required tests |
| --- | --- | --- | --- |
| Metadata | `MetadataProvider` | Implementing capabilities (`search`, `get_title`, `get_season`, `get_episode`, `trending`, `changes`, `images`, `videos`), normalizing into domain types, declaring a rate limit and a cache policy, answering `check` — the one operation that belongs to no capability, because it asks whether a credential works before one is stored (PG-E5-20) — and the title-scoped extras (`credits`, `similar`, `watch_providers`) | Recorded fixture replay, field-normalization snapshots, rate-limit behaviour, malformed/oversized payload rejection |
| Subtitles | `SubtitleProvider` | `search(query) -> Vec<SubtitleCandidate>`, `download(candidate) -> Bytes`, declaring auth and quota | Fixture replay, encoding-detection cases, quota-exhaustion behaviour |
| Torrent engine | `TorrentEngine` | Session lifecycle, add/select/stream/stats/limits | Local seeder↔leecher integration test over loopback, cancellation, restart recovery |
| Player backend | `PlayerBackend` | Launch/stop, command channel, state stream, position stream, track enumeration | Fake backend conformance suite; real backend smoke test per OS |
| Skip markers | `MarkerProvider` | `fetch(MarkerRequest) -> Vec<PlaybackMarker>`, declaring whether a credential is required | Recorded fixture replay; 401/429/5xx/malformed/oversized handling; a title with no identity returns nothing, never a guess |
| Indexer | `Indexer` | `search(ReleaseQuery) -> Vec<Release>` | Fixture replay, result de-duplication and ranking property tests, disabled-by-default assertion |

The MCP interface ([adr/0017](./adr/0017-mcp-server.md)) is a **consumer** of these ports, not a sixth extension point. It exposes existing services to agents; it cannot add behaviour, load code, or reach past the service layer. Nothing in the extension model may depend on it.

A `conformance` test crate runs each provider's fixtures against shared invariants, so "does this adapter behave like the others" is a test, not a review opinion. Adding a provider should touch exactly three files: the adapter, its fixture directory, and the provider registry manifest.

**Integrations are the same rule at the settings layer.** `peregrine-app`'s `integrations::MANIFEST` is one entry per adapter that exists — category, capabilities, credential fields, submission support, default priority, and probe. The desktop card, the read-only TV status, the credential job, and the marker resolver all read it. An id not in it is refused before a job exists, and adding an integration is a manifest entry plus an adapter rather than a second screen (ADR [0035](./adr/0035-integrations-and-markers.md)).

---

## 14. Performance architecture

### Budgets (enforced, not aspirational)

| Metric | Target | Measured by |
| --- | --- | --- |
| Cold start → interactive | ≤ 1200 ms (Apple silicon), ≤ 2000 ms (Windows mid-tier) | Startup trace in CI on a fixed machine class + manual per-OS release check |
| Warm start → interactive | ≤ 400 ms | Same |
| Discover first paint from cache | ≤ 100 ms | E2E assertion via `performance.mark` |
| Browse grid scroll | ≥ 60 fps at 5 000 items, ≥ 110 fps on a 120 Hz display | Playwright trace + frame-time histogram |
| Search keystroke → results | ≤ 16 ms p50, ≤ 50 ms p95 (local FTS5) | Criterion bench on the query path + UI measurement |
| Memory, idle | ≤ 250 MB RSS total (excluding mpv/ffmpeg), webview heap ≤ 120 MB | `sysinfo` sampling in a soak test |
| IPC payload | ≤ 64 KB per response; progress events ≤ 4 Hz per job | Assertion in the IPC layer + contract tests |
| Image cache hit latency | ≤ 8 ms for a cached 342 px poster | Bench on the asset handler |
| Installer size | Base install (app, mpv, minimal ffprobe): macOS ≤ 85 MB, Windows ≤ 75 MB, Linux AppImage ≤ 130 MB. The full ffmpeg pack adds ~50 MB and is downloaded on demand, not shipped | Release CI budget gate |

### Rules that produce those numbers

1. Cache-first rendering; a screen is never blank and never shows a full-page spinner.
2. Virtualized grids and rails with fixed geometry; no measurement-driven relayout during scroll.
3. Composite-only animation; no layout or paint in the interaction path.
4. IPC carries IDs and deltas; blobs go over the asset protocol or loopback HTTP.
5. Image variants are requested at the size they are displayed (no 1280 px backdrop in a 300 px tile) and resized once into the disk cache.
6. All playback and probe work happens out of process; a stalled stream degrades the player, never the UI.
7. Every hot path has a bench that fails CI on regression beyond 15%.

---

## 15. Degradation and error model

| Condition | Behaviour |
| --- | --- |
| No network at start | Discover shows cached rails with an "offline, last updated 3 h ago" banner. Library, downloads in progress, playback, and stored subtitles work fully. |
| Provider 429 / 5xx | Token bucket parks; the job is rescheduled with exponential backoff and jitter; the UI shows a provider-health dot in Diagnostics, not an error modal. |
| Provider schema drift | Decode failure is isolated to the affected resource; the row is marked `stale` and the title renders from cache with a "metadata refresh failed" affordance. Never a crash, never a blank screen. |
| Torrent stalled | Piece-starvation detection surfaces "no peers" with actionable detail (DHT nodes, tracker status, port reachability), plus the option to try an alternative release. |
| Disk full | New downloads blocked, active ones paused (not failed) with a resume affordance, image cache trimmed automatically, and a clear banner with the required bytes. |
| mpv crash | Session state machine moves to `failed`, UI shows the exit code and stderr tail, offers retry from the last known position. The app itself never dies. |
| DB locked / corrupt | `PRAGMA integrity_check` at startup; on corruption, quarantine the damaged file and its write-ahead files beside the database — never deleted — recreate a clean schema, and signal that the library must be reimported by scanning (`Health.rescanRequired`). A lock another process holds is reported as a busy error rather than as damage. Provider caches are simply refetched. |
| Subtitle provider quota exhausted | Falls through the preference chain, then reports "no subtitles found — quota reached for OpenSubtitles, next reset 04:00". |
| Cast device disappears | Session ends gracefully, position is written back, and local playback can resume at that point. |
| Update check fails | Silent; retried on the next launch; never blocks startup. |

---

## 16. Observability

| Signal | Collection | Retention |
| --- | --- | --- |
| Structured logs | `tracing` JSON to rotating files in the app log dir, redacted | 7 days / 20 MB |
| Job history | `job` table with timings, attempt counts, and failure reasons | 90 days |
| Provider metrics | Per-provider request count, latency histogram, error rate, rate-limit hits, cache hit ratio (in-memory, aggregated, persisted hourly) | 30 days |
| Player telemetry | Startup latency, dropped frames (from mpv `--stats` JSON), decoder, hw-decode path, audio underruns | Current session + last 20 |
| Diagnostics screen | Provider health, disk budget per tier, endpoint list, top slow queries, log tail with copy/export | On demand |
| Crash reports | Opt-in, scrubbed, self-hostable Sentry-compatible DSN (empty by default) | Per deployment |

Nothing leaves the machine unless the user opts in, and the export bundle (logs + diagnostics + config-minus-secrets) is inspectable before sending.

---

## 17. Cross-platform matrix

| Capability | macOS | Windows | Linux X11 | Linux Wayland | Notes |
| --- | --- | --- | --- | --- | --- |
| Window chrome | Full native controls; overlay titlebar with traffic-light inset | Native frame, native caption buttons, Snap Layouts intact; the app bar is a toolbar beneath them | Native decorations; a header bar under them, sized like GNOME/KDE's | Same | Per-OS chrome components behind one `WindowChrome` interface, and the platform is read from the core (`BuildInfo.target`) rather than the user agent. Tauri v2 applies `titleBarStyle`/`hiddenTitle` under `cfg(target_os = "macos")` only, so the overlay title bar is macOS's alone and no platform gets a web-drawn caption button: an invented one loses Snap Layouts on Windows and the desktop's own controls on Linux (docs/DESIGN_SYSTEM.md §8). The window is created `visible: false` and `tauri-plugin-window-state` shows it once the restored geometry is in place (PG-E1-02) |
| mpv video surface | App-positioned borderless child window (`--wid` is unimplemented on Cocoa) | `--wid` with the child window's `HWND` | `--wid` on the X11 window | Degraded hand-off: mpv in its own toplevel, webview video region hidden | Windows and X11 are siblings of the webview child window, so resize, move, and z-order are synchronized explicitly ([adr/0013](./adr/0013-mpv-distribution-and-video-surface.md)) |
| Hardware decode | VideoToolbox | D3D11VA / NVDEC | VA-API | VA-API | Detected at probe time, surfaced in Diagnostics, overridable |
| Tray / menu bar extra | Menu bar item | System tray | AppIndicator | AppIndicator | Optional; required if "keep seeding after close" is enabled. A state-aware menu whose two counts — jobs in flight and torrents seeding — are read from the core's scheduler, never assembled from a webview event, and rebuilt only when a value changed, on the main thread, because `muda` will not create a platform menu item anywhere else (PG-E1-09). `seeding` stays 0 until a torrent engine is composed into the runtime (PG-E10) |
| Dock/taskbar progress | `NSDockTile` badge | Taskbar overlay icon | Unity launcher API when present | — | Download progress mirroring |
| Notifications | `notify-rust` via UNUserNotificationCenter; system sounds or a bundled file by name | Windows toast; **requires an installed app** (dev builds show the PowerShell identity), `.wav` path for sound, clicks only reported while the toast is on screen | libnotify over D-Bus; XDG theme sound name as the `sound-name` hint | Same as X11 | Desktop has **no permission prompt at the plugin level** — `request_permission` returns Granted unconditionally, so consent is asked in-app and verification is a test notification. Grouping and scheduling are ignored on desktop, so coalescing is ours. Nothing can be retracted on macOS or Windows. `tauri-plugin-notification` is registered by the shell, which also registers the click handler in `setup` before any toast can exist — the plugin reports no click at all while no handler is registered — and carries the centre row's deep link in the notification's `extra` payload, so a click takes the same `external::forward_urls` route a `peregrine://` launch does. `show()` is fire-and-forget and discards the OS result, which is why the in-app centre, not the toast, is the record (AGENTS R31, PG-E1-14). The plugin's int32 id comes from a counter persisted in `notification-id` in the application data directory, issued `1..=i32::MAX` and wrapped back to 1; nothing routes by it. Consent is core state (`peregrine-domain::notify::Consent`): an unanswered ask stays askable so a closed wizard is resumable, a user who answered is never asked again unprompted, and an OS permission the user later revokes stops OS delivery without stopping the centre (PG-E1-20) |
| Autostart | Launch agent (SMAppService) | Registry Run key | XDG autostart | XDG autostart | Off by default, and off until the user asks: `tauri-plugin-autostart` owns the per-OS mechanism, registering it creates no login item, and the settings toggle is the only caller of `enable` (PG-E1-13) |
| Deep links | `peregrine://` via Launch Services | Protocol registration | `.desktop` handler | Same | `peregrine://title/tmdb/123`, `peregrine://magnet/<40 hex btih>`, and `peregrine://magnet?xt=urn:btih:<40 hex btih>`; every other route and shape is refused. `tauri-plugin-deep-link` registers the scheme and is the path a macOS Launch Services open takes; `tauri-plugin-single-instance`'s `deep-link` feature is deliberately **not** enabled, because it would hand the same argument vector to the plugin before the single-instance callback and route one link twice. The grammar is `peregrine-domain::deeplink` and a refused link is logged without its text — a magnet URI is untrusted input and the diagnostics tail is exported (SECURITY.md T12, T13). A magnet is classified and refused until the confirmation gate can show an inspection report (PG-E1-11, PG-E10) |
| Trash-based delete | Native trash | Recycle Bin | XDG trash | XDG trash | Fallback to permanent delete with explicit confirm |
| Casting | Chromecast + DLNA | Chromecast + DLNA | Chromecast + DLNA | Chromecast + DLNA | AirPlay out of scope (see [adr/0010](./adr/0010-casting-scope.md)) |
| Account gate (Phase 8) | `LAContext`: Touch ID, Apple Watch, or device password | Windows Hello via `UserConsentVerifier` | polkit agent, then fprintd over D-Bus, then app PIN | Same as X11 | App PIN is the documented fallback and the UI says it is weaker; with a shared OS login this is a usability boundary, not a security boundary |
| Media server (Phase 7) | LAN listener over Wi-Fi or Ethernet, interfaces selectable; headless service via launchd | Same, via a Windows service | Same, via a systemd unit | Same; Wi-Fi is the normal case and mDNS is the dependable discovery path | No WAN exposure by default. Transcode capacity is limited by the LGPL ffmpeg build ([adr/0018](./adr/0018-media-server.md)) |
| Homelab deployment (Phase 9) | Not a target: the daemon runs on the server, the desktop app connects to it as a client | Not a target, for the same reason | The primary target: the headless daemon as a container image or a NixOS module, with no display server, webview, or desktop session | Same as Linux X11 | Four patterns, not nine products: OCI with Compose (Debian, Ubuntu Server, OpenMediaVault, TrueNAS SCALE, Unraid, umbrelOS, CasaOS), Podman under systemd (Fedora), a guest on a hypervisor (Proxmox VE, XCP-ng), and Nix. One glibc image for `amd64` and `arm64`, non-root with `PUID`/`PGID`, one state directory, secrets from an explicitly chosen backend, and a browser console still to come ([adr/0020](./adr/0020-homelab-deployment.md)) |
| Agent access (MCP) | stdio sidecar; optional loopback Streamable HTTP | Same, launching a `.exe` sidecar | Same | Same | Identical on every OS because it is a local socket or loopback HTTP. Post-1.0, [adr/0017](./adr/0017-mcp-server.md) |
| System accent color | `NSColor.controlAccentColor` | Accent palette API | GNOME accent / KDE color scheme | Same | Feeds the theme's accent token when "follow system" is on |
| Application menu | An application menu placed by the OS, plus Edit, View, Playback, Window, and Help | Edit, Playback, View, Window, and Help; the native title bar keeps the system menu | The same five submenus; GNOME and KDE also export them over the desktop bus under their own conventions | Same | The Edit items are the platform's own `PredefinedMenuItem`s, which is what makes cut/copy/paste/select-all work in a text input, and their accelerators are `muda`'s `CMD_OR_CTRL` — Cmd on macOS, Ctrl elsewhere — rather than a table we maintain (PG-E1-07). A menu item exists only when a command serves it: Playback is present with its three items *disabled* until Phase 3, because an enabled item that does nothing is worse than an absent one |
| Keyboard shortcuts | One registry, rendered per platform | Same | Same | Same | `peregrine-ipc::shortcuts` is the single source: the Settings list is generated from it over IPC and the shell dispatches the same answer, so a shortcut cannot be listed and inert. An entry holds `muda`'s `Modifiers`/`Code` built from its `CMD_OR_CTRL`, which is the same value the application menu is built from, rather than a macOS string beside a Windows one. Conflict detection refuses two entries claiming one accelerator and an entry claiming one an application menu item owns; the OS-owned items' accelerators are `muda`'s own and unreadable through its public API (PG-E1-08) |
| Single instance | A Unix socket in `/tmp`, keyed by the bundle identifier | A named mutex | A D-Bus name | Same as X11 | `tauri-plugin-single-instance` owns the lock; a second launch hands over its argument vector and working directory and exits before its own runtime starts. Focus is `unminimize` then `set_focus` on the existing window, and an argument reaches the same `external::forward` seam a `peregrine://` deep link does (PG-E1-04, PG-E1-11) |
| File and folder pickers | One `NSOpenPanel` for both | Two dialogs: `IFileOpenDialog` and `IFileDialog` in folder mode, so the caller must say which it wants | One GTK dialog for both | The portal through `xdg-desktop-portal` under Flatpak | `tauri-plugin-dialog`, behind `picker.ts` and its branded `UntrustedPath` (PG-E1-12). A picked path is *not* trusted input: the dialog proves a human chose it and nothing about the path, so it is re-validated through `PathGuard` where it is used |
| Cached images | `asset:` scheme answered in-process | `asset:` / `http://asset.localhost` | Same | Same | Tauri's `protocol-asset` feature with a scope filled at runtime, never in the config: the only directory allowed is `<OS cache directory>/images`, which holds the content-addressed poster, backdrop, still, and local-poster variants — the last of those being a frame this machine generated from the title's own file, keyed by the title under the provider slot `local` (ADR [0023](./adr/0023-in-app-video-surface.md) §6). `apps/desktop/src/lib/media.ts` is the only module that names `convertFileSrc`, so bytes never travel as base64 over IPC (AGENTS R11), and the static `assetProtocol.scope` in `tauri.conf.json` is empty, so an image loads only because the shell allowed its directory. A path outside the cache — including a symlink planted inside it that resolves out — is refused by the scope, and the library root is never exposed (PG-E2-08) |
| Code signing | Apple Developer ID + notarization | Azure Trusted Signing or EV cert | — | — | Unsigned Linux builds are the distribution default |

Known gaps, stated honestly:

- Wayland video embedding is the highest-risk platform item. The plan assumes a fallback path ships in the first release rather than blocking on it. `SPIKE-1`
- WebKitGTK on Linux renders text and scrolling differently from WebKit/Chromium; the design tokens include a Linux-specific font stack and the perf budget for Linux assumes no GPU compositing of large blurred layers.
- Windows Defender SmartScreen will warn on unsigned builds; the README documents this rather than pretending it doesn't happen.

---

## 18. Packaging, distribution, updates

**What exists today.** The matrix builds four artefacts — DMG for both macOS architectures, NSIS, and a deb plus an AppImage — and the macOS and Windows bundles carry mpv and ffprobe as resources. An explicitly unsigned release candidate now builds, uploads the bundles, writes `SHA256SUMS`, and publishes a prerelease; `cargo xtask release-check --allow-unsigned` is the local gate for that policy. Signing, notarisation, the signed manifest, the updater, rpm/Flatpak, and a verified clean-machine launch are still owed; the minisign path runs only when `RELEASE_SIGNING_CONFIGURED` is set.

```mermaid
graph LR
    Tag["git tag vX.Y.Z"] --> CI["GitHub Actions matrix<br/>macOS arm64+x86_64, Windows x64, Linux x86_64"]
    CI --> Sign["Sign + notarize<br/>(macOS, Windows)"]
    Sign --> Bundle["Bundles<br/>dmg, nsis, msi, AppImage, deb, rpm, Flatpak"]
    Bundle --> Manifest["Signed update manifest<br/>+ CycloneDX SBOM + changelog"]
    Manifest --> Rel["GitHub Release"]
    Rel --> Upd["tauri-plugin-updater<br/>stable / beta channels"]
```

| Concern | Decision |
| --- | --- |
| Update mechanism | `tauri-plugin-updater`, signed manifests, in-app "restart to update" prompt, never a forced restart during playback or an active download. |
| Sidecars | **Base install carries mpv and a minimal ffprobe** — enough to play anything and to probe codecs, HDR, and tracks. **Full ffmpeg is an on-demand component pack**, because a complete build is 60–170 MB per platform and transcoding only matters when casting. Both base sidecars are signed and versioned with their own licence files and a source offer. The optional pack is fetched with an explicit user action, verified against a pinned checksum and signature, and never executed with elevated privileges. |
| Reproducibility | Locked `Cargo.lock` and `pnpm-lock.yaml`; `cargo-vet`-style audit trail via `cargo-deny` + SBOM; release builds use `--locked` and `--offline` where possible. |
| Linux packages | AppImage (default), `.deb`, `.rpm`, Flatpak manifest in-repo with portal-based file access. |
| Channels | `stable` and `beta`, selected in Settings → Advanced; the beta channel is opt-in and clearly labelled. |
| Rollback | The updater keeps the previous version's bundle for one release cycle so a bad update can be reverted offline. |

---

## 19. Testing architecture

| Layer | Tooling | What it must prove |
| --- | --- | --- |
| Domain unit + property | `cargo-nextest`, `proptest`, `insta` | Release-name parsing, quality scoring, path guards, subtitle timing, state machines, invariants. This crate targets ~95% line coverage and 100% of public invariants property-tested. |
| Storage | `nextest` + temp-file SQLite + migration fixtures | Migrations from every prior schema version, FTS5 search correctness, single-writer serialization, crash-mid-transaction recovery. |
| Providers | `wiremock` + recorded fixtures + `insta` | Normalization snapshots, 429/5xx/timeout/malformed/oversized handling, ETag revalidation, cap enforcement. |
| Torrent | Loopback seeder↔leecher integration | Sequential streaming read-ahead, file selection, cancellation, restart recovery, inspection gate rejection cases. |
| Playback | Fake `PlayerBackend` conformance + per-OS smoke test against a fixture file | Session state machine, resume policy, next-up, track switching, position coalescing. |
| Casting | Stub renderer (in-repo fake) + manual device checklist | Capability negotiation, remux trigger decisions, position reconciliation, session teardown. |
| UI unit | `vitest` + Testing Library + `msw` for IPC | Component behaviour, keyboard operability, focus management, reduced-motion paths. |
| E2E (web layer) | Playwright against the Vite build with a mocked IPC bridge | Critical flows: onboarding, browse→detail→download, queue, library scan, settings, deletion confirmation. |
| E2E (packaged app) | `tauri-driver` where supported; scripted per-OS smoke suite otherwise | Window lifecycle, menus, deep links, updater, playback smoke, tray. |
| Performance | `criterion` benches + budget gate in CI + frame-time assertions in E2E | The table in [§14](#14-performance-architecture). |
| Soak / chaos | 72 h download+stream soak on a nightly schedule; fault injection for disk-full, network flap, corrupt torrent, provider outage | Memory growth, file-descriptor leaks, orphan child processes, recovery behaviour. |

Fixture policy: only public-domain or explicitly licensed sample media (Big Buck Bunny, Sintel, Tears of Steel, public-domain trailers) is committed, and always as a tiny synthesized clip where the mechanism under test does not need real content. No copyrighted posters are committed — image fixtures are generated placeholders with a schema-identical shape.

---

## 20. Repository layout

```
peregrine/
  apps/desktop/            Tauri app: src (React), src-tauri (thin shell), capabilities, icons
  crates/                  per §5, one directory per crate
  packages/                ui, theme, ipc-client, player-ui, i18n
  assets/branding/         logo, app icon sources, previews
  assets/themes/           shipped theme token files
  docs/                    architecture, data model, flows, roadmap, tasks, adr, research
  xtask/                   arch check, migration check, IPC codegen, release helpers
  .github/                 workflows, issue and PR templates, CODEOWNERS
  AGENTS.md                rules for human and agent contributors
  SECURITY.md              threat model and vulnerability reporting
  README.md
```

## 21. Open spikes

These are the questions that could change a decision. Each is a time-boxed experiment with a written outcome appended to the relevant ADR.

| ID | Question | Time box | Decision it unblocks |
| --- | --- | --- | --- |
| SPIKE-1 | Per-platform surface quality: `--wid` embedding plus overlay z-order on Windows and X11, an app-positioned child window on macOS (resize, fullscreen, Spaces, display change), and the Wayland degraded hand-off | 3 days | Confirms [adr/0013](./adr/0013-mpv-distribution-and-video-surface.md); decides whether a render-API ADR is worth opening |
| SPIKE-2 | Does mpv's `--stats` JSON plus `--term-status-msg` give us frame drops, hw-decode path, and audio underruns without a render API? | 1 day | Player diagnostics |
| SPIKE-3 | Verify, on the pinned librqbit 9.0.1 and all three OSes, the capabilities the registry confirms exist: per-file streaming with a steerable read-ahead window, `only_files` selection and later `update_only_files`, SOCKS5 and `bind_device_name`, and clean shutdown with restart reattachment. Also test whether SOCKS5 covers **UDP tracker** traffic, which is unverified | 3 days | Torrent engine ADR, VPN binding feature |
| SPIKE-4 | Blurhash vs thumbhash vs a 16 px WebP LQIP: decode cost, size, and visual quality at 342 px poster width. | 1 day | Placeholder strategy |
| SPIKE-5 | FTS5 ranking quality on 100 k titles with alt titles and cast, versus a trigram index; measure p95 latency. | 2 days | Search implementation |
| SPIKE-6 | Encoder ladder on reference hardware: direct play, HLS remux, then hardware H.264 versus `libopenh264` for the MKV/H.265/AC3 cases. Produces the published capability matrix | 2 days | Cast pipeline scope, [adr/0014](./adr/0014-ffmpeg-build-and-transcode.md) |
| SPIKE-7 | Cost of a full library scan + ffprobe on 2 000 files, and whether ffprobe can be avoided for common containers. Registry evidence says it cannot for HDR metadata, so this measures throughput only | 2 days | Library scan architecture |
| SPIKE-8 | Trakt device-OAuth progress sync round-trip and conflict handling with local progress. | 1 day | Sync scope for v1 |
| SPIKE-17 | OS authentication reality: Touch ID, Apple Watch, and device password on macOS; Windows Hello with PIN, fingerprint, and face plus clean unavailability reporting; and what GNOME and KDE actually offer (polkit agent, fprintd, both, or neither) including the message when neither exists | 2 days | [adr/0019](./adr/0019-local-accounts-and-parental-gating.md) |
| SPIKE-18 | Gating completeness audit: a permanent leak suite that tries to reach blocked content through search suggestions, similar titles, continue-watching, notification bodies, direct item IDs, the casting server, the Jellyfin API subset, and a hand-written MCP call | 3 days | Phase 8 exit criteria |
| SPIKE-14 | Export against reality: generate the scan-friendly tree and confirm a real Jellyfin and a real Plex install pick up items with correct metadata, artwork, and subtitle tracks without manual matching | 2 days | Phase 7 scope, [adr/0018](./adr/0018-media-server.md) |
| SPIKE-15 | Jellyfin client reality: connect the API subset to two real clients (one mobile, one TV) and record what each needs beyond the subset | 3 days | The true v1.4 scope |
| SPIKE-16 | Measure a 4K remux over 2.4 GHz, 5 GHz, and Ethernet, and record where direct play stops being viable | 1 day | Link-budget thresholds in the server design |
| SPIKE-12 | MCP conformance: `server/discover` and version negotiation against one handshake-era client (`2025-11-25`) and one current client (`2026-07-28`), schema acceptance, the `pending_approval` round trip, revocation mid-session, and the prompt-injection fixture | 3 days | [adr/0017](./adr/0017-mcp-server.md), Phase 6 scope |
| SPIKE-13 | Monitoring signal quality: real air-date-to-release latency for ten shows with and without an indexer, and how often providers disagree about a date | 2 days | UI copy and cadence defaults for [adr/0016](./adr/0016-monitoring-and-availability.md) |
| SPIKE-11 | Per-OS notification reality check: the working sound value for each event class; whether an omitted sound is genuinely silent on macOS, Windows, GNOME Shell, and dunst; whether a packaged macOS build prompts from `Notification.requestPermission()`; Linux behaviour with no notification daemon; and the Windows click-reporting window | 1 day | Notification policy, [adr/0015](./adr/0015-native-notifications.md) |
| SPIKE-10 | Find the real sustained rate limits: ramp TMDB at 5/10/20/40 rps for 60 s and record the first `429`, and probe TVDB v4's undocumented limit with a key. Decides whether per-host limiters can be constants or must adapt | 2 days | Provider rate-limit policy, sync cadence |
| SPIKE-9 | `specta` + `tauri-specta` at the pinned `2.0.0-rc.25`: generate bindings for a representative command surface, confirm enums, `Option`, and nested generics round-trip, and confirm the generated client compiles in the frontend. There is no stable 2.x, so the fallback is hand-written DTOs plus a generation check | 2 days | The IPC contract's code-generation choice |
