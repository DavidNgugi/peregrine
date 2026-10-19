# ADR 0031 — Browser client parity: viewer writes, subtitles, and language defaults

- **Status:** Accepted. Refines [ADR 0028](./0028-tv-web-client.md) §3 and §6, and [ADR 0029](./0029-device-scoped-acquisition.md)'s acquisition sentence.
- **Date:** 2026-10-20
- **Evidence:** `crates/peregrine-app/src/web`, `crates/peregrine-app/src/jobs/acquire.rs`, `crates/peregrine-domain/src/subtitle/format.rs`, `crates/peregrine-domain/src/language.rs`, and `crates/peregrine-storage/migrations/0029_playback_preference.sql`
- **Related:** [0002](./0002-rust-core-owns-state.md), [0019](./0019-local-accounts-and-parental-gating.md), [0023](./0023-in-app-video-surface.md), [0028](./0028-tv-web-client.md), [0029](./0029-device-scoped-acquisition.md)

## Context

ADR 0028 made the browser surface a viewer: read the catalogue, play what the desktop already acquired, and write progress. Its §3 said there was no write surface beyond progress, and §6 deferred subtitle tracks. That was the right first slice and the wrong finished product. A person on a television or a desktop browser could not add a title to My List, mark an episode watched, download a season, see a download's progress on the title page, choose subtitles, or let the next episode start. The player also had no way to prefer English audio or subtitles.

The request is parity with the desktop screens for the actions a viewer performs on a title, without turning the LAN surface into the IPC command set. Deletion, settings, library mutation, keyring access, and arbitrary filesystem reads must stay unreachable (R33, ADR 0028 §1).

## Decision

### 1. Viewer-state writes are part of the `read` scope

The watchlist and the watched marks are viewer state, like watch progress. They are written through the same `Runtime` methods the desktop uses, under the authenticated device's `read` scope:

- `POST /api/watchlist` `{ titleId, saved }` adds or removes one title.
- `POST /api/watched` `{ titleId, scope: "title" | "episode" | "season", season?, episode?, watched }` marks one title, one episode, or one season.

Progress already travelled this way. The rule is therefore narrower than "the surface is read-only": **a write that changes only what this account has seen or saved is allowed; a write that spends bandwidth, touches settings, deletes, or exposes a path is not.** Acquisition remains behind the separate `acquire` scope (ADR 0029). A stream-only token still cannot read or write either.

The server re-checks every input: the title must exist, an episode must belong to the title, and a season must be a positive number. A failure is a 4xx with a typed code, never a 500.

### 2. A season download is one request, many audited jobs

`POST /api/acquire/season` `{ titleId, season }` resolves the season's episodes, skips the ones already playable on disk, and asks `Runtime::start_acquire` for each remaining episode with the same quality profile, safety gate, disk floor, and seeding policy the desktop uses. Each accepted episode leaves a `web_audit` row naming the device, title, episode, and job before the queue is touched. One episode failing does not fail the request; the answer reports `queued`, `alreadyPlayable`, and `failed`.

### 3. Subtitles are converted to WebVTT at the edge

The browser's `<track>` parser reads WebVTT reliably and SRT/ASS inconsistently. The listener therefore serves a sidecar subtitle through

`GET /api/subtitle/{trackId}?titleId=..&episodeId=..&offsetMs=..`

as `text/vtt`. The route resolves the track only through the title and episode's playable file, uses the domain's existing `SubtitleDocument` parser and WebVTT writer, and refuses a bitmap format, a missing file, or an unparsable document with 404. It carries the device cookie and no path.

A transcode's HLS timeline starts at zero while the sidecar's timestamps are absolute in the file, so an HLS ticket passes the start offset and the route drops cues that end before it and clamps the rest. A direct ticket passes zero. The response is `no-store`.

The playback ticket carries the subtitle track metadata and URLs, and marks exactly one track as the default according to the account's preference (English by default). The player offers Off plus every track; the menu is a real control on a desktop browser, not a hidden platform affordance.

### 4. One playback preference row, used by the player and the acquire job

`playback_preference` is a singleton row: `audio_language`, `subtitle_language`, and `subtitles` (`auto`, `forced`, or `off`), defaulting to `en`, `en`, `auto`. The desktop Settings screen edits it; the web client reads it but cannot write it. It drives three things:

- **The default subtitle track** on a playback ticket: a language match, then the row's own `is_default`, then a lone sidecar tagged `und` (the only subtitle the file has), then none. `forced` narrows to forced tracks; `off` selects none.
- **The audio stream a transcode maps.** ffprobe reports each audio stream's language tag; the host maps the first stream whose tag matches the preference, then stream zero. The ticket reports the chosen language. A direct-play file is left to the browser's own audio-track list, which the player exposes when the engine offers more than one.
- **The release an acquisition prefers.** Release names are not reliable metadata, so this is a preference and not a filter: `peregrine-domain::language` ranks a candidate named for the preferred language (or MULTi/DUAL) first, an unnamed candidate next, and a candidate explicitly named for another language last. A release with no language token is still a candidate, and nothing is dropped.

### 5. The title read carries the state the screen must draw

`GET /api/title/{id}` answers `watchlisted`, `watched`, and each title's and episode's current download lifecycle state and percentage. The client re-reads the title while a download is queued or running, so a Download button becomes Queued, Downloading n%, or Downloaded without a refresh. The state is the core's; the client maps the lifecycle to a label and never invents one.

### 6. The client is a first-class screen, not a television afterthought

The same web client now:

- puts the top navigation and a back control on every screen except the player;
- scrolls a title detail view to the top when it is opened from anywhere;
- draws every rail, episode list, and More Like This section as a vertically flowing grid, with a Hide/Show control on desktop and mobile alike;
- reveals the same 450 ms, 1.3× hover preview the desktop tiles use on a pointer-fine device, with its metadata panel and no layout shift;
- autoplays the next playable episode when one ends, unless the person leaves the player first.

The next episode is resolved by the core, not guessed by the client: a ticket carries the next playable episode in the same season, else the first of a later season, else null.

## Consequences

Positive:

- A browser or television can do what a viewer actually does on a title: save it, mark it, download it, watch it with subtitles, and continue to the next episode.
- The subtitle path is the domain's parser and writer; no new subtitle implementation exists in TypeScript.
- The preference is one row, read by both clients and by acquisition, so "English by default" means one thing.

Negative and accepted:

- **A `read` token can now change watch state and the watchlist.** That is a wider blast radius than ADR 0028 §3 originally accepted; it is still bounded to one account's viewer state, and a leaked token still cannot delete, reconfigure, acquire, or read a path. A device that must be strictly read-only is a future scope, not this one.
- **A season download is a job per episode.** It is slower and uses more queue rows than a season-pack search; a season-pack acquisition path is a later optimisation, not a correctness gap.
- **Language detection from release names is a heuristic.** A MULTi release may carry no English track, and an unlabelled release may. The ranking improves the odds; it does not promise a track. The player's per-file audio menu and the subtitle menu are the honest controls.
- **A browser's audio-track API is inconsistent.** A direct-play MP4 may expose no usable audio-track list; the transcode path is where the preference is guaranteed to select the stream.
- **HLS subtitles are sidecars, not renditions in the playlist.** They are shifted by the session offset and served beside the video, which works for the sidecars the library indexes and does not require a second ffmpeg output.

## Verification

- The watchlist and watched routes refuse a foreign title, a missing episode, an unknown scope, and a stream-only token; the accepted write is read back through the title read.
- A season acquire queues one job per missing episode, skips a playable one, audits each accepted episode, and survives one failing episode.
- A subtitle route serves `text/vtt` for an SRT sidecar, refuses a track from another file, drops a cue wholly before the offset, and keeps a cue that spans it.
- The preference round-trips through storage; the title read defaults to the English subtitle track; a transcode with an English-tagged second audio stream maps that stream.
- The language matcher ranks a preferred-language release above an unlabelled one and an explicitly foreign one last, and its property test never panics.
- The web client's route, focus, and hover-preview modules keep their tests; the title page's download label and the player's next-episode selection are pure and tested.
