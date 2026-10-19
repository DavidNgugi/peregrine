# ADR 0028 — TV web client: a paired LAN surface, read-only by default

- **Status:** Accepted. The first slice of E37 (the browser console) served by the desktop app, ahead of the E36 daemon that [ADR 0020](./0020-homelab-deployment.md) §3 assumed would host it. The acquisition sentence in §3 is superseded by [ADR 0029](./0029-device-scoped-acquisition.md); §3's read-only sentence and §6's subtitle omission are refined by [ADR 0031](./0031-browser-client-parity.md); every other boundary here stands.
- **Date:** 2026-10-19
- **Evidence:** the command surface and the event bus in this tree; the loopback media server in `crates/peregrine-stream`; [ADR 0018](./0018-media-server.md) §4 for the server posture, [ADR 0020](./0020-homelab-deployment.md) §3 for the console's scope, and [R33](../AGENTS.md) for the LAN rules
- **Related:** [0002](./0002-rust-core-owns-state.md), [0018](./0018-media-server.md), [0019](./0019-local-accounts-and-parental-gating.md), [0020](./0020-homelab-deployment.md), [0023](./0023-in-app-video-surface.md), [0029](./0029-device-scoped-acquisition.md)

## Context

The desktop app is a Tauri webview: every read and write in the UI goes through `invoke`, and the only HTTP listener is the player's media server, bound to `127.0.0.1` with a per-session token (`peregrine-stream`). Opening the UI in a television browser is not possible, and the architecture does not want it to be: [ADR 0018](./0018-media-server.md) §4 requires a server surface to be read-and-stream only, authenticated per device, and never a blind bind.

[ADR 0020](./0020-homelab-deployment.md) §3 already names the missing piece — a browser console (E37) — but sequences it after the headless daemon (E36) and leaves it unbroken-down. That sequencing is right for a homelab, and wrong for the actual request: a television in the same house, with a browser and no client app, while the desktop machine is on. Waiting for a daemon to serve a screen the desktop app can already answer leaves the most common living-room case unserved for a year.

The alternative shapes are worse:

| Shape | What it costs | Why it loses |
| --- | --- | --- |
| The desktop UI served as-is | A transport refactor plus a TV layout | Pointer-first screens on a D-pad; every command becomes a network endpoint, including delete and settings |
| A DLNA server only | A protocol stack; no browsing UI beyond the TV's file list | The TV's own player shows folders, not posters, synopsis, or resume |
| A Jellyfin-compatible server | The whole E33 subset | Correct long-term for *foreign* clients, but it does not produce a first-party TV experience and is months further out |
| Casting | E19's adapters and codec ladder | The right answer for a phone; a TV browser needs no pairing of a second device |

## Decision

### 1. A first-party web client, served by the desktop app, with a small API behind it

The change adds two surfaces and one client:

- A **LAN listener** in `peregrine-app` that is **off by default**, binds **one explicitly chosen address**, and stops when the app stops.
- A **deliberately small HTTP API** — its own DTOs and no generated parity with `peregrine-ipc`. Exposing the command set over the network would expose deletion, settings, and the keyring for free, which is the one thing [R33](../AGENTS.md) forbids. Acquisition is one opt-in scope and one route ([ADR 0029](./0029-device-scoped-acquisition.md)), not the command set.
- A **browser client** under `apps/web`, built as a ten-foot interface first: large type, few tiles per row, spatial focus for a D-pad, and a pointer path for the same screens. The same screens are touch-first on a phone — a two-to-three column grid, horizontal rails turned into vertical grids, collapsible rails and episode lists, a stacked title page, a persistent bottom tab bar for Home and Library, controls that wrap instead of overflowing, and video that letterboxes in portrait and fills the frame in landscape. The shape is the one a television audience already knows: a hero over horizontal rails of Continue Watching, the library, and the provider's lists; a title page with seasons and episode cards for a series, and an endless More Like This feed for anything the provider can name. Rails scroll horizontally by touch and by the D-pad, and a tile that is not held still opens a details page rather than pretending it can play. Every screen is an address (`/browse`, `/my-list`, `/title/16`), so a refresh restores the screen and the browser's Back and edge-swipe use real history; the player is the one transient route, and a refresh on it returns to the title rather than inventing a session. It shares `packages/theme`'s tokens. Its copy is local for this slice; moving it into the i18n catalogue is part of consolidating these screens with the console, and it does not share the Tauri-bound feature hooks.

The same client and the same API are what the daemon (E36) will serve later. This record does not create a second console; it lands the client early, against the desktop core.

### 2. Pairing is a two-screen proof, and the desktop is the authority

A code displayed only on the TV proves nothing: whoever can read the television can approve themselves. The ceremony therefore has two halves, and the desktop decides:

1. The TV opens the address and asks for a pairing: `POST /api/pair` with a display name. The server answers a `pairId` (128-bit, secret to that request) and an **eight-digit code**, valid for **five minutes**, single use, unique among pending requests.
2. The desktop shows the request — device name, address, and the code — in an in-app prompt **and** publishes `web://pair-requested`, so a notification surface can carry it. The person approves or denies. Approval can be by selecting the request or by typing the code; both resolve to the same pending row, and the code is what ties the prompt to the screen the person is looking at.
3. The TV polls `GET /api/pair/{pairId}` and receives the token **once** on approval. A denied or expired request never yields one. The approved answer also sets an `HttpOnly`, `SameSite=Strict` cookie scoped to this origin, which the browser alone carries: images and video are requested by elements that cannot attach an `Authorization` header, and a token in a URL would reach logs and history. A tool may present the same token as a bearer header instead.

The code is not the authorization; the desktop is. The code exists so a person cannot approve a request they are not looking at, and so a drive-by request from another machine is visible as "code 4821 9033" that no TV in the house is showing.

Controls on the ceremony:

| Threat | Control |
| --- | --- |
| Code guessing | Eight digits, five-minute TTL, one attempt to approve per pending request, max five pair requests per address per minute and a hard cap on pending requests |
| Approving the wrong device | The prompt names the address and the display name; the person compares the code with the screen |
| Replay of an approved code | A request is consumed on approval; `pairId` is never reusable |
| Token theft from the wire | Plain HTTP on the LAN is the accepted transport ([ADR 0018](./0018-media-server.md) §4); the token is revocable and named, and is read/stream unless its approval explicitly added acquisition. TLS stays the reverse-proxy path |
| Pairing loop after a denial | A denied request is terminal; the TV must start a new one, which produces a new prompt |

### 3. Device tokens are hashed, named, scoped, and revocable

- A token is 32 bytes of OS randomness, shown once as 64 lowercase hex characters.
- The database stores **`sha256(token)` only**, never the token; a stolen database is not a set of credentials.
- Each row is a device: a person-supplied name, a platform, the scope, timestamps, last address, and `revoked_at`.
- The default scope is `read,stream`. There is no write, delete, settings, keyring, or filesystem surface. Acquisition is a separate `acquire` scope granted only by the desktop's **Allow downloads** checkbox at pairing ([ADR 0029](./0029-device-scoped-acquisition.md)); a read-only token can watch films and read the library — and nothing else. The read side includes the download queue's progress.
- Revocation is immediate: every authenticated request hashes the presented token and reads the row, so a revoked device is refused on its next request, including one for a segment of a film.
- Last-seen is written at most once a minute per device, so a playing TV does not turn reads into a write per range request.

### 4. Playback is direct when it can be, and a transcode when it cannot

The core's own `LocalPlayback` rule decides delivery, and `ffprobe` refines it. A container the webview demuxes **and whose streams are ones a browser decodes** is served as bytes with `Range`; every other file — a Matroska film, or an MP4 carrying HEVC video or AC3 audio — is transcoded to HLS by an ffmpeg child process, so anything the library holds plays on the television as it does on the desktop. The encoder ladder is [ADR 0014](./0014-ffmpeg-build-and-transcode.md)'s: hardware H.264 first (VideoToolbox, NVENC, QSV, AMF), then `libopenh264`, and **never `libx264`**, because the ffmpeg this project redistributes is LGPL. A host with none of them answers a named refusal rather than starting a process that cannot finish.

The transcode is one session per device, four at most on the host, written under the data directory's `hls/` folder and killed and removed on stop, revoke, or expiry. It starts at the resume offset: the HLS timeline begins at zero and the client adds that offset back when it draws the clock and reports progress. **Seeking starts a new session at the target offset**, debounced, and preserves the ticket's episode id; a transcode cannot seek backwards and a forward seek would otherwise wait for the encoder to catch up, so a seek costs a second or two of startup rather than a stall. **Leaving the player stops the session** through `POST /api/playback/stop`, which kills the child and removes its directory; a start also clears directories a previous process left behind, because an orphaned transcode is the largest thing this surface writes.

**Progress is the same row the desktop writes.** `POST /api/progress` carries the title, the episode the playable file belongs to, the position, and the duration; it goes through `Runtime::record_progress`, so the percentage is the domain's (I6), a manual watch mark still outranks a report, and Continue Watching on the desktop and Resume on the television read one `watch_progress` row. The title read answers that position, so the TV offers Resume from where the desktop stopped and the reverse.

The home read is one authenticated request: a rotating hero, Continue Watching, My List, the library, and the provider's cached rails, so a screen draws from one answer rather than a request per row. Browse and search are two more reads over the same provider and local index: a kind, a sort, an optional genre, and a query. Browse asks for its next page as the reader nears the bottom, and the title feed pages its shared genres the same way, so neither ends in a Load more button. My List is the watchlist the desktop writes; this surface only reads it. A title read carries its seasons, the episodes of the selected season with their stills and per-episode progress, and the provider's similar titles. A series the catalogue only listed has no seasons yet; the read starts the same provider refresh the desktop starts, answers `refreshing`, and the client polls until the seasons and episodes land. An episode is addressed by season and episode number on the mint; the core resolves the row and the same file and transcode rules apply.

The URL carries a **short-lived playback token**, not the device token: the device token authorizes the mint, and each session token is bound to the device and an expiry. A copied media URL stops working when the session ends, and revoking the device also kills its sessions. HLS segments are addressed by the same session token and are refused when the device is revoked.

### 5. The listener is a posture, not a port

| Control | Rule |
| --- | --- |
| Default | Off. Nothing listens until a person enables the surface, and `R33`'s "no listener when disabled" is a test, not a comment |
| Bind | One explicit IP address, chosen from the detected LAN address or typed; the unspecified address (`0.0.0.0`/`::`) is refused, so a VPN or container bridge cannot be advertised by accident |
| Reach | LAN only. No UPnP port mapping, no WAN mode, no reverse proxy shipped; the documented remote path is the user's VPN |
| Host | Every request's `Host` must name the bound address (or a configured hostname) and port, which is what refuses a DNS-rebinding request from a browser on the same machine |
| Origin | A request carrying an `Origin` header must match the server's own origin; there is no CORS surface, so a foreign page cannot read an answer |
| Methods | `GET` for reads, `POST` only for pairing and playback mint; every other method is `405` |
| Bodies | A hard request-body cap, a header cap, and timeouts; pairing bodies are a name and a platform string |
| Images | Served by the cache key, validated as 64 hex characters, resolved inside the image cache and refused if the canonical path escapes it; never a filesystem path from the client |
| Files | Served by playback-token id, never by path; the id maps to a file the core resolved under its own guards |
| Audit | Pair requested, approved, denied, revoked, and authentication refused are structured log lines carrying the device id or address and the action, never the token or code. They are **not yet `event_log` rows**: the typed event vocabulary has no web variant, and writing a string outside it would break the vocabulary's one invariant. A typed web audit vocabulary is a named follow-up |
| Logs | Tokens and codes are never logged; an authentication failure logs the device name when it is known and the address otherwise |

The address is a property of the machine's current network, so it is followed rather than frozen. When the
bound address stops belonging to a local interface — a Wi-Fi switch assigning a new lease — the listener
opens the same port on the address the default route now names, closes the old socket, and pushes the new
status to the window. A listener still bound to an address the machine owns is left alone, so an explicitly
chosen interface is never second-guessed. The QR and the Settings row show the bound address, because it is
the one origin every browser on the network can open; a television's browser generally cannot resolve an
mDNS name. The platform's Bonjour name is offered beside it and accepted by the Host gate, and a device that
does resolve it keeps its pairing cookie across an address change, because the cookie is scoped to the
origin and the name outlives the address. That hostname is the machine's own Bonjour name, not an in-app
responder. The listener holds a system-sleep assertion while a device is streaming — released a minute after the
last media request — and while the surface is on if the person enables the keep-awake switch; the player
warns when the host's charge is below ten percent, because the stream dies with the machine.

### 6. What is deliberately not in this slice

- **Subtitle tracks in the transcode.** Sidecars are not yet mapped into the HLS playlist; the desktop player still offers them. That is `PG-E17`'s and E19's subtitle delivery task.
- **Hardware coverage beyond the ladder.** VAAPI is not configured (it needs a device and a filter chain), and a host with no permitted encoder is refused rather than falling back to a codec the licence does not allow.
- **Settings, acquisition, deletion, library mutation.** None of them exist on this surface by construction.
- **Accounts.** The listener reads under the installation scope ([ADR 0019](./0019-local-accounts-and-parental-gating.md), R34). Per-device account binding is E35's.
- **Serving the built client from the desktop bundle.** The first slice serves the client from a configured directory (and the Vite dev server on the LAN for development). Bundling the assets into the app is a packaging task, not a security one.
- **mDNS advertisement, short URLs, and the isolation probe.** QR pairing has landed, and the surface accepts one configured hostname with the desktop offering the machine's own Bonjour name. An in-app responder, short URLs, and the probe that names access-point isolation are [PG-E32-09](../TASKS.md)'s.

## Consequences

Positive:

- A television on the same network gets a poster grid and playback today, from the app the user already runs, with no second product to install.
- The API surface is small enough to audit. A leaked default token has a read/stream blast radius; a leaked `acquire` token can also spend bandwidth and disk, which is why that scope is opt-in, per-device, and audited ([ADR 0029](./0029-device-scoped-acquisition.md)).
- The client, the API, and the pairing model are the ones the daemon will serve later, so E37 does not restart from zero.

Negative and accepted:

- **The desktop app is not a server.** The TV stops when the app quits, the lid closes, or the machine is put to sleep; that is stated in the UI rather than discovered in the living room. While a device is streaming the surface holds a system-sleep assertion, and it holds one whenever the surface is on if the person turns on **Keep this computer awake** (which states the battery cost); the idle timer therefore does not end a film, but the daemon (E36) remains the answer for always-on.
- **A transcode costs the host.** One session per device and four at once is the bound; a seek restarts the encoder, so rapid scrubbing is a burst of short transcodes rather than a cheap seek, and a host without a hardware encoder pays for it in CPU.
- **Plain HTTP is the LAN default.** A token is revocable and read/stream by default (or also `acquire` when explicitly granted), but it is visible to anything that can sniff the network. This is the same posture ADR 0018 §4 accepted, and TLS remains the reverse-proxy path.
- **A second client is a second UI to maintain.** The mitigation is that it shares the theme, the i18n catalogue, and the API contract, and has no business rules of its own (R7).
- **Two DTO families.** The web DTOs are not the IPC DTOs, deliberately: parity would be a security bug. The cost is a mapping layer that must be kept in step, tested by a contract test over the JSON.
- **The TV browser is an unknown.** Its Chromium generation, HLS support, key codes, and storage behaviour are measured on the set before the layout is finalised; the client degrades to a pointer-friendly grid when the D-pad is absent.

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| Expose the Tauri IPC surface over HTTP | Every command becomes a network endpoint, including delete, settings, and acquisition; R33 forbids the surface, not just the bind |
| Bind `0.0.0.0` and filter by subnet | Advertises VPN and bridge interfaces, and makes the bind a policy the code cannot verify |
| Put the token in the media URL's query string | URLs reach logs, history, and screenshots; a per-session token bound to the device is the same convenience with a bounded life |
| Trust the code as the authorization because it is shown on the TV | Anyone who can see the TV can approve themselves; the desktop must be the authority |
| Store tokens in the keyring | Tokens are issued, listed, and revoked by the app; a hash in the database is the correct record, and the keyring is for secrets the app consumes, not for credentials it mints |
| Serve the desktop build's HTML and patch the transport | The screens are pointer-first and the transport rewrite is the same work with a worse security surface |

## Task breakdown

E37 in [TASKS.md](../TASKS.md) breaks this into ten tasks measured at 24 ideal days: the listener and its posture, pairing, tokens and storage, the read API, playback sessions, the HLS transcode and its seek-restart, progress write-back, the TV client, the desktop devices surface, and the security suite.

## Verification

- The security suite: no listener when disabled; an unspecified bind refused; a request with a foreign `Host` refused; an unauthenticated read refused; a revoked token refused on the next request and on a media range; an expired playback token refused; a code reused, guessed past the limit, or approved after expiry refused; an image key traversing out of the cache refused.
- The pairing round trip is driven end to end against a test listener: request, prompt, approve, token, authenticated read, revoke, refusal.
- The client is opened against a real TV browser and the findings recorded: direct-play support for the fixture files, D-pad key codes, focus behaviour, and whether the token survives standby.