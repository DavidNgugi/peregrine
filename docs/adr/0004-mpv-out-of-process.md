# ADR 0004 — mpv runs out of process, driven over JSON IPC

- **Status:** Accepted
- **Date:** 2026-10-09
- **Verification:** `SPIKE-1` (video surface per platform), `SPIKE-2` (telemetry)
- **Superseded in part by:** [0013](./0013-mpv-distribution-and-video-surface.md) for the mpv build and the per-platform video surface
- **Related:** [0001](./0001-desktop-shell-tauri.md), [0009](./0009-licensing-and-redistribution.md), [0012](./0012-streaming-readahead.md)

## Context

Peregrine must play essentially anything a user has downloaded — Matroska, MP4, H.264, H.265, AV1, HDR10, Dolby Vision, DTS, TrueHD, embedded bitmap subtitles — with hardware decoding on all three platforms. It must also read playback position at high frequency, switch audio and subtitle tracks at runtime, and never take the whole application down when a decoder or GPU driver faults.

Options: link `libmpv` in-process and use its render API; link `libvlc`; decode with ffmpeg ourselves; or run mpv or VLC as a child process and drive it over its control interface.

## Decision

Run **mpv as a child process**, driven over **JSON IPC** through a named pipe on Windows and a Unix socket elsewhere, with the video surface attached per platform: `--wid` on Windows and X11, an app-positioned child window on macOS, and a documented degraded hand-off on Wayland ([0013](./0013-mpv-distribution-and-video-surface.md)).

Hardening is non-negotiable and applied on every launch:

```
--no-config --load-scripts=no --no-ytdl --no-auto-sub
--input-ipc-server=<per-session socket> --idle=yes --keep-open=no
--hwdec=<detected> --vo=<platform surface> --wid=<handle>   # Windows and X11 only
```

Additional rules:

- The process is owned by a guard that reaps it on drop; startup kills orphans identified by PID **and** process start time, never by name.
- Surface attachment is platform-specific and defined in [0013](./0013-mpv-distribution-and-video-surface.md): `--wid` where mpv implements it, an app-positioned child window on macOS, and a degraded hand-off on Wayland. The render API remains a possible follow-up ADR, not part of 1.0.
- Playback position is read from mpv properties and written to SQLite at most once every five seconds and on every state change, so a crash loses at most five seconds.
- Telemetry (dropped frames, decoder, hardware-decode path, cache speed) comes from mpv's `--stats` JSON and property notifications rather than a render-API hook, if `SPIKE-2` confirms coverage.

## Consequences

Positive: a crash or GPU fault kills the video window, not the application; mpv can be upgraded independently of a release; no copyleft code enters our process, because mpv ships as a separate executable under its own GPL licence and an aggregate of separate programs does not reach our code ([0013](./0013-mpv-distribution-and-video-surface.md)); one integration path covers every format mpv supports, which is more than we could build; no `unsafe` FFI in our crates.

Negative and accepted:

- In-window embedding is limited by mpv itself: `--wid` is unimplemented on macOS and Wayland. macOS uses an app-positioned child window and Wayland ships a documented degraded hand-off rather than pretending parity ([0013](./0013-mpv-distribution-and-video-surface.md)).
- HTML cannot be composited as a DOM sibling of the video surface, so the overlay is its own layer with its own positioning rules.
- Process lifecycle, IPC framing, and error propagation are ours to build and test (roughly a week of work in Phase 3).
- Installer size grows by the mpv binary per platform.

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| `libmpv` linked in-process with the render API | A decoder fault takes down the app; `unsafe` FFI and per-OS GL surface code; rebuilding the app to update mpv; the render API buys only the Wayland case, which it does not solve cleanly either |
| `libvlc` | VLC 4.0 removed the `libvlc_event_manager_t` API in favour of versioned callback structs, so most documentation and examples are stale; the player surface is far heavier than needed; LGPL-2.1 dynamic linking is workable but adds legal review overhead for no functional gain |
| ffmpeg decode in-process | Rebuilding a player: A/V sync, seeking, track switching, subtitle rendering with libass, HDR tone mapping, and hardware decode negotiation are years of work |
| HTML5 `<video>` in the webview | Codec support is at the mercy of the webview; no Matroska, no DTS, no bitmap subtitles, no reliable HDR path, and hardware decode control is absent |
| Bundling VLC or libvlc instead | libVLC has a more complete embedding API (an `NSView` on macOS, `HWND`, X11) but requires linking a large LGPL library, brings `unsafe` FFI into an otherwise `unsafe`-free codebase, has a 4.0 API break that leaves ecosystem examples stale, and still does not solve Wayland embedding |

## Verification

- `SPIKE-1`: measure embedding quality, resize and fullscreen behaviour, input focus, and the Wayland fallback on all four surfaces.
- `SPIKE-2`: confirm `--stats` JSON covers dropped frames, hardware-decode path, and audio underruns.
- Phase 3 exit criteria: playback survives a stall, an mpv crash, and a network flap without restarting the app.
