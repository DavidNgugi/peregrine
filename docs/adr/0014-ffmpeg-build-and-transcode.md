# ADR 0014 — ffmpeg build configuration and the cast transcode strategy

- **Status:** Accepted. Refines ADR [0009](./0009-licensing-and-redistribution.md) §Decision 3 and ADR [0010](./0010-casting-scope.md) §Decision 6.
- **Date:** 2026-10-09
- **Evidence:** [research/dependency-matrix.md](../research/dependency-matrix.md) §4.4 and §4.5, verified against [ffmpeg.org/legal.html](https://ffmpeg.org/legal.html) and `configure`'s `EXTERNAL_LIBRARY_GPL_LIST`
- **Related:** [0009](./0009-licensing-and-redistribution.md), [0010](./0010-casting-scope.md)

## Context

Peregrine ships ffmpeg and ffprobe as sidecars for probing, remuxing, subtitle extraction, and casting. ffmpeg is LGPL-2.1-or-later by default, but "if those parts get used the GPL applies to all of FFmpeg": compiling with `--enable-gpl` or `--enable-nonfree` makes the binary GPL, and `--enable-version3` upgrades the (L)GPL version. The practical consequence for casting was not obvious until verified: **libx264 is in ffmpeg's `EXTERNAL_LIBRARY_GPL_LIST`**, so an LGPL-only ffmpeg cannot encode H.264 with libx264 — the default answer for "transcode for Chromecast" is unavailable to us.

## Decision

### 1. Ship LGPL-only ffmpeg

Build with neither `--enable-gpl` nor `--enable-nonfree`, and do not use `--enable-version3`. The exact `configure` line for each shipped binary is recorded in the repository beside the pinned version and checksum, and shipped with the sidecar.

Available under this configuration: remuxing (`-c copy`), subtitle conversion, AC-3 and DTS to AAC through ffmpeg's own LGPL AAC encoder, hardware H.264 encoders (VideoToolbox, NVENC, QSV, AMF, VAAPI — ffmpeg's own source for these is LGPL), and `--enable-libopenh264`, which sits in `EXTERNAL_LIBRARY_LIST` rather than the GPL list.

Unavailable: libx264, libx265, and every other GPL-only external library.

### 2. Cast transcode as a ladder, with hardware encoding preferred

| Step | Delivery | When | Encoder |
| --- | --- | --- | --- |
| 1 | Direct play | Container and codecs in the renderer's supported set | none |
| 2 | Remux to fMP4 HLS | Container unsupported, codecs fine (H.264 in MKV, H.265 to MP4/CMAF) | none (`-c copy`) |
| 3 | Hardware H.264 transcode | Video codec unsupported by the device | VideoToolbox, NVENC, QSV, AMF, or VAAPI |
| 4 | `libopenh264` software transcode | Step 3 unavailable on the machine | libopenh264 |
| 5 | Refuse with an explanation | No H.264 encoder is available at all | — |

Step 5 is deliberate. We do not ship a GPL ffmpeg to make step 3 succeed, and we do not lie to the user: the UI says which delivery mode is in use and, when casting is impossible, why. A user who wants libx264 can point Peregrine at their own ffmpeg build in Settings, with a one-line note that their build is their responsibility.

The renderer matrix from the verified research governs step 1: Chromecast's container list is MP2T, MP3, MP4, OGG, WAV, and WebM — **Matroska is absent**; HEVC is unsupported in Transport Stream containers (so H.265 goes to MP4/CMAF, never TS segments); first- and second-generation Chromecast devices have no HEVC at all; AC-3 and E-AC-3 are passthrough-only and must be probed; DTS and TrueHD are never supported; and all subtitle resources must be WebVTT or TTML **and CORS-enabled**, which our own HTTP server controls.

### 3. ffprobe stays the authoritative probe

Verified comparison across ffprobe, `lofty`, `matroska`, `mp4parse`, `mp4`, and `symphonia`: **only ffprobe exposes HDR mastering metadata** (ST 2086, MaxCLL, MaxFALL) and dynamic metadata (HDR10+, Dolby Vision RPU). `matroska` gives in-process MKV structure and `lofty` gives audio tags, but neither covers video colour or mastering metadata, and the pure-Rust MP4 parsers have not shipped since 2023.

So: ffprobe for anything authoritative, cached by path, modification time, and probe version; `matroska` may be used for cheap in-process inspection where a probe spawn is unnecessary; `lofty` only for audio tags if a need appears. Probe results are batch-processed and cached because each spawn costs 10–50 ms.

## Consequences

Positive: no copyleft code in our process; a casting path that works on machines with hardware encoders, which is most machines that can run this app; HDR metadata available for display; the licence obligations are mechanical and checkable.

Negative and accepted:

- Without libx264, a machine with no hardware H.264 encoder cannot cast codecs the renderer rejects. Accepted, surfaced in the UI, and escapable by pointing at a user-supplied ffmpeg.
- A self-built `libopenh264` does not inherit Cisco's gratis binary's MPEG-LA patent coverage. This is recorded as a known risk; the fallback exists for capability, not as a licensing claim, and step 5 exists so nothing depends on it.
- Sidecar payload grows (roughly 70–80 MB per platform for the pair), and macOS notarization and Windows signing must now cover two more binaries per platform.
- The full obligation set is now explicit and must be honoured per release: corresponding source for the exact binaries on the same distribution channel, the `configure` line, a `git diff` if patched, credit in the About box and on any download page, no obfuscated binary renames, and no EULA term that forbids reverse engineering.

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| Ship a GPL ffmpeg to get libx264 | Makes the ffmpeg binary GPL, adds a GPL source offer per release, and buys only software H.264 encoding on machines without hardware encoders |
| Link libav* into the app | Puts (L)GPL code inside our binary, complicates the relink obligation, and loses the crash isolation we get from a subprocess |
| Transcode with a non-ffmpeg encoder | Rebuilding encoder integration for one fallback path |
| Require the user to supply their own ffmpeg | Breaks playback and subtitle features out of the box; acceptable as an override, not as the default |
| Pure-Rust probing only | Would silently drop HDR and Dolby Vision display, which is precisely the metadata a modern media app should show |

## Verification

- `xtask check-notices` fails a release whose sidecar *declaration* lacks a licence, a source offer, or a recorded `configure` line. Checking that the licence texts themselves are present in the bundle is owed by PG-E26-01.
- `SPIKE-6` measures the encoder ladder on reference hardware and produces the published capability matrix.
- A CI fixture test asserts that the shipped ffmpeg reports no `--enable-gpl` configuration.
