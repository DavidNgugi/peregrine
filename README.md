# Peregrine

**One desktop app for finding, downloading, and watching your own media.**

Peregrine does the job of a Prowlarr, Radarr, Sonarr, qBittorrent, and Jellyfin stack in a single window: search a title, pick a release, download it, play it. It is built for people who want the file on their own disk and their own server.

![The Discover screen, showing a featured title, its details, and a trending rail](assets/screenshots/discover.webp)

> **Status: v1.0 release candidate, unsigned.** Discovery, acquisition, library scanning, episode-level playback, watch state, the watchlist, and scheduled downloads work end to end on macOS. The release workflow builds macOS, Windows, and Linux bundles, but they are unsigned, unnotarised, and unpublished; Windows and Linux have not been run. Subtitle search/download, mpv subtitle selection, casting to a device, and accounts still do not exist. Treat it as a release candidate, not a supported release.

## What works today

- **Discover** — TMDB-backed rails, search, and title pages with seasons and episodes, cached in SQLite so screens open from disk.
- **Acquire** — search your indexers, rank releases against a quality profile, and download the one you pick. Peregrine can install and run a managed Prowlarr for you, or use one you already have.
- **Download queue** — live progress with pause, resume, and removal, and deletion that plans before it acts.
- **Play while it downloads** — a download is playable once its file list is known, served from the torrent engine through a loopback HTTP server.
- **Your library** — point it at folders you already have. Filenames are parsed and matched to titles, with manual matching for the ones it cannot place. Scanned titles get their own **My library** rail on Discover, and the Library screen searches and filters them and states the storage they use. A scanned title still carrying no provider id is matched automatically in the background: the app searches the configured provider by name and kind on its next pass, then fills in the poster and metadata. Nothing is moved or renamed without asking.
- **TV web client** — Settings → Web access starts a surface on this network, for a television or a phone. Open the address it shows in a television's browser, compare the code on the TV with the one on the desktop, and approve it; the approval is read-only unless you tick **Allow downloads**. The TV opens on a rotating hero and rails of Continue Watching, My List, your library, and your provider's lists; a series page lists its seasons and episodes as cards, asking the provider for them in the background when the catalogue only listed the title, and every title with provider data ends in an endless More Like This feed. Browse has the search field and the film/series, sort, and genre filters and pages itself as you scroll; My List is the same watchlist and can add or remove a title; Downloads shows the desktop queue's live progress. A device approved with **Allow downloads** also sees a Download button on a missing film, a missing episode, or a whole season, and a series with no episodes offers Download series; it asks the core to start the same automatic acquisition the desktop starts, and without that scope the title page offers no button and the server refuses the write. Nothing scrolls sideways: rails, episode lists, and More Like This are vertical grids on every screen, each with a Hide/Show control on desktop and phone alike. The top navigation and a Back control sit on every screen but the player; a title page always opens at the top; a pointer resting on a poster on a desktop browser gets the same 450 ms, 1.3× hover preview the desktop app uses; navigation sits in a bottom tab bar on a phone, and the player handles portrait and landscape. The address bar is the route: refreshing /title/16 returns to that title, and Back or an edge swipe follows the app's history. When a provider backdrop is missing it draws the poster as a sharp panel on desktop and a banner on a phone, rather than stretching a small poster across the hero. It plays **any container it holds** — a file the browser cannot demux is transcoded to HLS, hardware-accelerated where the host has an encoder. Progress is written to the same rows the desktop player writes, so a film started on the TV resumes on the desktop and the other way round. It cannot delete or change settings. Its writes are the viewer state the desktop also writes (progress, the watchlist, watched marks), an opt-in audited acquisition, and a download the same quality and safety rules govern. It reads the audio and subtitle language preference from the desktop's Settings, starts on that subtitle track, and autoplays the next playable episode when one ends; sidecar SRT/ASS subtitles are converted to WebVTT so a desktop browser can switch them or turn them off. It is off until you start it, and it stops when the app closes.
- **Hardware-accelerated transcoding** — the HLS transcode behind the web client uses the host's H.264 encoder first: VideoToolbox on macOS, NVENC, Quick Sync, or AMF on a machine that has one. Where the build carries it, the fallback is the software `libopenh264`; `libx264` is never used, because the ffmpeg Peregrine ships is LGPL. A host with no permitted encoder answers a named refusal rather than starting a stream it cannot finish.
- **Usenet** — Settings → Usenet accepts a provider and keeps its password in the operating system's keychain. A Usenet release returned by Prowlarr downloads over NNTP instead of a swarm, appears in the same queue, and imports a direct-media post like a torrent. A RAR/par2 set needs the archive tools; without them the job fails with a named message rather than importing nothing. Peregrine connects directly to the provider you configure and is not anonymity software; it manages no VPN.
- **Cast** — each refreshed title carries its top-billed cast from TMDB, portraits included, drawn as a rail on the title page. Clicking a person opens their page: the titles in your library that credit them, plus the rest of their credits from TMDB.
- **Watchlist** — save a title from its page. A bookmark marks it on every rail and grid, and Discover's **My list** rail collects the saved titles.
- **Where to watch** — a title page carries the services streaming, renting, or selling it in your region, from TMDB's JustWatch data. The marks are all one size, one per service rather than one per subscription tier, subscription first, and the region is named. A title nobody has asked about draws no section rather than an empty one.
- **Playback** — pick an episode and play that episode, not the title's first file. mpv for containers the webview cannot demux, resume, and continue watching that reopens the exact episode.
- **Skip Intro and next-up** — local chapters, TheIntroDB, and AniSkip can answer with intro, recap, credits, and preview windows. A Skip Intro/Recap button appears only while a marker is on screen, and a credits window or the end of an episode opens a ten-second next-episode countdown with Play now and Cancel. A title with no marker has no button; a film ends on a panel with Replay and Back. The desktop and the TV/web player draw the same controls over their own engine. The external-mpv hand-off still opens mpv's own window and has no React overlay; that is the embedded-player task, not a hidden claim.
- **Integrations** — one Settings screen enables and orders TMDB, TheIntroDB, AniSkip, and local chapters, tests a connection, and stores each credential field in the OS keychain. TheIntroDB and AniSkip need no key to read; TheIntroDB's key is for submissions only. The TV/web client shows which sources are configured and which need attention and says to manage credentials on the desktop. OpenSubtitles, Subdl, IntroHater, and Trakt are not built yet. A marker can be suppressed locally through storage and resolution, but the settings control for that is still on the roadmap.
- **Subtitles** — `.srt/.vtt/.ass/.ssa/.sub` sidecars beside a video are indexed and offered in the webview player, served from the same session token as the media, and the selected sidecar is handed to mpv too. The desktop webview plays WebVTT directly; the browser client converts an SRT/ASS/SSA sidecar to WebVTT as it serves it, so the same track can be selected there. Provider search and download are not built.
- **Watch state** — a bar along each episode's still fills as you watch and completes when you finish; the first unfinished episode is marked **Next up**, and Settings carries the full viewing history. An episode, a whole season, or a film can be marked watched or unwatched by hand, and that mark outranks a later playback report.
- **Scheduled downloads** — an unreleased episode or title offers Schedule instead of Download; the app fetches it on its release date and the Scheduled list in Downloads cancels it.
- **Profiles** — the household's profiles, stored in the database and shared by the desktop and every paired browser. The library is shared; the watchlist, progress, history, and continue-watching belong to the profile you choose. Names, avatars, and kids profiles (which hide adult and unrated titles) travel with the account; the PIN/OS parental gate is still on the roadmap.
- **Safety gate** — every torrent is inspected before a byte is written: path traversal, executables, symlinks, unicode spoofing, and size and file-count caps. Downloaded media is never opened with the OS handler.
- **Notifications** — an in-app centre, because OS delivery can fail silently. A finished or failed download is recorded there with a link back to the queue; OS toasts stay off until consent has a place to live.
- **About** — Settings shows the version, the build target, and the provider notices this build is required to reproduce.

## Roadmap

Packaging and signed releases come first, then subtitles, casting to a device, air-date monitoring with watchlist auto-acquire, local accounts with parental gating (a parent curates a child's library by rating and by hand, and the child cannot download), MCP access for agents, and a Jellyfin-compatible API so other clients can play from Peregrine.

The browser client's first slice — the TV surface — is served by the desktop app today. A headless server with the same client, installable with Docker or on NAS platforms such as UmbrelOS, Unraid, and TrueNAS, is the next step. Once the core is stable, TV and mobile playback clients: Android TV, Tizen, iOS, and Android, in the way Jellyfin and Plex offer theirs. The full plan, with phases and exit criteria, is in [docs/PLAN.md](docs/PLAN.md).

## Scope

Peregrine hosts and distributes nothing. The indexers it searches are examples or sources you configure yourself, and what you download and keep is your responsibility. It is not a streaming site, and it is not a headless server yet.

## Why Peregrine

Self-hosted media is usually five services: an indexer manager, a film manager, a series manager, a torrent client, and a media server, each with its own configuration. Wrapper dashboards put one interface over all five. Peregrine replaces the path itself: the torrent engine, the library, and the player live in the app. The only external service is Prowlarr, which the app can install and run for you, or you can point it at one you already have.

| | Peregrine | *arr + Jellyfin | Wrapper dashboard | Streaming add-on |
| --- | --- | --- | --- | --- |
| What you install | one desktop app | five services | five services plus a UI | a player plus an add-on |
| Config surfaces | one setup flow, in progress | one per service | one per service | an add-on URL and an account |
| Torrent engine | in the app | a separate client | a separate client | external |
| Player | in the app | Jellyfin or another | Jellyfin or another | external |
| Play while downloading | yes | no | no | yes, without keeping the file |
| Keeps the file, works offline | yes | yes | yes | no |

The wins worth stating: a native app rather than a web console; one configuration surface rather than five; an engine and a player that can coordinate, which is what makes play-while-downloading possible; and an inspection before any byte is written.

The honest limits: Peregrine is an unsigned release candidate, verified only on macOS, and its bundles are neither signed nor notarised, while the stacks above have years of maturity. The first-run setup wizard exists, but there are no real accounts yet: profiles are local names and pictures, not scoped libraries. The bundled mpv and ffprobe remove the separate install on macOS and Windows; the Linux bundle still has no player.

## Build from source

Prerequisites: Rust 1.95 or newer, Node 22 or newer, and pnpm. The build fetches the pinned mpv and ffprobe sidecars itself (`pnpm sidecars:fetch`), so no separate mpv install is needed; a development build falls back to `PATH` when a sidecar has not been staged.

Release builds are compiled with a shared TMDB token supplied as `PEREGRINE_TMDB_TOKEN` at build time, so a fresh install can search without a key. The token is never in the repository. A key saved in Settings takes precedence, and a local build without the variable simply asks for one.

The release workflow produces four bundles — DMGs for both macOS architectures, an NSIS installer, and a deb plus an AppImage — and an unsigned v1.0 release candidate is published with a `SHA256SUMS` file. Signed releases wait for the signing secrets; the maintainer procedure is in [docs/RELEASE.md](docs/RELEASE.md).

```bash
pnpm install
pnpm tauri dev
```

The gates this repository is held to:

```bash
cargo xtask check-arch      # the dependency rules
cargo test --workspace      # the Rust suite
pnpm typecheck && pnpm lint && pnpm test   # the frontend
```

## How it is built

A Tauri v2 shell, a Rust core, and a React front end. The core owns all state and the UI projects it; every external system sits behind a trait so it can be replaced with a fake in tests. Design, module map, and rejected alternatives are in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and the [ADRs](docs/adr/).

## Using a VPN

Peregrine does not create or manage a VPN tunnel: run WireGuard, your VPN provider's app, or a router-level tunnel, and turn on its kill switch. Torrent and provider traffic follows the operating system's route, so a dropped tunnel can expose your address. Settings → Network checks the address the internet sees when you open it; binding torrent traffic to an interface or SOCKS5 proxy is still open (`PG-E10-11`).

## Documentation

| Document | Contents |
| --- | --- |
| [ARCHITECTURE.md](docs/ARCHITECTURE.md) | System design, module map, IPC contract, caching, concurrency, performance budgets |
| [DATA_MODEL.md](docs/DATA_MODEL.md) | Schema, indices, invariants, migrations, retention |
| [FLOWS.md](docs/FLOWS.md) | End-to-end sequence diagrams and state machines |
| [PLAN.md](docs/PLAN.md) | Phases, exit criteria, effort model, risk register |
| [TASKS.md](docs/TASKS.md) | The backlog: what each task still owes, and its acceptance criteria |
| [RELEASE.md](docs/RELEASE.md) | Cutting and verifying a release, the unsigned RC policy, and the signing secrets |
| [SECURITY.md](SECURITY.md) | Threat model, controls, accepted risks, reporting policy |
| [AGENTS.md](AGENTS.md) | The engineering contract, for human and AI contributors alike |

## Privacy

No telemetry, no accounts, no remote code. The app contacts the metadata and indexer services you configure, the BitTorrent swarm for torrents you add, and nothing else — except when you open **Settings → Network**, which makes one request to `api.ipify.org` to show your exit address and does not log the answer. Provider keys live in the OS keyring and are redacted from logs and diagnostics. See [SECURITY.md](SECURITY.md).

## Contributing

Read [AGENTS.md](AGENTS.md) first. Pick a task from [docs/TASKS.md](docs/TASKS.md), keep the pull request small, and keep the gates green. Commits follow the conventional format and carry a DCO sign-off.

## Licence

Dual-licensed under either **MIT** or **Apache-2.0**, at your option. Third-party components and bundled sidecars keep their own licences; see [DEPENDENCIES.md](docs/DEPENDENCIES.md) and the bundled `NOTICE` file.

The name "Peregrine" and the mark are the project's identity. Forks are welcome; builds that change behaviour should be renamed to avoid confusion about what is official.

## Acknowledgements

Built on [mpv](https://github.com/mpv-player/mpv), [FFmpeg](https://ffmpeg.org/), [librqbit](https://github.com/ikatson/rqbit), [Tauri](https://tauri.app/), and [TMDB](https://www.themoviedb.org/). Poster and backdrop images are provided by TMDB.

Architectural lessons, and specific mistakes to avoid, came from [qBittorrent](https://github.com/qbittorrent/qbittorrent), [seerr](https://github.com/seerr-team/seerr), [VLC](https://github.com/videolan/vlc), [Peeri](https://github.com/Aayush9029/Peeri), and [torlink](https://github.com/baairon/torlink); the licence analysis of each is in [docs/research/reference-codebases.md](docs/research/reference-codebases.md).

> This application uses TMDB and the TMDB APIs but is not endorsed, certified, or otherwise approved by TMDB.
