# Decision records

One file per decision that constrains the architecture. Each records the context, the decision, the consequences we accept, the alternatives it beat, and how it will be verified. Changing a decision requires a new record that supersedes it, not an edit to history.

| # | Decision | Status |
| --- | --- | --- |
| [0001](./0001-desktop-shell-tauri.md) | Desktop shell: Tauri v2 | Accepted |
| [0002](./0002-rust-core-owns-state.md) | The Rust core owns state; the UI is a projection | Accepted |
| [0003](./0003-sqlite-and-fts5.md) | SQLite with FTS5 for storage and search | Accepted |
| [0004](./0004-mpv-out-of-process.md) | mpv runs out of process, driven over JSON IPC | Accepted; build and surface superseded in part by 0013 |
| [0005](./0005-librqbit-torrent-engine.md) | librqbit in-process behind a `TorrentEngine` port | Accepted, contingent on SPIKE-3 |
| [0006](./0006-url-first-caching.md) | Cache URLs and identifiers, never bytes we can refetch | Accepted |
| [0007](./0007-metadata-sources.md) | Metadata sources: TMDB primary, IMDb datasets for ratings, no scraping | Accepted |
| [0008](./0008-incremental-polite-sync.md) | Incremental, polite sync; local-first with no accounts | Accepted |
| [0009](./0009-licensing-and-redistribution.md) | Licensing and redistribution boundaries | Accepted; §3 and §4 corrected by 0013 and 0014 |
| [0010](./0010-casting-scope.md) | Casting scope: Chromecast and DLNA, AirPlay out | Accepted |
| [0011](./0011-extension-model.md) | Extensibility by compile-time traits, not a plugin runtime | Accepted |
| [0012](./0012-streaming-readahead.md) | Streaming: piece-level scheduling keyed to the demuxer's read position | Accepted, contingent on SPIKE-3 |
| [0013](./0013-mpv-distribution-and-video-surface.md) | mpv distribution and the video surface per platform | Accepted |
| [0014](./0014-ffmpeg-build-and-transcode.md) | ffmpeg build configuration and the cast transcode strategy | Accepted |
| [0015](./0015-native-notifications.md) | Native notifications: delivery, sound, and permission honesty | Accepted |
| [0016](./0016-monitoring-and-availability.md) | Monitoring: watching upcoming releases and new episodes | Accepted, post-1.0 (Phase 6) |
| [0017](./0017-mcp-server.md) | MCP server: exposing the library to agent tools | Accepted, post-1.0 (Phase 6) |
| [0018](./0018-media-server.md) | Media server: what is possible, in what order, and what we refuse | Accepted, post-1.0 (Phase 7) |
| [0019](./0019-local-accounts-and-parental-gating.md) | Local accounts, avatars, and parental gating | Accepted, post-1.0 (Phase 8); the scope seam lands in 1.0 |
| [0020](./0020-homelab-deployment.md) | Homelab deployment: the headless daemon in a container, and what we refuse to put in one | Accepted, post-1.0 (Phase 9); the browser console in §3 is scoped, not broken down |

Open questions that could change a decision are tracked as spikes in [ARCHITECTURE.md](../ARCHITECTURE.md) §21. Each spike's outcome is written back into the relevant record here.
