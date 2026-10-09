# ADR 0012 — Streaming: piece-level scheduling keyed to the demuxer's read position

- **Status:** Accepted, contingent on `SPIKE-3`
- **Date:** 2026-10-09
- **Related:** [0004](./0004-mpv-out-of-process.md), [0005](./0005-librqbit-torrent-engine.md)

## Context

"Play while downloading" is the feature that separates a torrent client with a media player attached from a media app. Research into the two reference torrent implementations produced a finding that shapes this decision: **neither implements true streaming.** The macOS reference hands finished files to the OS handler and explicitly documents that its piece priority is not strict sequential download; the TypeScript reference serves already-written files over HTTP Range, so an in-flight file has holes. Neither schedules pieces against what a demuxer is about to read, and neither does read-ahead.

The gap is broader than the two references we started from. qBittorrent — the most widely used client in this space — searches its entire source for `set_piece_deadline`, `piece_deadline`, or `add_piece` and returns **zero hits**. Its per-torrent controls are a libtorrent `sequential_download` flag and a self-computed first-and-last-piece priority vector; its "Preview" action refuses unless the file is already complete on disk and then hands the path to the operating system ([research/dependency-matrix.md](../research/dependency-matrix.md), [research/reference-codebases.md](../research/reference-codebases.md) §5). Deadline-driven readahead is not a refinement of the state of the art here; it is absent from it.

That leaves three architectural options for delivering bytes to the player:

| Option | How it works | Seek | Player compatibility | Complexity |
| --- | --- | --- | --- | --- |
| **A. Loopback HTTP server with Range** | Our server reads from the engine's file reader and answers byte-range requests | Native, instant, and the player drives it | Any player or cast renderer that speaks HTTP | Moderate; we own Range, 416, and backpressure semantics |
| **B. FUSE mount** | Present the downloading file as a filesystem path | Depends on FUSE behaviour for incomplete files; blocking reads stall the player's IO thread | Excellent, it is just a file | High, and FUSE on macOS and Windows is a real dependency and a driver install for users |
| **C. Pipe to the player's stdin** | Stream bytes in order | Impossible without restarting the stream | Poor; no seeking, no track probing, no casting | Low, but unacceptable |

## Decision

**Option A**, plus a scheduling policy that is the actual differentiator.

1. **A loopback HTTP server serves the media over byte ranges** from a `MediaSource` that reads through the engine. The same server implementation serves the LAN cast listener with a separate bind, token, and lifetime ([ADR 0010](./0010-casting-scope.md)).
2. **A hybrid policy, not blanket sequential.** The read-ahead window is strict sequential priority 0; everything outside it stays rarest-first, so swarm health and total transfer time are not sacrificed for the part nobody is watching. The window is derived from the file's real bitrate — `readahead_bytes = readahead_seconds × bitrate_bps / 8` — with a practical floor of 30–60 seconds of media and around two minutes for high-bitrate 4K remuxes.
3. **Head and tail priority**: the first and last pieces of the file are requested first, because the tail carries the container index (`moov`, cues, MKV seek head) that makes seeking possible at all.
4. **Read-ahead is a deadline-scheduled window, not a fixed lead time.** The controller derives the playhead from the server's request pattern plus mpv's cache state, and keeps a window of pieces ahead of it. Each piece in the window carries a deadline derived from its media offset; when a deadline is at risk, the piece is cancelled from the slow peer and re-requested from the fastest. That deadline behaviour is what separates streaming from downloading, and it is the part both reference implementations lack.
5. **Seek handling**: on a seek, the priority window is recomputed immediately, and in-flight low-priority piece requests are abandoned rather than completed. Abandoning work is the part most implementations get wrong.
6. **Buffer health is two numbers, defined precisely**: `buffered_seconds`, the contiguous verified-present span from the playhead, and `buffer_health_ratio = buffered_seconds / readahead_seconds`. Both are surfaced in the player. If the playhead reaches the end of contiguous data, we hold or pause playback rather than letting the demuxer read a hole.
7. **The player never blocks on the network**: if the buffer runs dry, the session transitions through `stalled` with actionable options rather than freezing the UI thread ([FLOWS](../FLOWS.md) SM3).
8. **A file that is fully downloaded takes the plain local-file path**, so the streaming session has no overhead when it is not needed.
9. **Play-while-downloading is a first-class action, not a side effect of completion.** As in uTorrent's Stream behaviour, a download exposes a Play action as soon as metadata is known — while the payload is still incomplete — and playback starts from the first bytes available:
   - A per-job **"Download in order"** toggle, off by default for non-media torrents and on by default when the picked files are media.
   - **Head and tail first**, so the container index exists before it is needed and seeking works from the start.
   - The player becomes the priority source: its read position drives the window, and closing the player releases the priority rather than leaving the torrent sequential forever.
   - While a stream is active, other jobs' bandwidth is capped so the stream keeps headroom.
10. **mpv's disk cache is not our piece store.** `--cache-on-disk` is append-only, never reclaims the space it prunes, and is deleted when playback closes, so it cannot back a torrent. The relevant knobs we do set are `--cache`, `--cache-secs`, `--demuxer-max-bytes`, and `--demuxer-readahead-secs`, tuned so mpv's in-memory cache stays a buffer rather than a copy of the file.

Design against VLC's `stream_t` semantics — `read`, `peek`, `seek`, `can_seek`, `can_pause`, `can_control_pace` — as the interface shape for the byte source, since those are the operations a demuxer actually needs and they have been validated by a decade of VLC usage ([research/reference-codebases.md](../research/reference-codebases.md)).

## Consequences

Positive: seeking works, casting works, the player sees a normal HTTP URL, and the scheduling logic is ours to tune and to test. This is the capability the reference implementations lack, so it is also the feature most worth investing in.

Negative and accepted:

- We own HTTP Range correctness: `206` with the right `Content-Range`, `416` on unsatisfiable ranges, and connection teardown that abandons work rather than draining it.
- The loopback server is a security surface, however small. Mitigated by loopback-only binding for the player, a per-session token in the path, no directory listing, and a lifetime tied to the session.
- Read-ahead tuning is empirical. Mitigated by a local seeder harness and by making the window a setting with a conservative default.
- FUSE's advantage — the file looks like a file to any application — is lost. Accepted: our player and our cast server are the only consumers, and both speak HTTP.

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| FUSE or WinFsp mount | A kernel driver or a third-party dependency on every platform, blocking reads that stall the player, and full-file semantics we cannot guarantee for an incomplete download |
| Piping to stdin | No seeking and no casting; unacceptable for a media app |
| Download-then-play only | The entire user-visible point of the feature, and both reference implementations already behave this way |
| Rarest-first until completion, then play | Maximizes swarm health and minimizes time to a playable file. Nobody tunes a media app for swarm health at the expense of the user's first ten seconds |
| Hand the partial file path directly to mpv | mpv would read holes as zero bytes and either fail or play garbage; the resulting bug reports are undiagnosable |

## Verification

- `SPIKE-3` confirms the engine exposes per-file reads and controllable piece priorities on all three platforms.
- Phase 2 acceptance: a magnet from a local seeder downloads, streams, imports, and plays, with first playable byte under the target time.
- Phase 3 acceptance: playback survives a stall, a seek storm during buffering, and a network flap without restarting the app.
