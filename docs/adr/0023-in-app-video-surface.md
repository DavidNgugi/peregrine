# ADR 0023 — The in-app video surface, and the loopback delivery it needs

- **Status:** Accepted. Supersedes ADR [0004](./0004-mpv-out-of-process.md) §Decision on where the video surface lives, and ADR [0013](./0013-mpv-distribution-and-video-surface.md) §Decision 2 (the per-platform surface strategy) **for local files in the initial slice**. ADR 0013 §Decision 1 — ship upstream's GPL mpv as a separate executable, driven over JSON IPC — stands untouched, as do 0004's licence reasoning and 0013's rules about the WebView and mpv being siblings.
- **Date:** 2026-10-15
- **Evidence:** measured on this checkout and this machine, recorded in §Context; `apps/desktop/src-tauri/tauri.conf.json`; `docs/ARCHITECTURE.md` §5 and §17; SECURITY.md T10; `crates/peregrine-platform/src/sidecar.rs`
- **Related:** [0004](./0004-mpv-out-of-process.md), [0009](./0009-licensing-and-redistribution.md), [0012](./0012-streaming-readahead.md), [0013](./0013-mpv-distribution-and-video-surface.md), [0022](./0022-component-packs-and-sidecar-verification.md)

## Context

The design on record is that playback is mpv's job, with a surface per platform: `--wid` embedding on Windows and X11, a positioned borderless child window on macOS, and a degraded hand-off on Wayland (ADR 0013 §2). Two measured facts make that design unavailable today, and a third makes a different one available.

**Fact 1: there is no mpv, and no way to get one.** `which mpv` answers nothing on this machine, and `which ffmpeg` and `which ffprobe` answer nothing either. Those binaries are declared as sidecars in `sidecars.toml` and are meant to arrive as component packs, but the pack downloader and verification are `PG-E26-01` and do not exist. So today the shipping build cannot start a player of any kind, and the title screen's Play control is disabled with `noPlayer` as its reason (`apps/desktop/src/lib/play-action.ts`) — which is honest, and is what a reader reported as "we still can't play".

**Fact 2: the library a real user has is mostly one codec pair.** Ten files are indexed in this installation's database, under one root. `mdls -name kMDItemCodecs` answers `H.264` + `MPEG-4 AAC` for the nine MP4s. The tenth is Matroska, and its header carries `V_MPEGH/ISO/HEVC` and `A_AAC`: HEVC in an MKV, which is the one shape a WebKit video element cannot demux.

**Fact 3: the webview already has a decoder, and it is native.** WKWebView decodes H.264/AAC in the platform's own media stack. So a `<video>` element pointed at the bytes plays nine of those ten files with **no sidecar, no download, and no new decoding code in our process** — on the machine the user actually has.

That is the whole of the argument. The choice is between a player that cannot ship yet and a player that can ship today for the codecs a media library is mostly made of, and the second one does not foreclose the first.

**And the delivery had to be designed before the surface could be.** `docs/ARCHITECTURE.md` §17 already decides that media bytes travel over the asset protocol *or* the loopback HTTP server, and §626 already refuses the asset protocol for the library root: only `<OS cache directory>/images` is ever scoped, so a library file is deliberately unreachable through it. `peregrine-stream`'s row in §5 already names an axum router serving byte-range media with token auth and a per-session lifetime. What did not exist was any of it: `crates/peregrine-stream/src/lib.rs` was a seven-line module doc, and `arch-allowlist.toml` had already permitted `axum`, `hyper`, `tower`, and `tower-http` for it. So the server is not a new decision either — it is the first implementation of a recorded one. This ADR records only what the *surface* decision changes.

## Decision

### 1. The video surface is inside the webview, and the bytes arrive over loopback HTTP

`peregrine-stream` runs an axum server bound to `127.0.0.1:0` — one listener **per playback session**, so the socket's lifetime *is* the session's, which is what SECURITY.md T10 asks for. It serves exactly one file, chosen when the session opens, with byte-range support so seeking works. The renderer plays it in a `<video>` element. No media code is linked, and nothing is decoded in our process.

The alternative was the asset protocol with the library root scoped. That is refused, and the refusal is deliberate rather than incidental: `ARCHITECTURE` §626 says the library root is never exposed, and a protocol that serves arbitrary files by path is a much larger surface than a loopback route that serves the one file a session was opened for.

### 2. The session token travels in the request path

A `<video>` element cannot set an `Authorization` header, so a bearer token in a header is not available to this client at all. The token is therefore a path segment: `GET /media/{token}`.

**This is the one compromise in the design, and it is bounded on purpose.** A token in a URL is normally a leak: it reaches `Referer`, proxy logs, and browser history. None of those exist here — the client is a media element inside a webview whose CSP forbids every remote origin, the response is `Cache-Control: no-store`, the route serves one file, the listener is bound to loopback, and the socket closes when the session ends. The compensating controls are exactly the four T10 names, and the token's job is to stop **a web page** — the DNS-rebinding half of T10 — not a local process running as the same user, which can already read the file directly. Saying that plainly matters, because a control whose threat is mis-stated gets weakened for the wrong reason.

`Host` is validated against the loopback forms on every request, which is the half that actually refuses a rebinding request.

### 3. The token is minted behind a port, in `peregrine-platform`

`cargo xtask check-determinism` permits a random source in `peregrine-platform` and nowhere else, so the token is produced there through a new `TokenSource` port defined in `peregrine-domain` (`Clock` and `IdGenerator` are the existing two). Nothing mints a token inline, and every test injects a fixed one.

A ULID is deliberately not reused for this: `IdGenerator` is ULID-shaped and wall-clock-prefixed, and a token that leaks the second it was minted is a worse token for no saving.

### 4. mpv stays the backend for what the webview cannot do

This is a **first implementation of the playback surface, not a replacement for the player**. Playback that the webview cannot perform stays mpv's, and the list is long and specific: Matroska, which is what the tenth file in this very library is; HEVC in MKV; HDR tone mapping; embedded subtitle tracks; audio passthrough and device selection; chapter and track selection; frame-accurate seek; hardware-decode selection; and anything arriving from a torrent stream.

The seam that keeps both is `PG-E16-02`'s `PlayerBackend`, which is not written yet and is the reason this ADR does not need to name a winner. The UI is written against **the session and the URL**, never against the element: what the renderer knows is "there is a session, here is its URL, here are its duration and position". When the mpv adapter lands it answers the same question with a surface instead of a URL, and the screen does not change.

**The consequence we accept, stated rather than discovered:** a file whose container the webview cannot demux fails to play, and the only honest thing the screen can do is say so. §5 below is where that sentence comes from — the core decides playability from the container, not the renderer's error event, so the page can say "this build cannot play this container yet" *before* pressing Play rather than after.

### 5. Container support is the core's answer, not the renderer's guess

`media_file.container` is a stored column. The runtime classifies it into one of three answers — `Playable`, `NeedsMpv`, `Unknown` — and the title page draws Play, Play-disabled-with-a-reason, or Play-with-a-caveat from that. The renderer never infers it from an `error` event, because an element that failed reports a code, not a reason, and "it did not work" is not something a person can act on.

### 6. The renderer captures the poster frame, and posts it back to the same listener

A local library has titles no provider has described — a folder of MP4s scans into stubs — and a grid of
grey rectangles is an honest answer that reads as a broken one. The film is right there, and the only
decoder this build has is the one behind the `<video>` element §1 decided on, so the frame is captured
**in the renderer**: drawn onto a canvas, encoded as a JPEG, and `POST`ed to the session's own listener,
which hands the bytes to a `ThumbnailSink` the composition root implements over the image cache.

Three consequences follow, and each is a decision rather than a detail.

**The canvas needs a CORS-readable response.** A frame drawn from a cross-origin video taints the canvas,
and a tainted canvas refuses `toBlob` — so without `Access-Control-Allow-Origin` on the media response
there is no poster at all. This ADR's earlier draft listed "no CORS headers" among what is deliberately
absent; exactly one policy now exists, for this, and it is scoped to the session's own listener. It is
safe for the reason the token is: the request must already carry a per-session credential in its path, and
a page without one is refused by the guard before CORS is consulted.

**The key is the title, the variant is `local`, and the provider slot reads `local`.** `ImageVariant`
gains a fourth member that no provider serves, `ImageKey` gains a second constructor, and the cache's
provider column holds the word `local` for these rows. The alternative — a second cache, or a new
`Provider` variant — would be a second thing to evict, account, and scope; and `Provider` is the closed set
of metadata services, which a frame this machine made is not.

**A capture never overwrites a poster a provider drew.** The core decides whether one is wanted, from the
catalogue row and the cache together, and answers `needs_poster` on the session. A renderer that captured
on every play would spend a decode of the film to replace real artwork with a film still — and the
replacement would only become visible later, when the provider's blob was evicted, which is the worst kind
of bug to find.

**Two callers, one operation.** The player captures a frame while it plays, and the library screen
backfills posters for the titles its read says have none — because a freshly scanned folder is exactly the
state in which a grid is a wall of grey rectangles, and telling a reader to open each film and press Play is
not a plan. Both go through the same pair of commands (`start_poster_capture` / `stop_poster_capture` for
the backfill, `start_playback` for the player), the same sink, and the same `needs_poster` rule; the backfill
runs **one title at a time**, because each capture decodes a video and a hundred at once would pin the
machine the reader is sitting at.

**A capture session is not a playback session, and they hold separate slots.** A backfill runs while a
reader may be watching something else, so a shared slot would mean opening a backfill session stopped the
film. The command names are different for the same reason: a reader who found `start_playback` called by a
poster backfill could not tell whether the film was meant to be playing.

**What this is not: a poster as good as a provider's.** A frame is 16:9 and a poster box is 2:3, so the
tile crops it hard; a provider's poster is composed for that box and this is a still from a film. It is a
better answer than nothing and a worse one than artwork, which is exactly why it is the fallback and why
`needs_poster` exists.

### 7. CSP gains exactly one origin shape

`media-src` and `connect-src` gain `http://127.0.0.1:*`. The port wildcard is required because the listener binds port 0, which is what makes "one socket per session" possible. `script-src` stays `'self'` and no remote origin is added anywhere, so this does not create a path for content to arrive from the network; it lets the document fetch from a server this process started.

## Consequences

Positive:

- Playback works today for H.264/AAC, and for any other container the platform's own media stack handles, with no sidecar, no download, and no packaging change.
- No media code is linked and nothing is decoded in our process, so nothing about the licence position in ADR 0009 changes.
- Seeking works, because range delivery is implemented once in `peregrine-stream` and is what a torrent stream will also need (`PG-E16-01`'s `MediaSource` covers both).
- The Wayland problem ADR 0013 could not solve — no `--wid`, no cross-client embedding, no client-side positioning — does not exist for a surface the webview draws.
- The screen can state *why* a file will not play instead of showing a black rectangle.

Negative and accepted:

- **MacOS and Windows only in practice for what is playable today.** WebKitGTK's codec set is whatever GStreamer plugins the distribution ships, so Linux playback is "sometimes", and the honest answer there is the same sentence §5 produces: needs mpv.
- The token is in a URL. Bounded as §2 describes, but it is a real concession and it is recorded rather than glossed.
- `media-src http://127.0.0.1:*` lets the webview reach any local HTTP server on any port. The renderer is ours, `script-src` is `'self'`, and no remote content is loaded, so this is not reachable by an attacker-controlled page — but it is a wider grant than the previous policy and it is the reason that policy changed.
- The player users get first is an HTML video element: its controls are the browser's, its keyboard handling is not ours, and it has no next-up, no track menu, and no hover scrub. `PG-E16-07` and `PG-E16-08` own those, and they are written against the session rather than the element.
- Two surfaces can exist at once (a webview session and later an mpv one), so `PlayerBackend`'s session handoff is now a requirement rather than a nicety. `PG-E16-14` already owns it.

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| Wait for mpv and the component packs, and play nothing until then | The surface is not what PG-E26-01 is blocked on, and a media app that cannot play a file it has indexed is not a smaller version of the product — it is a different one |
| Scope the library root onto the asset protocol and point `<video>` at `asset:` | ARCHITECTURE §626 refuses it explicitly, and it would turn a cache-scoped read-only protocol into a general file-read-by-path surface for the whole webview |
| Hand-roll HTTP/1.1 over `TcpListener` to avoid a dependency | Range parsing, `Host` validation, and header handling are where the bugs are; `axum` is already the recorded choice for this crate and already permitted by `arch-allowlist.toml`. A dependency that the architecture names is not an added dependency |
| Transcode everything to H.264/AAC with ffmpeg first | ffmpeg is not installed either, and it would put a transcode in front of every play for a codec pair most of a real library already is |
| libmpv in-process through a `dlopen` render API | Needs `unsafe` FFI, which AGENTS R9 forbids without its own ADR, and per-platform GL surface code — ADR 0013 already rejected it for the Wayland case and nothing here reopens it |
| An `Authorization` header instead of a path segment | Not available to a `<video>` element, which is the client |

## Verification

Measured, on this machine and this checkout:

1. The library's ten indexed files exist, and their codecs are as §Context says: `mdls -name kMDItemCodecs` for the nine MP4s, and `strings` over the Matroska header for the tenth (`V_MPEGH/ISO/HEVC`, `A_AAC`).
2. `which mpv`, `which ffmpeg`, and `which ffprobe` all answer nothing, and no manifest declares `librqbit` or a sidecar binary, so "a player of any kind is unavailable" is a fact rather than a reading.
3. `arch-allowlist.toml` permits `axum`, `hyper`, `tower`, and `tower-http` for `peregrine-stream`, so the server adds no permitted edge.
4. The range server's own behaviour is asserted in `peregrine-stream`'s tests against a real loopback socket: a full read, a single range, a suffix range, a range past the end answered `416`, an unsatisfiable `Host`, a missing token, a wrong token, a `HEAD`, and `no-store` on every response.

**Not verified either, and it is the second thing this ADR's central claim rests on:** that the capture
works end to end — a frame drawn from the stream, encoded, posted, stored, and drawn on a tile. The canvas
half needs a DOM this harness does not have, so what is tested is the arithmetic (`frameSeekMs`), the route,
the sink's key, and the reads that answer a poster; the capture itself is owed to the same manual run.

**Not verified, and it is the one thing this ADR's central claim rests on:** that a WKWebView `<video>` element actually plays these files. The harness cannot drive the Tauri window, so playback was not observed — the server, the URL, and the session are tested, and the element is not. What would settle it: `pnpm tauri dev`, opening a title whose file is one of the nine MP4s, and confirming a first frame. That is a manual step and it is owed, not done.

Also unverified: Windows and Linux behaviour, and every codec claim about a container other than the two this library actually contains.
