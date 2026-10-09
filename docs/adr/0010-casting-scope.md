# ADR 0010 — Casting scope: Chromecast and DLNA, AirPlay out

- **Status:** Accepted
- **Date:** 2026-10-09
- **Related:** [0004](./0004-mpv-out-of-process.md), [0009](./0009-licensing-and-redistribution.md)

## Context

Users expect to throw a video at a TV. The realistic protocols from a cross-platform desktop app are Google Cast (CASTV2 over TLS with protobuf), DLNA/UPnP AV (SSDP discovery, SOAP AVTransport), and AirPlay. Each has a different cost: Google Cast needs an HTTP server with correct Range and metadata semantics plus remux or transcode for unsupported codecs; DLNA needs SSDP and DIDL-Lite metadata with the same server; AirPlay from non-Apple platforms is largely undocumented and has no reliable cross-platform implementation.

Casting also has a security dimension: the desktop app must briefly become a media server on the local network.

## Decision

1. **Ship Chromecast and DLNA in the same release as the rest of the playback stack**, but keep casting out of the core 1.0 critical path so it cannot delay the release ([PLAN.md](../PLAN.md) §4).
2. **AirPlay is out of scope.** Decided, not deferred indefinitely: it is documented as a known limitation, and Apple-platform users can select the AirPlay output through the OS audio path if it applies.
3. **One embedded HTTP server, two listeners.** A loopback listener for the local player (always available during playback) and a separate LAN listener that exists **only** while a cast session is active. Different bind addresses, different tokens, different lifetimes.
4. **Loopback first, LAN never by default.** Enabling casting is an explicit Settings action that states in plain language that any device on the same network could reach the session URL. A PIN is available for shared or untrusted networks.
5. **A capability model decides delivery**, not the user: direct play when the renderer supports the container and codecs; remux to fragmented-MP4 HLS when only the container is unsupported; transcode only when the video codec is unsupported. Every session shows which mode is in use and why.
6. **Transcoding is a bounded sidecar job** following the ladder in [0014](./0014-ffmpeg-build-and-transcode.md): hardware H.264 encoder first, `libopenh264` second, and a clear refusal when neither exists, because our LGPL ffmpeg has no libx264. Explicit CPU cap, cancellable, and never running for the desktop playback path.
7. **Subtitles follow the renderer's capability.** Sidecar WebVTT when supported, burn-in when the format is bitmap (PGS, VobSub) or unsupported.
8. **Position is reconciled** on session end so casting and desktop playback share one resume point, in both directions.
9. **Discovery uses only the standard protocols** (mDNS `_googlecast._tcp`, SSDP `M-SEARCH`). No port scanning of the local network, no subnet sweeps.

### Verified refinements (2026-10-09)

Research into the actual libraries and Google's published media support changed three details without changing the decision.

- `rust_cast` is a transport, not a solution: it has **no device discovery** (mDNS is ours, via `mdns-sd`) and **no track or subtitle API** (`EDIT_TRACKS`, `activeTrackIds`, and `textTrackStyle` are absent), so receiver-side subtitle selection is hand-rolled or it does not ship. Version 0.21.0 is MIT and only marginally maintained.
- There is **no `dlna` crate**, and `ssdp` has been dead since 2018. DLNA therefore means `rupnp` for SSDP discovery and SOAP invocation, plus hand-rolled `AVTransport`, `RenderingControl`, and `ConnectionManager` XML. That is real work and is budgeted as such.
- Chromecast's supported container list is MP2T, MP3, MP4, OGG, WAV, and WebM — **Matroska is absent**; HEVC is unsupported in Transport Stream containers, so H.265 must go to MP4/CMAF; first- and second-generation devices have no HEVC at all; AC-3 and E-AC-3 are passthrough-only and must be probed with `canDisplayType()`; DTS and TrueHD are never supported; and subtitle resources must be WebVTT or TTML **and CORS-enabled**, which our own server controls. Transcoded H.264 must come from a hardware encoder or `libopenh264`, because our LGPL ffmpeg has no libx264 ([0014](./0014-ffmpeg-build-and-transcode.md)).
- DLNA renderers routinely advertise protocols in `GetProtocolInfo` that they cannot actually play, so the capability model treats advertised support as a hint and verifies on the first attempt.

## Consequences

Positive: a locally served cast path gives us full control over Range, HLS, and subtitle delivery, which a Node `castv2-client` sidecar would not; the security posture is explicit and switchable; the capability model produces honest, explainable behaviour instead of "it doesn't work".

Negative and accepted:

- Remux and transcode pipelines are real work (roughly 20–38 ideal days) and can dominate a release if allowed to. Hence the deliberate exclusion from the core critical path.
- Codec coverage on the renderer side will always have gaps. Mitigated by documenting the matrix and by showing the delivery mode in the UI.
- Chromecast's default media receiver ignores some metadata; the UI should not promise rich listing behaviour it cannot deliver.
- AirPlay absence is a visible limitation for Mac users and must be stated in the README rather than discovered.

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| Skip casting entirely | It was an explicit product requirement, and the LAN-server infrastructure also enables future features (a phone remote, a headless mode) |
| Rely on the OS or mpv to cast (for example `mpv --audio-device` or a system-level AirPlay target) | Covers audio routing at best; not video casting to a TV |
| Bundle a Node `castv2-client` sidecar | Adds a runtime, a second protocol implementation, and an IPC hop while still requiring our own HTTP server |
| Transcode everything for maximum compatibility | Wastes CPU and quality for the common direct-play case |
| Discover devices by scanning the subnet | Invasive, slow, and indistinguishable from malware behaviour to network monitoring |
| Leave the LAN listener up permanently for faster session start | A media server running the entire time the app is open, for no user benefit |

## Verification

`SPIKE-6` measures direct-play versus remux versus transcode success rates for representative files and decides the initial capability matrix. Phase 4 exit criteria require the codec matrix to be documented with known limitations, and a test asserting that no LAN listener exists when casting has never been enabled.
