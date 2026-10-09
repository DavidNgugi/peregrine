# ADR 0018 — Media server: what is possible, in what order, and what we refuse

- **Status:** Accepted as **post-1.0 roadmap** (Phase 7). Maintainer decisions recorded 2026-10-09: **v1.3 (sharing and DLNA) is the active sub-phase**, and **remote access is out of scope — LAN only, with a VPN as the documented remote path**.
- **Date:** 2026-10-09
- **Evidence:** Jellyfin library documentation (Movies, Placeholders — naming, NFO sidecars, local artwork, `.disc` placeholders) verified 2026-10-09 at `jellyfin.org/docs/general/server/media/`; Plex's own description of its architecture at `support.plex.tv/articles/200288286-what-is-plex`; `api.jellyfin.org` (ReDoc, the published API surface)
- **Related:** [0010](./0010-casting-scope.md), [0014](./0014-ffmpeg-build-and-transcode.md), [0016](./0016-monitoring-and-availability.md), [0017](./0017-mcp-server.md)

## Context

The request was "be a media server and stream to Jellyfin or Plex". That phrasing contains a direction error worth correcting before any code is written, because the error determines whether the feature is a two-week job or an impossible one.

**Jellyfin and Plex are servers, not players.** Plex's own documentation describes the flow: install the Plex Media Server, add libraries, install the Plex app, "press play and enjoy your media as it streams from the Plex Media Server to your device". Jellyfin is the same shape. There is no supported way for a third-party application to *push* media into either one, and **Plex clients cannot be pointed at a foreign server at all** — they authenticate and discover through Plex's own account infrastructure. Any attempt to impersonate a Plex server would be a terms-of-service violation and would break on the next authentication change. That request is refused on technical grounds, not on preference.

What is actually possible is four different things, and they have very different costs:

| # | Integration | Works with | Cost | Value |
| --- | --- | --- | --- | --- |
| A | **Scan-friendly export**: write a library tree that their scanners understand — their naming convention, NFO sidecars, local artwork, subtitle suffixes — inside a folder they index | Jellyfin **and** Plex | Low | High, and it works with both today |
| B | **Placeholder items**: `.disc` files so a title exists in the Jellyfin library before or without a local file | Jellyfin only (Plex has no equivalent) | Low | Medium |
| C | **Jellyfin-compatible API subset**: Peregrine serves the Jellyfin API so Jellyfin *clients* — Android TV, Findroid, Streamyfin, Infuse, the Kodi addon — browse and play from Peregrine's library | Jellyfin clients (not the Jellyfin server, not Plex) | High | Highest: an entire client ecosystem for free |
| D | **DLNA/UPnP server**: announce ourselves, expose ContentDirectory, serve over the byte-range server we already have | TVs, consoles, VLC, Kodi | Medium | Medium; already half-built for casting |
| E | **Peregrine as a client** of an existing Jellyfin or Plex server — browse *their* library, play through mpv, write progress back | Their servers | Medium | High for people who already run one |

A, B, and D ship value without touching anyone's protocol; E is the reverse direction and cheaper than C; C is the real prize but the largest single piece of work in the roadmap.

## Decision

### 1. Wi-Fi is a first-class target, and the server binds to interfaces, not cables

Whether the bytes travel over Wi-Fi or Ethernet is invisible to HTTP. The server binds an address; the transport does not matter. What *does* matter, and what the design must handle:

| Concern | Design response |
| --- | --- |
| Multiple interfaces are normal (Ethernet, Wi-Fi, VPN, container bridges, Thunderbolt) | The share configuration lists interfaces to bind and advertise on, defaulting to all private-range addresses. A "detected LAN addresses" list with the primary one highlighted, never a blind bind to `0.0.0.0` on every address including a VPN | 
| Discovery over Wi-Fi is the fragile part, not streaming | **mDNS (DNS-SD) is the primary discovery path**, because SSDP's multicast is sent at the Wi-Fi basic rate, is frequently filtered, and breaks across access points and VLANs. SSDP remains best-effort for DLNA. A **QR code and a short URL** are always available as the manual path, and the UI never assumes discovery worked |
| Client isolation silently blocks everything | The first-run sharing check probes whether a client can reach the server, and the failure copy names access-point isolation and guest networks explicitly, because "server not found" is otherwise undiagnosable |
| 2.4 GHz is not enough for large files | A 4K remux peaks around 60–100 Mbit/s; 2.4 GHz real throughput is roughly 50–100 Mbit/s shared. The server measures the actual session throughput and decides direct play, remux, or transcode from the measurement, and shows the client's link quality and chosen mode |
| Wi-Fi power save causes periodic stalls; roaming and mesh handoffs drop sessions | Sessions are resumable by construction (HTTP Range plus HLS), progress is written on disconnect, and the app inhibits sleep while serving, as it already does while playing and downloading |
| The best topology is hybrid | Documented plainly: **server on Ethernet where possible, clients on Wi-Fi**. A router bridges the two at layer 2, so nothing in our design has to change for it to be the recommended setup |

### 2. Export first (A, B, D), API second (C), client mode alongside (E)

- **v1.3 — Sharing.** A "Share with Jellyfin and Plex" feature that materialises a scan-friendly tree: their naming convention (`Movie Name (2005) [imdbid-tt…]`, `Season 01/S01E01`), NFO sidecars from our already-normalized metadata, local artwork at the filenames they read (`poster.jpg`, `backdrop.jpg`, `logo.png`, `S01E01-thumb.jpg`), and external subtitle suffixes (`.default.en.srt`, `.forced.en.ass`, `.en.sdh.srt`). Files are **hardlinked** when the share is on the same filesystem as the media, copied otherwise, with the disk cost stated before the user commits. Optionally `.disc` placeholders for monitored titles that are not on disk yet.
- **v1.3 — DLNA server.** SSDP announce, `ContentDirectory` Browse with DIDL-Lite, and our existing range-capable HTTP server underneath. This reuses the casting work rather than duplicating it, and covers the TV and console cases with no client software.
- **v1.4 — Jellyfin-compatible API.** A documented subset: `System/Info`, authentication, `Items` browse and detail, `Items/{id}/PlaybackInfo`, image endpoints, and the video/audio/subtitle stream endpoints with Range. Enough for real clients to browse and play. **Not** full parity: no Live TV, no plugins, no user administration, no sync/transcode fanciness beyond what clients need to press play.
- **v1.5 — Client mode (E).** Consume a Jellyfin or Plex library as a source alongside the local library and the catalogue, with playback through mpv and progress written back. This is deliberately last only because it is independent; it could be pulled earlier if the demand is higher than expected.

### 3. A server needs a headless mode, and that is a real product surface

A server that only runs while a window is open is not a server. Phase 7 therefore includes a `peregrine-server` mode: the same core, no GUI, installed as a launchd agent, a systemd unit, or a Windows service, with a small local config and a status endpoint. `.disc` placeholders and the Jellyfin API both depend on this, because a client that connects at 23:00 must not find the library offline.

Headless mode inherits the full core, which is why it is scheduled after 1.0: it depends on the storage, scheduler, monitor, and streaming layers being finished and stable.

### 4. Server security is a different posture, and the defaults say so

Every previous network feature was scoped and brief. A server is long-lived:

| Control | Rule |
| --- | --- |
| Exposure | **LAN only, decided.** The server binds private-range interfaces and never a WAN address; there is no remote-access mode. The documented remote path is a VPN the user already runs. Adding remote access later would need its own ADR, a TLS story, and a threat review — it is not a checkbox we add to this one |
| Identity | Per-device tokens issued at pairing, named, revocable, listed with last-seen. Optional PIN for shared networks. No password system: accounts map to server users, and the Jellyfin subset accepts a single user with a token |
| Transport | Plain HTTP on the LAN by default with a clear note; TLS via a reverse proxy is the documented path, and Jellyfin clients expect HTTPS for anything beyond the LAN |
| Surface | Read and stream only. No delete, no library writes, no settings changes, no filesystem paths in any response |
| Audit | Every authenticated request group is logged with device, action, and byte count, and the sharing screen shows who is connected and what they are playing |
| Rate and scope | Per-device request limits; a device can be restricted to specific libraries |

### 5. Transcoding on a server is capacity-constrained by our licence choice

[ADR 0014](./0014-ffmpeg-build-and-transcode.md) removed libx264 from our ffmpeg, so software H.264 encoding is available only through hardware encoders or `libopenh264`. That was a reasonable trade for a desktop player. For a **server** it is a real constraint: a headless host — a NAS, a mini PC, an old laptop — often has no usable hardware encoder, and a client on weak Wi-Fi or an old device may have no choice but to request H.264.

The honest consequences, which the UI must state rather than hide:

1. Direct play and remux are always available and cover most clients.
2. Hardware-accelerated transcode works where a GPU or iGPU encoder exists, and the capability is detected and shown per host.
3. Without hardware encode, `libopenh264` is the software fallback, with its patent caveat already recorded.
4. If neither exists, the server says *why* it cannot serve that client and offers direct play; it does not silently ship a GPL ffmpeg. A user who wants libx264 can point the server at their own ffmpeg build, exactly as on the desktop.

This is the strongest argument yet for revisiting ADR 0014 for the server build specifically, and it should be re-examined at the start of v1.4 rather than assumed.

### 6. What we refuse

| Refused | Reason |
| --- | --- |
| Impersonating a Plex Media Server so Plex clients connect | Plex clients discover and authenticate through Plex's own infrastructure; a foreign server cannot serve them, and faking it violates their terms and breaks without warning |
| Pushing media *into* Jellyfin or Plex | Not a capability either product offers. The supported equivalents are the folder export and placeholders in §2 |
| Full Jellyfin API parity | Hundreds of endpoints, most serving features we do not have. The subset in §2 is what clients need to browse and play |
| WAN exposure in the server mode | Decided out of scope (2026-10-09). A listener that outlives the session and is reachable from the internet is a different product with multi-user accounts, quotas, and a much larger security burden. LAN only, VPN for remote |
| Becoming a multi-tenant media service | Users, quotas, invite links, and remote streaming are a different product with a different security model. accounts plus per-device tokens are the limit |

### 7. Licensing note for the API work

Jellyfin is GPL-2.0. Implementing a compatible API is not copying code, but the work must be done from **published API documentation** (`api.jellyfin.org`) and observed client behaviour, and no Jellyfin source, schema file, or generated stub from their repository may be vendored into ours. This is a Phase 7 gate, checked the way [ADR 0009](./0009-licensing-and-redistribution.md) checks sidecars.

## Consequences

Positive: the two cheapest integrations (folder export, DLNA) deliver "my downloads show up on the TV" without any protocol work and with both ecosystems; the API subset unlocks a large client ecosystem nobody has to build; Wi-Fi is treated as the normal case; and the security posture is explicit at a point where it usually is not.

Negative and accepted:

- **A server is a new threat surface that outlives the session.** It gets its own threat-model review, its own audit trail, and a LAN-only default that will frustrate users who want remote access — who should use a VPN.
- Transcoding capacity is genuinely limited by our licence choice, and some headless hosts will not be able to serve some clients. Documented, not papered over.
- Headless mode means service installation, autostart, log rotation, and upgrade paths on three operating systems — packaging work that has nothing to do with media.
- Export creates a second tree on disk and a synchronization problem: a moved or upgraded file must update the link and the NFO, or the other server shows a stale item. Addressed by treating the share as a job with a verifiable manifest, not a one-off script.
- Two ecosystems means two sets of naming quirks. They share the Kodi conventions, which is why one writer covers both, but the differences will still produce bug reports.

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| Only build the Jellyfin API and skip export | Largest work, and it leaves Plex users with nothing. Export is days, not months |
| Only build export | Gives up the client ecosystem and leaves the "stream to my phone" case unserved |
| Make Peregrine a Jellyfin *plugin* | Plugins run inside their server and could only contribute metadata, not streaming; also drags us into C#/`.NET` and their release cycle |
| Bind to `0.0.0.0` and advertise every address | Advertises the VPN and container bridges to the LAN, and produces the classic "which of my five IPs is it on" support burden |
| Rely on SSDP for discovery because DLNA needs it | SSDP over Wi-Fi is the least reliable part of the stack; mDNS plus a QR code is the dependable path, with SSDP as best effort |
| Open remote access as a convenience feature | Security cost with no safe default. VPN, or an explicit opt-in with a warning |

## Task breakdown

v1.3 is broken into twelve tasks as E32 in [TASKS.md](../TASKS.md), measured at ~39 ideal days. E33 (Jellyfin API and headless service, ~30 days) and E34 (client mode, ~21 days) stay scoped-but-unbroken-down until v1.3 lands, because `SPIKE-15` will change their shape.

## Verification

- `SPIKE-14` (2 days): stand up the export against a real Jellyfin and a real Plex install, and confirm items appear with correct metadata, artwork, and subtitle tracks without manual matching.
- `SPIKE-15` (3 days): connect the Jellyfin API subset to two real clients (one mobile, one TV) and record what each requires beyond the subset — this defines the true v1.4 scope.
- `SPIKE-16` (1 day): measure a 4K remux over 2.4 GHz, 5 GHz, and Ethernet, and record where direct play stops being viable, so the link-budget thresholds are measured rather than assumed.
- Prior to v1.4, re-examine [ADR 0014](./0014-ffmpeg-build-and-transcode.md) for the server build, with the hardware-encoder survey as its input.
