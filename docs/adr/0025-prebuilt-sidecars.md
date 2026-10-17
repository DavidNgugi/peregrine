# ADR 0025 — Prebuilt sidecars: GPL-3 mpv and LGPL-3 ffmpeg on Windows and Linux

- **Status:** Accepted. Refines [0009](./0009-licensing-and-redistribution.md) §Decision 3 and supersedes [0014](./0014-ffmpeg-build-and-transcode.md) §Decision 1's ban on `--enable-version3`.
- **Date:** 2026-10-17
- **Evidence:** live artifact verification recorded in `packaging/sidecars.lock.json`; sizes and licence strings read from the shipped binaries, not from documentation.

## Context

The base install must not ask the user to install a video player or a prober. The 2026-10-17 survey found three facts that make the previous policy impossible to keep:

1. **No redistributable LGPL ffmpeg or ffprobe exists for macOS.** ffmpeg.org publishes source only; the common macOS binaries are GPL and Intel-only. macOS needs a build of our own either way.
2. **BtbN's "LGPL" builds are LGPL-3.0-or-later**, because the embedded configure line carries `--enable-version3`. ADR 0014 forbade that switch.
3. **mpv's official binaries are GPL-3.0**, because they embed FFmpeg built with `--enable-gpl --enable-version3`. The project already redistributes GPL-3 code as a separate executable through Prowlarr, so this is a licence we already accept, with a source offer that must now name FFmpeg as well as mpv.

The practical question was whether to hold the no-version3 line and build ffmpeg for four targets, or to accept LGPL-3 for the Windows and Linux sidecar and build only where no binary exists. Bundling all four is the product requirement; the second path reaches it with one build (macOS ffmpeg) and one build-dependent binary (Linux mpv).

## Decision

### 1. Bundle mpv in the base install

- **macOS and Windows:** the official upstream release builds, pinned by version, asset name, and SHA-256 in `packaging/sidecars.lock.json`. They are GPL-3.0 as shipped. The written source offer names the mpv commit and the FFmpeg source the binary embeds.
- **Linux:** built in CI from `mpv-build` at a pinned commit and digest-pinned base image, and bundled as one binary. GPL-2.0-or-later.

The player is the product. A first-run download for it is a failed first run.

### 2. Bundle ffprobe, and keep ffmpeg on demand

`ffprobe` ships in the base install because the library scan cannot work without it. `ffmpeg` remains an on-demand component pack, as ADR 0014 decided: it is 77–168 MB and only transcoding and subtitle extraction need it.

### 3. Accept LGPL-3.0-or-later for the Windows and Linux ffmpeg sidecar

FFmpeg is a separate executable, invoked as a subprocess, never linked. The obligations are the ones already recorded for a sidecar: ship the licence text, the exact configure line, and a written offer of the corresponding source. `--enable-version3` upgrades the licence version; it does not change the aggregation analysis in ADR 0009.

The macOS build stays ours and uses the same configure shape, including `--enable-version3`, so every platform ships one licence string: `LGPL-3.0-or-later`. `--enable-gpl` and `--enable-nonfree` remain forbidden.

### 4. The pins are data, not prose

`packaging/sidecars.lock.json` holds the URL, SHA-256, archive shape, binary paths, and licence for every shipped sidecar per platform and architecture. `scripts/fetch-sidecars.mjs` is the only thing that downloads them, and it refuses a mismatch. A version, hash, or URL in a document that disagrees with the lock file is the document's bug.

`sidecars.toml` keeps the licence, source, and source-offer facts that `cargo xtask check-notices` turns into `THIRD_PARTY.md`.

### 5. Platform floors follow the binaries

The official mpv macOS builds require macOS 14 on arm64 and macOS 15 on Intel. That becomes the floor for the bundled build. A lower floor requires building mpv ourselves, which is the follow-up if the floor is unacceptable.

## Consequences

- The installer grows by roughly 100 MB (mpv) plus a few MB for ffprobe. The figures in `docs/DEPENDENCIES.md` §5 were 3–7× too low and are corrected in the same change.
- The macOS bundle carries 97 nested dylibs from mpv.app. They must be re-signed with our identity and notarised as nested code; `spctl` rejects the upstream ad-hoc signature.
- The source offer for mpv must name FFmpeg's source and the versions inside the bundle, not only mpv's tag.
- A user can still point the app at their own mpv or ffmpeg, which keeps the escape hatch ADR 0014 promised.
