# ADR 0034 — Samsung Tizen client: a shell over the paired LAN surface, signed per television

- **Status:** Accepted.
- **Date:** 2026-10-21
- **Evidence:** the paired LAN surface and cookie in [ADR 0028](./0028-tv-web-client.md) and `crates/peregrine-app/src/web`; the ten-foot client in `apps/web`; the Tizen findings in the reference television project and the verification section of [apps/tizen/README.md](../../apps/tizen/README.md)
- **Related:** [0018](./0018-media-server.md), [0028](./0028-tv-web-client.md), [0029](./0029-device-scoped-acquisition.md), [0031](./0031-browser-client-parity.md)

## Context

[ADR 0028](./0028-tv-web-client.md) already serves a paired, read-and-stream web client on the LAN. The living-room television is the case that surface exists for, and a television browser can open its address today. A first-party app adds the three things a browser tab cannot: an icon a person can find with the remote, a remembered address, and the Back and transport keys that belong to a television rather than to a web page.

The app cannot be a second client. It cannot read the desktop's pairing cookie, it cannot inject keys into a page on another origin, and a packaged copy of the client on the app's own origin would not receive that cookie at all. The hard constraints are the reference television's: no root, no Home takeover, no debloat, no boot or vconf changes, no certificate or token in git, Tizen 6.0 (Chromium 76) on a 1 GB-class set, plain HTTP on the LAN, and a desktop that must be running with Web access on.

## Discovery

The findings that shaped the decision, recorded here because they are measurements rather than a second design note. The full checklist is in [apps/tizen/README.md](../../apps/tizen/README.md).

- **The set.** `UA65AU8000UXKE`, model `21_NIKEL_UHD_BASIC`, a 3840x2160 panel; the reference app on this model reports a 1920x1080 viewport. Tizen 6.0 with Chromium 76 is the target, and the reference project measured the same engine on this model.
- **The viewport is served, not scripted.** The listener rewrites the client's one viewport meta tag to `width=1920, height=1080` for a Tizen User-Agent. A meta tag changed by script after the parser has read it does not resize the engine's canvas; the tag has to be in the document it receives. The Tizen bootstrap then adds the `tizen` class before first paint and probes flex `gap`, because the Tizen 6.0 engine predates it.
- **The URL is the whole integration.** A top-level `location.replace` makes the page, `/api/*`, `/img/*`, `/media/*` and `/hls/*` one origin. The listener's `Host` gate requires the typed host to be the bound address or the configured hostname, so the address a person enters is also the address the API will accept.
- **The cookie is set by the approval answer.** `GET /api/pair/{pairId}` answers `peregrine_token=...; HttpOnly; SameSite=Strict; Path=/; Max-Age=31536000`. The browser attaches it to every same-origin request; the page never reads it, and after a top-level navigation it is the same origin that was navigated to. There is no CORS surface to add.
- **Back is a DOM event with two spellings.** Tizen fires `tizenhwkey` with `keyName === "back"`; the reference set also reports keyCode 10009 for the same press. The client handles both with a short debounce and calls `preventDefault`, so Back on a title page returns to the screen behind it instead of closing the app.
- **Transport keys are key codes, not buttons.** Play 415, pause 19, play/pause 10252, stop 413, forward 417, rewind 412. They have no browser default and must be handled only while the player is mounted.
- **The engine is older than the client.** Chromium 76 cannot parse optional chaining or nullish coalescing, and it cannot substitute an `oklch()` custom property, `dvh`, `clamp()`, `min()`/`max()`, or `inset`. The build targets `chrome76`; the theme gains sRGB equivalents under a feature query; the critical layout values gain static fallbacks. **`aspect-ratio` is not cosmetic here**: without it a rail's poster container is zero-height, the absolutely positioned image is never laid out, and the lazy image never reaches its threshold. The feature query is not the only carrier: the Tizen class reserves the 2:3 and 16:9 boxes with padding and lets the absolutely positioned image stretch from its offsets, so a WebView that answers `@supports` differently still gets a tile. **The card art is a non-replaced flex item**, and this engine resolves a percentage width on it against an indefinite container, so the art shrank inside a full-width grid track while the Library/Downloads posters (direct, explicitly sized elements) worked; the Tizen rule stretches the art (`width:auto; align-self:stretch`) and fills the image from its offsets instead of percentages. Flex `gap` arrived in Chromium 84, so most of this engine's spacing gaps are cosmetic but collapsed; the bootstrap adds `no-flex-gap` only on engines that fail the probe (a later Tizen set supports it), and the downloads card uses grid tracks so its title, state, progress and stats do not depend on flex gap at all.
- **HLS is hls.js over MSE, with direct MP4 as the other path.** On the reference set a HEVC MKV opened an HLS session with `h264_videotoolbox` and wrote a progress row; the checklist records the rest.

## Decision

### 1. Phase 1 is a thin shell, not a client

`apps/tizen` is a Tizen web app with a setup screen and one navigation:

1. It reads the address from `localStorage`, then the build-time `config.js` default, and
   normalizes it to an `http://` or `https://` URL.
2. It shows the saved address for three seconds with "press BACK to change server", then calls
   `location.replace(url)`.
3. The Peregrine page owns pairing, Home, Browse, Search, title pages, playback and progress.

There is no iframe, no asset copy, no second backend, no CORS, and no bearer token. The shell's
own origin disappears from the session at the navigation, which is the point: the cookie, the
images and the media all belong to the listener's origin.

The cost is accepted: after navigation the setup screen is gone until the app restarts, so the
three-second Back window is the recovery path for a wrong address. The shell says so, and the
README repeats it.

### 2. The client owns the television-specific keys; the shell never injects them

The only Peregrine-side additions are the ones a remote needs and a browser page cannot express:

- Back (`tizenhwkey`, plus keyCode 10009 where the same press arrives as a key event) in
  `apps/web/src/tizen.ts`: player → close, inner screen → history, root → exit the Tizen app.
- Transport keys inside `Player.tsx`, so the listener exists only while a player is open.
- One floating focus plate for the remote, moved with `transform` and `opacity` and resized only
  when the target changes. The reference set measured a per-tile focus class at roughly 5 fps and
  a single moved plate at 60 fps; the client follows that recipe and does not draw the per-tile
  outline while the plate is present. Hidden pointer, no text selection, and the pairing code at
  96px sit alongside it.
- A ten-foot layout scoped to the Tizen class: six viewport-scaled poster columns, larger type, a
  top bar that scrolls instead of covering the Browse controls, hover lift and the pointer preview
  removed, entry controls autofocused (Home hero, Browse search, Title Play/Resume), and the first
  visible row eager-loaded so its posters do not wait behind the lazy threshold.
- A `chrome76` build target and sRGB/static CSS fallbacks, because the engine the client is
  navigated into is older than the syntax the client was written in.

No key is sent across origins. The page after navigation is the client, so the client handles its
own remote.

### 3. The app is a normal sideloaded app with a per-television identity

- Package `peregrine1` (ten characters; the reference TV rejects shorter package ids with a
  parsing error), application `peregrine1.Peregrine`, `required_version="6.0"`, TV profile,
  `internet` privilege, and `<access origin="*" subdomains="true">` for the LAN address.
- Every television gets its own Samsung certificate profile: the author certificate plus a
  distributor certificate carrying that set's DUID. Neither the profile nor the TV's address,
  token, or identifier is committed. The app is distributed as source with the per-user signing
  step, not as a signed build for a list of DUIDs.
- The app is not Home, is not a launcher, and does not try to become either. Uninstalling it is
  the complete undo, and it touches no Home routing, package, vconf, or boot state.
- **Supported Tizen floor: 6.0.** The target set is 6.0/Chromium 76; newer sets are expected to
  work because the app requests no newer API. Older sets are not supported, and the web client's
  build target would not run on them.

### 4. Phase 2 is gated on a written Phase 1 failure report

The packaged client with its own assets, a bearer token, a CORS allowlist, and a media-auth path
is not built now. It starts only if Phase 1 fails in a way the shell cannot fix: the cookie does
not survive an app restart, the remote page cannot be opened top-level, or the set cannot play
HLS at all. That report is a document in the change that triggers it, and it names which failure
was observed on which firmware.

### 5. The open questions, answered for this slice

| Question | Decision |
| --- | --- |
| Stable host or discovery? | A typed, stored address. No mDNS/SSDP discovery in Phase 1: a television usually cannot resolve a mDNS name, and the listener's host gate accepts one configured name only. A DHCP reservation is the documented answer; the Bonjour name is the stable fallback when the set can resolve it. A changed address is a new origin and a new pairing. |
| Read-only or downloads? | The shell adds no scope. The existing client offers downloads only when the pairing was approved with **Allow downloads** (`acquire`, [ADR 0029](./0029-device-scoped-acquisition.md)); leave the box unticked for a read-only television. |
| Tizen floor beyond 6.0? | 6.0 is the floor and the target. Nothing newer is required, and nothing older is claimed. |
| Source plus per-user signing, or a signed build per DUID? | Source plus per-user signing. A shared signed build would put one distributor certificate and one DUID list in the distribution path; it is not this project's model. |

## Consequences

Positive:

- The television reuses the client, the cookie model, the API, the HLS ladder and the progress
  rows the desktop already writes. There is no second UI to keep in step, and no new auth surface.
- The shell is small enough to audit in one sitting, and uninstalling it is the whole rollback.
- The client additions are testable where they live: URL normalization, Back/transport mapping,
  and the token fallback are unit tests, not television guesses.

Negative and accepted:

- The desktop is the server. The television stops when the app quits, the machine sleeps, or the
  address changes; the client and the README say so.
- After navigation the shell is gone. The three-second window is the only in-app recovery from a
  wrong address, and it is explained on screen.
- The client now carries a television target: the `chrome76` build target and the fallback blocks
  are part of the web client's contract, not a temporary patch. A future engine change is a
  deliberate change to them.
- Per-television signing is manual, and a firmware update can invalidate developer mode or the
  certificate. That is the price of sideloading.
- The app stores the server address in the app's own storage. It is not a secret and it is not in
  git; it is per installation.

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| An iframe around the Peregrine page | The cookie belongs to the iframe's origin and the parent cannot read it; media and full-screen behave differently; the shell would own focus in the wrong document. |
| Package the client's assets inside the Tizen app in Phase 1 | A different origin, so the pairing cookie is not sent; it needs a bearer token, CORS and a media-auth path, which is Phase 2 and needs its own failure report. |
| A second backend or a Tizen-specific API | It forks the core and duplicates ADR 0028 for no gain. |
| Send remote keys from the shell into the page | Cross-origin by construction; the client after navigation is the only document that can receive them. |
| A Home takeover or a launcher | Forbidden by the brief and the reference TV's findings, not reversible by uninstalling this app, and not this app's job. |
| A store listing | The target is a sideloaded, per-TV certificate; the store path is a different distribution product. |

## Verification

- Unit tests: `apps/web/src/tizen.test.ts` (Back target on every screen, hardware/back key codes,
  transport mapping, and the focus plate's inset geometry), `apps/tizen/test/server-url.test.mjs`
  (the address shapes a person can enter), and `packages/theme/src/tokens.test.ts` (an sRGB value
  for every semantic colour when OKLCH is unavailable).
- Build: `tizen package -t wgt -s <profile>` produces a signed `Peregrine.wgt`; the package
  contains only the runtime files.
- Headless Tizen-user-agent check: at 1920 and 3840, the six-column layout, a 418 px card art with
  a loaded 342 px poster, and the focus plate aligned to the focused control (4 px inset, 12 px
  card radius) were observed. This exercises the DOM and CSS path, not the set's compositor.
- Television: the checklist in `apps/tizen/README.md` records what was observed on
  `UA65AU8000UXKE` on 2026-10-21: install and launch, pairing, cookie across reinstall/relaunch,
  hero/rail posters after the card-art stretch and image-offset fix (confirmed on the reopened set),
  an HLS transcode (`h264_videotoolbox`) with a progress row, and a user-started HLS episode
  (18,315 ms of 3,176,107 ms) on the final build. The focus plate and entry autofocus are confirmed
  in use; pause/resume, player Back, root exit, the full D-pad walk, uninstall, and a 30-minute soak
  remain recorded there as not run or partial rather than claimed.
- Phase 1 is not called a success until that checklist's cookie-across-restart and HLS lines are
  observed, not assumed. The cookie and HLS-start lines passed; the remaining playback rows are
  still open.
