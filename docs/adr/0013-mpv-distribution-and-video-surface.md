# ADR 0013 — mpv distribution and the video surface per platform

- **Status:** Accepted. Supersedes ADR [0004](./0004-mpv-out-of-process.md) §Decision bullet 4 (embedding via `--wid` on macOS) and the mpv sentence in ADR [0009](./0009-licensing-and-redistribution.md) §Decision 3.
- **Date:** 2026-10-09
- **Evidence:** [research/dependency-matrix.md](../research/dependency-matrix.md) §4.2, verified against mpv's `DOCS/man/options.rst`, `Copyright`, `video/out/cocoa_common.m`, `video/out/wayland_common.c`
- **Related:** [0004](./0004-mpv-out-of-process.md), [0009](./0009-licensing-and-redistribution.md), [0012](./0012-streaming-readahead.md)

## Context

Two verified facts invalidated the original plan.

**Fact 1: an LGPL-only mpv build has no Linux X11 video output.** mpv is GPL-2.0-or-later by default and LGPL-2.1-or-later only when built with `-Dgpl=false`. That switch disables `vo_x11.c`, `vo_xv.c`, and `x11_common.*`, which are GPL-only. mpv upstream states the switch "does not in itself create a LGPLv2.1+ license grant" and that "it's not recommended to build mpv CLI in LGPL mode at all" — LGPL mode exists for libmpv. Choosing LGPL mode therefore means silently giving up Linux X11 playback.

**Fact 2: `--wid` is unimplemented on macOS and Wayland.** `--wid` is documented for X11, win32, and Android only. X11 implements it by reparenting. `video/out/cocoa_common.m` and `video/out/wayland_common.c` contain no `WinID` reference at all — not undocumented, unimplemented. On Wayland there is a second, independent blocker: Wayland has no protocol for one client to embed another client's surface, and it does not let a client position itself, so the "float a borderless window exactly over the app" trick is unavailable there too.

## Decision

### 1. Ship the ordinary GPL mpv build as a separate executable

We distribute upstream's normal mpv binary, at arm's length, driven over JSON IPC. A separate program invoked as a subprocess is an aggregate rather than a derivative work, so no copyleft reaches our MIT/Apache code. Our only added obligation is the GPL's: ship mpv's licence text and a written offer of the exact corresponding source for the binary we ship.

The LGPL-only build is rejected outright: it would trade a licence obligation we can satisfy for the loss of Linux X11 playback.

We also refuse the two linking traps: `mpv-client` is GPL-3.0 and is never used, and the `libmpv2` crate is itself LGPL-2.1 while Cargo links crate code statically, which would put LGPL code in our binary.

### 2. Video surface strategy per platform

| Platform | Mechanism | Why |
| --- | --- | --- |
| Windows | `--wid` with the child window's `HWND` as `uint32_t`; mpv reparents and fills it | Documented and implemented |
| Linux X11 | `--wid` with the `Window` id; mpv sets it as parent | Documented and implemented |
| macOS | **Positioned borderless child window** owned by the app, kept in sync with the player region; `--wid` is not an option | `--wid` is unimplemented on Cocoa; a child window we position is the supported approach |
| Linux Wayland | **Degraded hand-off** for 1.0: mpv opens its own toplevel in theatre mode and the webview hides its video region. The render API is the future path and needs its own ADR | No `--wid`, no cross-client embedding, and no client-side positioning. The in-process render API would require a `dlopen`-based dynamic load of libmpv plus an exception to the no-`unsafe` rule, which is a decision that deserves to stand alone |

The surface mechanism is one trait method (`attach_surface`) per platform module, so the overlay, session state machine, and resume logic are written once and do not know which path is live.

### 3. Sibling-window synchronization is part of the design, not a detail

On Windows and X11 the WebView2 or WebKitGTK child window and mpv's child window are siblings, so `--wid` alone does not compose them. The implementation must:

- Resize and move mpv's child window in lockstep with the player region, driven by the same layout pass that positions the overlay, never by a polling loop.
- Control z-order explicitly (raise or lower, or reserve a hole in the webview where the video surface belongs), because the overlay must sit above the video and the rest of the UI must not.
- Handle display changes, fullscreen transitions, DPI changes, and window restore by recomputing both regions in one place.

This is the highest-risk part of the player work and is why the platform modules exist separately.

## Consequences

Positive: we keep the X11 and Wayland-capable X11 lineage of output and hardware decode paths; licensing stays trivial (aggregate plus a source offer); mpv can still be upgraded independently; the UI never links media code.

Negative and accepted:

- We ship a GPL binary and must maintain a source offer per release. `xtask check-notices` fails a release that omits it.
- macOS gets a positioned child window rather than true embedding, so handoff between the overlay and the video region is our problem. Tested as part of `SPIKE-1`.
- Wayland ships a visibly degraded mode in 1.0. This is documented in the README and in the platform matrix rather than hidden.
- The sibling-window z-order problem does not exist on macOS's child window path but does on Windows and X11, so three platform modules carry three different sets of edge cases.

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| LGPL-only mpv build to avoid the GPL | Loses all Linux X11 video output; upstream discourages it; and the licence obligation it avoids is one we can meet |
| Link libmpv dynamically (runtime `dlopen`) in 1.0 | Requires `unsafe` FFI (forbidden by AGENTS R9 without a dedicated ADR) and per-platform GL surface code; it buys the Wayland case only, and only with more risk than the degraded hand-off |
| `libmpv2` crate | LGPL-2.1 and statically linked by Cargo; would put LGPL code inside our binary |
| `mpv-client` crate | GPL-3.0; would force the entire application to GPL |
| Fullscreen mpv as the only playback surface | Loses the overlay entirely; a media app without resume, track switching, or next-up on screen is a worse product than one with a Wayland caveat |
| Force X11 via XWayland on Wayland sessions | Unreliable and user-hostile; XWayland would need a matching X11 window and behaves inconsistently per compositor |

## Verification

`SPIKE-1` (3 days) now measures something narrower and concrete:

1. Windows and X11: `--wid` embedding, resize, fullscreen, DPI change, and z-order against the overlay.
2. macOS: child-window positioning fidelity during window resize, fullscreen, display change, and Spaces switching.
3. Wayland: confirm the degraded hand-off is usable and that the app never shows a black region where video should be.
4. Confirm `--no-config --load-scripts=no` is honoured and that IPC works on a unix socket, a named pipe, and with the app's own socket path.

A macOS or Wayland failure does not reopen this ADR; it triggers the render-API ADR as a follow-up with measured cost.
