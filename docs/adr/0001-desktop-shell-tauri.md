# ADR 0001 — Desktop shell: Tauri v2

- **Status:** Accepted
- **Date:** 2026-10-09
- **Related:** [0002](./0002-rust-core-owns-state.md), [0004](./0004-mpv-out-of-process.md), [0009](./0009-licensing-and-redistribution.md)

## Context

Peregrine must feel like a native desktop application on macOS, Windows, and Linux, install without bloat, animate a virtualized poster grid at 60–120 fps, and own a substantial backend: a torrent engine, a metadata scraper, a library scanner, subtitle tooling, and an embedded HTTP server for streaming and casting. The team is one to three developers.

## Decision

Build on **Tauri v2** with a Rust workspace backend and a React 19 + TypeScript frontend served into the platform webview (WKWebView, WebView2, WebKitGTK).

Concretely:

- `apps/desktop/src-tauri` is a thin shell: window and menu wiring, capability files, and command registration. No business logic.
- All product logic lives in Rust crates that build and test without Tauri, so the core is testable headlessly and reusable by a future CLI or companion service.
- Frontend talks to the core only through generated typed IPC and the asset protocol.
- The UI is a static bundle; nothing is fetched remotely to run the app.

## Consequences

Positive: 10–15 MB installers before sidecars; the backend is memory-safe; window, tray, menu, badge, and notification APIs are native; a per-window permission model exists; the core can be tested without a GUI.

Negative, accepted explicitly:

1. Three webview engines means three rendering realities. Mitigation: Linux-specific font stack, effect budgets that assume weak GPU compositing on WebKitGTK, ordered workarounds for known DMA-BUF failures documented in [research/desktop-ux.md](../research/desktop-ux.md), and a per-OS visual regression check in the release checklist.
2. No in-process video compositing; HTML cannot be a DOM sibling of the video surface. Mitigation: the overlay is its own layer over the mpv surface ([ADR 0004](./0004-mpv-out-of-process.md)).
3. A code-generation step links Rust types to TypeScript. Mitigation: CI fails when the committed bindings drift from a regeneration.

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| Electron | 150 MB+ installers because Chromium and Node ship with every app; directly contradicts the no-bloat requirement; a Node backend gives up memory safety in the component that parses hostile torrent data |
| Flutter desktop | Ships its own rendering engine; text rendering, input, and window behaviour do not match platform conventions; the "does not look like a web app" requirement is hardest to satisfy in a non-native toolkit |
| Qt (C++ or PySide) | C++ development cost for this team, LGPL/commercial licensing complexity for redistributed binaries, and a much larger install |
| Native Rust UI (egui, Iced, GPUI, Slint) | Every layout, animation, text-shaping, and accessibility primitive becomes ours. Correct for a utility, wrong for a Netflix-grade grid with shared-element transitions and per-OS typography |
| .NET MAUI or Avalonia | Thinner Linux story, weaker media and torrent ecosystem in the same language, and a second runtime to ship |
| Local web server plus a browser | The user asked for a desktop app with OS integration; a browser tab cannot own a tray, badges, deep links, or a global playback surface |

## Verification

- Phase 0 exit criteria: the packaged app launches on all three OSes, theme switching is instant, and `xtask check-arch` enforces the layering.
- `SPIKE-1` decides the Wayland video-surface path, the highest-risk consequence of this decision.
