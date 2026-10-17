# Peregrine

**One desktop app for finding, downloading, and watching your own media.**

Peregrine does the job of a Prowlarr, Radarr, Sonarr, qBittorrent, and Jellyfin stack in a single window: search a title, pick a release, download it, play it. It is built for people who want the file on their own disk and their own server.

![The Discover screen, showing a featured title, its details, and a trending rail](assets/screenshots/discover.webp)

> **Status: early.** Discovery, acquisition, library scanning, and playback work end to end on macOS. Windows and Linux are untested, nothing is packaged, and subtitles, casting, and accounts do not exist yet. Treat this as a working prototype.

## What works today

- **Discover** — TMDB-backed rails, search, and title pages with seasons and episodes, cached in SQLite so screens open from disk.
- **Acquire** — search your indexers, rank releases against a quality profile, and download the one you pick. Peregrine can install and run a managed Prowlarr for you, or use one you already have.
- **Download queue** — live progress with pause, resume, and removal, and deletion that plans before it acts.
- **Play while it downloads** — a download is playable once its file list is known, served from the torrent engine through a loopback HTTP server.
- **Your library** — point it at folders you already have. Filenames are parsed and matched to titles, with manual matching for the ones it cannot place. Nothing is moved or renamed without asking.
- **Playback** — mpv with resume, continue watching, and an external-player route for containers the webview cannot demux.
- **Safety gate** — every torrent is inspected before a byte is written: path traversal, executables, symlinks, unicode spoofing, and size and file-count caps. Downloaded media is never opened with the OS handler.
- **Notifications** — an in-app centre, because OS delivery can fail silently.

## Roadmap

Subtitles, casting, watchlists with air dates, local accounts with parental gating, MCP access for agents, and a Jellyfin-compatible sharing mode so other clients can play from Peregrine. Packaging and signed releases come first. The full plan, with phases and exit criteria, is in [docs/PLAN.md](docs/PLAN.md).

## Scope

Peregrine hosts and indexes nothing. You bring your own indexers, providers, and files. It is not a streaming site, and it is not a headless server yet.

## Build from source

Prerequisites: Rust 1.95 or newer, Node 22 or newer, pnpm, and `mpv` on your `PATH` (`brew install mpv` on macOS). `ffprobe` is used for probing when present.

Nothing is packaged yet, so there is nothing to download and install.

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

## Documentation

| Document | Contents |
| --- | --- |
| [ARCHITECTURE.md](docs/ARCHITECTURE.md) | System design, module map, IPC contract, caching, concurrency, performance budgets |
| [DATA_MODEL.md](docs/DATA_MODEL.md) | Schema, indices, invariants, migrations, retention |
| [FLOWS.md](docs/FLOWS.md) | End-to-end sequence diagrams and state machines |
| [PLAN.md](docs/PLAN.md) | Phases, exit criteria, effort model, risk register |
| [TASKS.md](docs/TASKS.md) | The backlog: what each task still owes, and its acceptance criteria |
| [SECURITY.md](SECURITY.md) | Threat model, controls, accepted risks, reporting policy |
| [AGENTS.md](AGENTS.md) | The engineering contract, for human and AI contributors alike |

## Privacy

No telemetry, no accounts, no remote code. The app contacts the metadata and indexer services you configure, the BitTorrent swarm for torrents you add, and nothing else. Provider keys live in the OS keyring and are redacted from logs and diagnostics. See [SECURITY.md](SECURITY.md).

## Contributing

Read [AGENTS.md](AGENTS.md) first. Pick a task from [docs/TASKS.md](docs/TASKS.md), keep the pull request small, and keep the gates green. Commits follow the conventional format and carry a DCO sign-off.

## Licence

Dual-licensed under either **MIT** or **Apache-2.0**, at your option. Third-party components and bundled sidecars keep their own licences; see [DEPENDENCIES.md](docs/DEPENDENCIES.md) and the bundled `NOTICE` file.

The name "Peregrine" and the mark are the project's identity. Forks are welcome; builds that change behaviour should be renamed to avoid confusion about what is official.

## Acknowledgements

Built on [mpv](https://github.com/mpv-player/mpv), [FFmpeg](https://ffmpeg.org/), [librqbit](https://github.com/ikatson/rqbit), [Tauri](https://tauri.app/), and [TMDB](https://www.themoviedb.org/). Poster and backdrop images are provided by TMDB.

Architectural lessons, and specific mistakes to avoid, came from [qBittorrent](https://github.com/qbittorrent/qbittorrent), [seerr](https://github.com/seerr-team/seerr), [VLC](https://github.com/videolan/vlc), [Peeri](https://github.com/Aayush9029/Peeri), and [torlink](https://github.com/baairon/torlink); the licence analysis of each is in [docs/research/reference-codebases.md](docs/research/reference-codebases.md).

> This application uses TMDB and the TMDB APIs but is not endorsed, certified, or otherwise approved by TMDB.
