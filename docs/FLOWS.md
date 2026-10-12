# Peregrine — Core Flows and State Machines

Sequence diagrams for the twelve flows that define the system, plus the four state machines they share. Types and tables referenced here are defined in [DATA_MODEL.md](./DATA_MODEL.md); module boundaries in [ARCHITECTURE.md](./ARCHITECTURE.md) §5.

Notation: `UI` is the React app, `IPC` is the thin Tauri command layer, services are `peregrine-app` service objects, and adapters are leaf crates.

---

## F1 — First-run setup and permissions

```mermaid
sequenceDiagram
    autonumber
    participant UI
    participant IPC
    participant Settings as SettingsService
    participant Perm as platform::Permissions
    participant Secrets as secrets::Keyring
    participant Sync as SyncService
    participant Acct as AccountService
    participant DB as storage

    UI->>IPC: system.get_permissions
    IPC->>Perm: probe each capability
    Perm-->>IPC: ungranted set
    IPC-->>UI: setup state

    Note over UI: Wizard steps: welcome, your account (name and avatar),<br/>language and themes, folders, providers, playback, indexers, done

    UI->>IPC: accounts.create(name, avatar_kind, avatar_ref)
    IPC->>Acct: validate the name, normalise the avatar, insert the admin account
    Acct-->>UI: the account with its avatar ready to paint

    UI->>IPC: system.request_permission(kind=library_folder)
    IPC->>Perm: open native folder picker
    Perm-->>IPC: SafePath
    IPC->>Settings: add_library_root(path, kind)
    Settings->>DB: insert library_root

    UI->>IPC: settings.set_value(metadata.tmdb_api_key)
    IPC->>Secrets: store key, return secret_ref
    Secrets-->>IPC: ref
    IPC->>Settings: persist secret_ref only
    Settings->>DB: insert secret_ref

    UI->>IPC: settings.set_value(downloads.dir)
    UI->>IPC: settings.set_value(theme.mode=system)
    UI->>IPC: system.request_permission(kind=notifications)

    UI->>IPC: sync.bootstrap
    IPC->>Sync: enqueue imdb.ratings + providers.healthcheck
    Sync->>DB: seed job rows
    Sync-->>IPC: job ids
    IPC-->>UI: setup complete, jobs streaming

    Note over UI: Wizard is skippable at every step except<br/>downloads directory, which is required for acquisition
```

Rules: no permission is requested before the screen that explains it; a skipped step leaves the corresponding feature visibly disabled with a one-click "enable later" affordance rather than a silent failure. Nothing in the wizard blocks on network.

Tests: wizard resumability (kill mid-wizard, relaunch), missing keyring backend on Linux (falls back to an encrypted file with an explicit warning), permission revoked externally after setup (detected on next use and surfaced).

---

## F2 — Cold start and discovery hydration

```mermaid
sequenceDiagram
    autonumber
    participant UI
    participant IPC
    participant Disc as DiscoveryService
    participant Cache as storage::cache
    participant DB as storage
    participant Sync as SyncService
    participant TMDB as providers::tmdb

    Note over UI: Window shown only after first paint is possible
    UI->>IPC: discover.get_home_rails(account, page)
    IPC->>Disc: home_rails()
    Disc->>Cache: read rail manifest

    alt Cached manifest is fresh
        Cache-->>Disc: titles + artwork refs
        Disc-->>UI: rails (cache_hit = true) in ≤100 ms
        Note over UI: Skeleton blurhash renders immediately, then bytes stream in
    else Cold cache
        Disc-->>UI: rail shells + empty slots
        Disc->>Sync: enqueue trending and discover refresh
        Sync->>TMDB: trending, discover
        TMDB-->>Sync: normalized titles
        Sync->>DB: upsert titles, enqueue image prefetch
        Sync-->>UI: sync://health + library://changed
        UI->>IPC: discover.get_home_rails
        Disc-->>UI: rails
    end

    par Background revalidation
        Sync->>TMDB: if-none-match for each stale rail
        TMDB-->>Sync: 304 or new body
        Sync->>DB: touch or update
        Sync-->>UI: library://changed (affected keys only)
    and Image hydration
        UI->>IPC: asset request for poster variant
        IPC->>Cache: lookup
        Cache-->>UI: bytes or miss then fetch, resize, cache
    end
```

Why the UI never waits: the rail query reads only locally cached state and returns whatever exists. Network work is always a job whose completion invalidates cache keys. The blurhash is stored on the title row, so the very first paint has real colors — no grey boxes, no spinners.

Tests: cold-cache first paint under 100 ms with an empty DB (rail shells only), warm-cache first paint, offline cold start (empty state explains itself and offers retry), and a revalidation storm where 8 rails go stale simultaneously (must coalesce into one batched invalidation, not 8 refetches).

---

## F3 — Search

```mermaid
sequenceDiagram
    autonumber
    participant UI
    participant IPC
    participant Search as DiscoveryService
    participant FTS as storage::fts
    participant Sync as SyncService
    participant Provider as MetadataProvider

    UI->>IPC: discover.search(query, filters, cursor)
    IPC->>Search: search()

    alt Query matches an external id pattern
        Search->>FTS: external_id lookup
        FTS-->>Search: exact title
    else Local FTS
        Search->>FTS: MATCH prefix terms, bm25 + popularity
        FTS-->>Search: ranked rows with total count
    end
    Search-->>UI: local results in ≤50 ms p95
    Note over UI: Grid renders immediately; a remote row appears below if local is thin

    opt Local results are thin and the query has been stable for 250 ms
        Search->>Sync: enqueue search.remote with debounce key
        Sync->>Provider: search
        Provider-->>Sync: candidates
        Sync->>FTS: upsert as stub titles
        Sync-->>UI: library://changed search key
        UI->>IPC: discover.search(same query)
        Search-->>UI: merged results
    end
```

Rules: keystrokes never hit the network. The debounce key coalesces duplicates through the job dedupe index, so typing "breaking bad" one character at a time enqueues at most one remote search. Results are ranked locally, and the sort is stable across pages so keyset pagination cannot duplicate or skip a row.

Tests: property test that a query of 1–40 characters never panics or returns unordered results; a 100 k-title FTS bench (`SPIKE-5`); remote search failure leaves local results intact.

---

## F4 — Incremental metadata sync

```mermaid
sequenceDiagram
    autonumber
    participant Sched as SyncService scheduler
    participant DB as storage
    participant TMDB as providers::tmdb
    participant IMDb as dataset::imdb
    participant UI
    participant Img as ImageCache

    loop Every 24 h per changes cursor (the API's floor)
        Sched->>DB: read dataset_state(tmdb.changes.movie)
        Sched->>TMDB: /movie/changes?start_date=cursor
        TMDB-->>Sched: changed ids
        Sched->>DB: upsert job rows for affected titles, batch 50
        Sched->>DB: advance cursor
    end

    loop Every 24 h
        Sched->>IMDb: HEAD title.ratings.tsv.gz
        alt ETag unchanged
            IMDb-->>Sched: 304
            Sched->>DB: touch dataset_state
        else New dataset
            IMDb-->>Sched: stream body
            Sched->>DB: replace ratings table in one transaction
            Sched->>DB: update title.imdb_rating and imdb_votes by external_id join
            Sched-->>UI: library://changed (ratings)
        end
    end

    par Per-title refresh, rate limited per host
        Sched->>TMDB: /movie/{id} or /tv/{id} with if-none-match
        TMDB-->>Sched: full body or 304
        Sched->>DB: upsert title, seasons, episodes, credits, artwork refs
        Sched->>Img: enqueue prefetch for primaries
    end

    Sched-->>UI: sync://health with counts and next run
    Note over Sched: 429 or Retry-After parks the token bucket<br/>and reschedules with jitter, never retries into the wall
```

The cursor is persisted before the work is done, so a crash mid-window loses at most one window's worth of change detection, and the overlapping window (`start_date = cursor - 1 day`) makes that self-healing.

Tests: simulated 429 with `Retry-After`, provider 500 storm (circuit opens, health degrades, UI keeps serving cache), schema-drift fixture (unknown field, wrong type) must mark the row stale without losing the existing data, and a crash between cursor advance and title upsert.

---

## F5 — Release resolution and quality selection

```mermaid
sequenceDiagram
    autonumber
    participant UI
    participant Acq as AcquisitionService
    participant Idx as Indexer port
    participant Rank as domain::ReleaseRanker
    participant DB as storage

    UI->>Acq: search_releases(title_or_episode, account)
    Acq->>DB: load enabled indexers, ordered by priority
    par Per indexer, rate limited
        Acq->>Idx: search(query, categories)
        Idx-->>Acq: raw releases
    end
    Acq->>Acq: normalize, dedupe by info hash and normalized title
    Acq->>Rank: rank(candidates, account, context)
    Rank-->>Acq: scored list with per-rule reasons
    Acq->>DB: persist release_decision rows
    Acq-->>UI: ranked releases with explanations

    Note over Rank: Pure function. Inputs: resolution, source, codec, HDR,<br/>audio, size versus runtime and account caps, release group<br/>allow/block, seeder count, age, and upgrade cut-off rank.
```

Ranking is a pure domain function so it can be property-tested and unit-tested exhaustively: monotonicity (better resolution never lowers a score within a account), determinism (same inputs, same order), and totality (every candidate gets a finite score, no panics on malformed release names).

Auto-selection requires all of: the top candidate above the account's minimum score, seeders above the configured floor, size within caps, and the indexer healthy. Otherwise the UI presents the list.

---

## F6 — Acquisition with the safety gate

```mermaid
sequenceDiagram
    autonumber
    participant UI
    participant Acq as AcquisitionService
    participant Eng as TorrentEngine
    participant Gate as domain::TorrentInspector
    participant Perm as PathGuard
    participant DB as storage
    participant Stream as StreamServer

    UI->>Acq: start_download(release, account)
    Acq->>DB: insert download_job(state=queued, safety_report_json=NULL)
    Acq->>Eng: fetch metadata only (magnet, .torrent bytes)
    Eng-->>Acq: TorrentMeta with file list and total size

    Acq->>Gate: inspect(meta, save_path, settings)
    Gate->>Gate: validate each path, extension allowlist,<br/>count and size caps, no symlink, no reserved names
    Gate-->>Acq: SafetyReport{verdict, findings[], file_count, total_bytes}

    alt Blocked
        Acq->>DB: state=blocked, persist report, error_code=UNSAFE_CONTENT
        Acq-->>UI: blocked with per-file reasons
        Note over UI: No bytes were written. Override is offered only for<br/>warn-level findings, never for executables or traversal.
    else Warnings
        Acq-->>UI: confirmation with the exact findings
        UI->>Acq: confirm_download(job_id, acknowledged[])
    else Clean
        Acq->>DB: persist report
    end

    Acq->>Perm: resolve(save_path, root=downloads)
    Perm-->>Acq: SafePath
    Acq->>Acq: check free space against total size + floor
    Acq->>Eng: add(torrent, files=selected, sequential=true)
    Eng-->>Acq: handle
    Acq->>DB: state=downloading, engine_state_json, save_path
    Eng-->>Acq: progress ticks
    Acq->>DB: coalesced progress write ≤1 Hz per job
    Acq-->>UI: download://stats throttled to ≤4 Hz

    opt Play while downloading
        UI->>Stream: open session for the primary media file
        Stream->>Eng: sequential read-ahead from byte 0
    end

    Eng-->>Acq: complete
    Acq->>DB: state=seeding or completed, completed_at
    Acq->>Acq: import into library (media_item + media_file + probe)
    Acq-->>UI: library://changed + notification
```

Critical ordering: **inspection precedes any byte written.** The engine is asked for metadata only; the file set is validated against the resolved `SafePath`; only then is the torrent handed over with an explicit file selection. The `SafetyReport` is persisted with the job so the UI can explain a block after a restart without re-fetching metadata.

Tests: hostile fixtures (traversal path, absolute path, `..%2f` encoded, symlink entry, Windows reserved name, unicode right-to-left spoofed `.exe`, 10 000-file torrent, 5 TB reported size, nested archive bomb), free-space boundary (exactly enough, one byte short), and an upgrade flow that keeps the old file until the new one is verified.

---

## F7 — Streaming playback

```mermaid
sequenceDiagram
    autonumber
    participant UI
    participant PS as PlaybackService
    participant Src as MediaSource
    participant Eng as TorrentEngine
    participant Stream as StreamServer
    participant Mpv as mpv child process
    participant DB as storage

    UI->>PS: start_session(media_file_id or episode_id)
    PS->>DB: read watch_progress and track preferences

    alt Local file
        PS->>Src: LocalFile(path)
    else Torrent not complete
        PS->>Src: TorrentStream(info_hash, file_index)
        Src->>Eng: prioritize first and next pieces with read-ahead
    end

    PS->>Stream: open session, mint token
    Stream-->>PS: http://127.0.0.1:PORT/s/{token}/media
    PS->>PS: probe cache lookup or ffprobe
    PS->>Mpv: spawn with hardened flags, surface handle, ipc socket
    PS->>Mpv: loadfile(url, start=resume_ms, sub-files, tracks)
    Mpv-->>PS: property change: playback-restart, duration, tracks
    PS-->>UI: player://state (playing)
    UI->>UI: overlay mounts over the video surface

    loop Every 250 ms while playing
        Mpv-->>PS: time-pos, cache-speed, demuxer-cache-duration, dropped frames
    end
    loop Every 5 s and on pause, seek, track change, and close
        PS->>DB: write watch_progress coalesced
    end

    alt Buffer starvation on a torrent stream
        PS->>Eng: raise piece priority for the read window
        PS-->>UI: buffering indicator with buffer health, not a spinner
        opt Starvation persists beyond the threshold
            PS-->>UI: offer pause, lower read-ahead ceiling, or wait for completion
        end
    end

    alt User quits or file ends
        PS->>DB: final position, watched flag per policy (≥92% or credits)
        PS->>Mpv: quit, reap
        PS->>Stream: close session, invalidate token
        PS-->>UI: player://state (ended) + next-up payload
    end
```

Both paths share one `MediaSource` abstraction, so resume, track selection, subtitle attachment, and progress reporting are implemented once. mpv runs with `--no-config` and a fixed flag set so a user's global mpv config cannot inject scripts into our session, and its video surface is attached per platform: `--wid` on Windows and X11, an app-positioned child window on macOS, and a documented degraded hand-off on Wayland ([adr/0013](./adr/0013-mpv-distribution-and-video-surface.md)).

Tests: resume at 2%, at 50%, at 99% (must not resume 3 s before credits), seek storm during buffering, mpv crash mid-playback (state becomes `failed`, app survives, retry resumes from the last known position), track selection persistence per show, and an EPIPE on the mpv IPC socket (treated as a crash, not a hang).

---

## F8 — Local library scan and match

```mermaid
sequenceDiagram
    autonumber
    participant UI
    participant Lib as LibraryService
    participant Scan as Scanner
    participant Parse as domain::NameParser
    participant Probe as ProbeService
    participant DB as storage
    participant FF as ffprobe

    UI->>Lib: scan_library(roots, mode=incremental)
    Lib->>DB: insert job(library.scan)
    Lib->>Scan: walk roots with ignore rules
    Scan-->>Lib: candidate files with mtime and size

    Lib->>DB: skip files unchanged since last scan
    par Per file, bounded concurrency
        Lib->>Parse: parse(path, sidecar nfo if present)
        Parse-->>Lib: ParsedMedia{title, year, season, episode, confidence, reasons}
        alt Confidence high
            Lib->>DB: match to existing title or create stub
        else Confidence low or ambiguous
            Lib->>DB: insert unmatched_file with suggestions
        end
        Lib->>Probe: probe(path)
        Probe->>FF: ffprobe -print_format json
        FF-->>Probe: streams and format
        Probe->>DB: cache by path + mtime + probe_version
    end

    Lib->>DB: reconcile media_item and media_file rows
    Lib->>DB: mark missing_since for absent paths
    Lib-->>UI: library://changed, unmatched count, storage delta
    Note over UI: Scan progress is a job; the UI stays fully interactive
```

Reconciliation never deletes watch progress or catalogue rows when a file disappears; it marks `missing_since` and keep the rows, so re-adding the file restores the item instead of recreating an orphan.

Tests: naming corpus of 200 real-world release and file names with expected parses (anime absolute, multi-episode, season packs, year ambiguity, sample files, extras folders, `S01E01E02`, `1x01`, `EP01`, `Season 1/01.mkv`), a file that changes size without changing mtime (hash fallback), a root that becomes unreachable (network volume), and 20 k files scanned within the time budget of `SPIKE-7`.

---

## F9 — Subtitle acquisition and live sync

```mermaid
sequenceDiagram
    autonumber
    participant UI
    participant Sub as SubtitleService
    participant Pref as preference chain
    participant Prov as SubtitleProvider
    participant DB as storage
    participant Mpv as mpv
    participant Conv as ffmpeg

    UI->>Sub: ensure_subtitles(media_file, languages)
    Sub->>DB: load existing tracks for this file or episode
    alt Preferred language already present
        Sub-->>UI: tracks ready
    else Missing
        Sub->>Sub: compute opensubtitles moviehash for the file
        Sub->>Pref: language chain, e.g. [en, en-SDH, sw]
        par Per provider in the chain
            Sub->>Prov: search(hash, imdb_id, tmdb_id, season, episode, language)
            Prov-->>Sub: candidates with score
        end
        Sub->>Sub: rank by hash match, release name match, downloads, rating
        Sub->>Prov: download(best candidate)
        Prov-->>Sub: subtitle bytes
        Sub->>Sub: detect encoding, convert to UTF-8 SRT or ASS
        Sub->>DB: insert subtitle_track with score reasons
    end

    opt Embedded tracks exist
        UI->>Sub: extract_embedded(media_file, stream_index)
        Sub->>Conv: ffmpeg -map 0:s:N -c:s srt extraction, no video touch
        Conv-->>Sub: subtitle file
        Sub->>DB: insert subtitle_track(origin=embedded)
    end

    UI->>Sub: attach(session, track)
    Note over Sub,Mpv: Runtime subtitle changes use sub-add with the file URL and a flag of<br/>select, auto, or cached, plus change-list sub-files append. Bare sub-file is<br/>now an alias of sub-files-append, and setting sub-file alone does nothing. Then apply sub-delay and sid.
    opt Live offset adjustment
        UI->>Sub: set_subtitle_offset_ms(+250)
        Sub->>DB: persist offset per track
        Sub->>Mpv: set sub-delay
    end
```

The provider chain, not the user, handles quota exhaustion: OpenSubtitles quota reached means the next provider in the chain is tried, and the UI only reports the fall-through when every provider fails. Per-title automatic fetching respects a "subtitles only for languages I watch" setting so a background scan cannot burn a provider quota on 2 000 files.

Tests: encoding matrix (UTF-8, UTF-8 BOM, CP1252, ISO-8859-1, Shift-JIS, UTF-16LE), ASS to SRT round-trip preserving italics, offset persistence across sessions, provider quota exhaustion fall-through, hash mismatch fallback to metadata scoring, and a 0-byte or HTML-error-body "subtitle" download rejected before persistence.

---

## F10 — Watch progress, resume, and next-up

```mermaid
sequenceDiagram
    autonumber
    participant Player as PlaybackService
    participant DB as storage
    participant Policy as domain::WatchPolicy
    participant UI
    participant Trakt as optional Trakt adapter

    Player->>DB: read watch_progress(account, episode)
    DB-->>Player: position_ms, duration_ms, watched
    Player->>Policy: resume_point(progress, duration, settings)
    Policy-->>Player: start_at_ms or restart or next_episode

    Note over Policy: percent <0.02 → start at 0<br/>0.02..0.95 → resume at position - 3 s<br/>≥0.95 or credits → mark watched, offer next

    loop During playback
        Player->>DB: coalesced position write every 5 s and on state change
    end

    Player->>Policy: finalize(position, duration, ended_naturally)
    Policy-->>Player: {watched, watched_at, play_count}
    Player->>DB: upsert watch_progress
    Player->>DB: insert event_log(progress.updated)

    opt Next episode exists and autoplay is on
        Player->>DB: read next episode in the active ordering scheme
        Player-->>UI: player://state next_up with countdown
        UI->>Player: play_next or cancel
    end

    opt Trakt connected
        Player->>Trakt: scrobble start, pause, stop
        Trakt-->>Player: accepted or rate limited
        Note over Trakt: Failures never block local playback and never roll back local state
    end
```

The watched policy is a pure function with the boundary cases pinned by tests: exactly at the threshold, a video shorter than the skip window, a manual "mark unwatched" that must survive the next finalize, and a re-watch that increments `play_count` without clearing `watched_at`.

---

## F11 — Casting session

```mermaid
sequenceDiagram
    autonumber
    participant UI
    participant Cast as CastService
    participant Disc as discovery
    participant Renderer as Chromecast or DLNA
    participant Cap as CapabilityModel
    participant Stream as StreamServer
    participant FF as ffmpeg
    participant DB as storage

    UI->>Cast: discover_devices
    Cast->>Disc: mDNS _googlecast._tcp and SSDP M-SEARCH
    Disc-->>Cast: devices with model and protocol
    Cast-->>UI: device list, pinned first

    UI->>Cast: start_cast(device_id, media_file_id)
    Cast->>DB: load probe result and track selection
    Cast->>Cap: decide_delivery(file, device, tracks)

    alt Direct play supported
        Cap-->>Cast: delivery=direct
    else Container or codec unsupported
        Cap-->>Cast: delivery=remux_hls
        Cast->>FF: remux to fragmented MP4 HLS, copy video, transcode audio if needed
    else Video codec unsupported
        Cap-->>Cast: delivery=transcode_hls
        Cast->>FF: transcode via a hardware encoder, libopenh264 fallback, never libx264
    end

    Cast->>Stream: open LAN listener bound to the selected interface with a session token
    Stream-->>Cast: http URL with token, or PIN-protected URL
    Cast->>Renderer: load media, set metadata, attach or burn in subtitles
    Renderer-->>Cast: status stream
    Cast->>DB: insert cast_session, state=playing
    Cast-->>UI: cast://state with position and delivery mode

    loop Every 2 s
        Renderer-->>Cast: position, buffering, idle reason
        Cast->>DB: update cast_session position
    end

    opt User returns to the desktop player
        UI->>Cast: stop_cast(transfer_position=true)
        Cast->>Renderer: stop
        Cast->>DB: write watch_progress from the cast position, close session
        Cast->>Stream: close LAN listener and invalidate token
    end
```

Casting is off by default. Turning it on binds a LAN listener, and the settings screen states plainly that any device on the same network could reach the token URL during a session, with a PIN option for shared networks. The listener is closed the moment the session ends — it is not a long-lived media server.

Tests: capability matrix fixtures per renderer class, remux decision correctness for MKV/H.265/AC3/DTS inputs, unsupported subtitle format paths (PGS must burn in), renderer disappearing mid-session, port conflict on the LAN bind, and the assertion that no LAN listener exists when casting has never been enabled.

---

## F12 — Deletion with confirmation

```mermaid
sequenceDiagram
    autonumber
    participant UI
    participant Maint as MaintenanceService
    participant DB as storage
    participant Guard as PathGuard
    participant Trash as platform::Trash
    participant Eng as TorrentEngine

    UI->>Maint: plan_deletion(selection)
    Maint->>DB: resolve files, sizes, active jobs, shared files
    Maint-->>UI: DeletionPlan{files[], total_bytes, warnings[], blockers[]}

    Note over UI: Dialog enumerates every path and byte count.<br/>Blockers: file in use by the player, seeding job,<br/>file shared by two media items, path outside a root.

    alt Plan has blockers
        UI->>UI: offer to stop the player or stop seeding first
    else Ready
        UI->>Maint: confirm_deletion(plan, mode=trash or permanent, typed_phrase?)
        Maint->>Guard: resolve each path again, verify containment and identity
        Guard-->>Maint: SafePath list or rejection

        alt Trash mode
            Maint->>Trash: move each file to the OS trash
        else Permanent
            Maint->>Maint: unlink only files recorded in media_file or download_file
        end

        Maint->>Eng: remove torrent and stop seeding if applicable
        Maint->>DB: delete media_file rows, downgrade or delete media_item, keep watch_progress
        Maint->>DB: delete subtitle_track rows pointing at removed files
        Maint->>DB: insert event_log(media.deleted) with sizes preserved
        Maint-->>UI: result per file, reclaimed bytes, undo offer where trash is reversible
    end
```

Deletion never cascades from a title to files unless the user explicitly selected "delete files" — removing something from the library and reclaiming disk are two distinct, separately confirmed actions.

Tests: path revalidation between plan and confirm (a symlink swapped in between must be refused), a file shared by two items (blocked with an explanation), a file currently playing, a seeding job (must be stopped first), trash unavailable on the filesystem (fall back to permanent with a distinct confirmation), and 500-file bulk deletion (progress job, cancelable, no partial DB state on cancel).

---

## F13 — Notifications, consent, and click routing

```mermaid
sequenceDiagram
    autonumber
    participant UI
    participant NS as NotificationService
    participant DB as storage
    participant OS as OS notification backend
    participant Deep as deep-link router

    Note over UI,NS: Consent is asked in the wizard and at the first<br/>relevant action, never unprompted at launch

    UI->>NS: probe_permission
    NS->>OS: isPermissionGranted
    alt Not granted and the user consented
        NS->>OS: requestPermission
        OS-->>NS: granted or denied
    end
    NS-->>UI: state, test-notification button, OS settings link

    Note over NS: An event arrives, for example download.completed
    NS->>DB: insert notification, delivery_state=pending
    NS->>NS: policy: enabled, quiet hours, focus, dedupe window, rate limit

    alt Suppressed
        NS->>DB: delivery_state=suppressed with a reason
        NS-->>UI: centre entry only, no toast and no sound
    else Allowed
        NS->>NS: render title and body per the privacy setting
        NS->>OS: show with int32 id, resolved sound, deep link in extra
        NS->>DB: delivery_state=delivered, delivered_at, native_id
        NS-->>UI: centre entry plus the in-app chime when in-app-only
    end

    Note over NS,OS: Critical events ignore quiet hours but never bypass OS focus modes

    alt User clicks the toast
        OS-->>NS: actionPerformed with tap
        NS->>DB: clicked_at
        NS->>Deep: route the deep link carried in extra
        Deep-->>UI: focus the window and navigate
    else The toast was lost, for example the Windows Action Center
        UI->>DB: user opens the in-app centre
        UI->>Deep: the same deep link, the same route
    end
```

Invariants and tests for this flow:

- The centre entry is written **before** the OS call, so a crash between the two loses a toast and never the event.
- A notification is only shown with a listener registered; the handler is installed during startup, because the plugin neither displays nor reports actions without one, and notifications shown before it exists report nothing.
- The same deep link is reachable from the centre, so no capability depends on a toast being clicked. This matters on Windows, where a click is only reported while the toast is on screen.
- Sound-off is best-effort on Linux and Windows and the UI says so; the in-app chime is the only sound whose behaviour we control end to end.
- Tests: consent declined (the centre still works, no toast attempted); permission granted but delivery fails (state `failed` with an error, entry still present); quiet hours (non-critical suppressed with a reason, critical delivered); a burst of five completions (one summary toast, five centre entries); the window focused on the downloads screen (no toast); and a body that never contains a path, a magnet, an infohash, or an adult title.

---

## F14 — Monitoring and availability detection

```mermaid
sequenceDiagram
    autonumber
    participant User
    participant Mon as MonitorService
    participant Sched as job scheduler
    participant Meta as metadata providers
    participant Idx as indexer RSS
    participant DB as storage
    participant Notif as NotificationService

    User->>Mon: add_monitor with scope, target, notify and acquire policy
    Mon->>DB: insert monitor in state scheduled with its sources
    Mon->>Sched: enqueue the air-date sync

    loop Per source and cadence, never faster than the source can change
        Sched->>Meta: episode air dates, release dates, TVDB updates
        Meta-->>Sched: dates, release types, changed ids
        Sched->>DB: reconcile and write a monitor_event for a real change
        Sched->>Idx: poll feeds for monitored targets
        Idx-->>Sched: candidate releases
    end

    Mon->>Mon: derive state from evidence: scheduled, airing, released, release_found
    alt The state transitioned
        Mon->>DB: update the monitor and insert a monitor_event with its dedupe key
        alt This key has notified before
            Mon->>DB: skip the notification
        else First time
            Mon->>Notif: raise the monitor event
            Notif-->>User: toast plus centre entry
        end
        opt Acquire policy is auto_acquire and a release clears the account floor
            Mon->>Mon: request acquisition through the normal path
            Note over Mon: safety gate, ranking, queue limits and the free-space floor all still apply
        end
    end

    opt The app was closed for weeks
        Mon->>Sched: one coalesced catch-up pass, never one per missed day
        Note over Mon: notify the current state only, never every transition that was missed
    end
```

Invariants: the state only advances on recorded evidence, so an event-less transition is a test failure; the dedupe key is unique in the database, so a re-poll that re-derives the same state cannot notify twice; and a monitor in `paused`, `ended`, or `blackholed` costs no requests at all.

Tests: an air date that moves (one `air_date_moved` event, one notification, no duplicates on re-poll); a source that fails (state unchanged, `source_failed` recorded, no notification claiming availability); the app closed for a month (one catch-up pass, current state only); a release appearing before the air date (anime and early leaks must not be treated as an error); and a 20-monitor watchlist staying inside the documented request budget.

---

## F15 — MCP tool call with consent

```mermaid
sequenceDiagram
    autonumber
    participant Client as Agent client
    participant Bridge as peregrine-mcp
    participant Svc as McpService
    participant Audit as mcp_call audit
    participant UI as Settings and approval dialog
    participant Acq as AcquisitionService

    Client->>Bridge: server/discover, then tools/list
    Bridge->>Svc: the same over the local socket, protocol version per request
    alt First contact from this client
        Svc->>UI: pairing code, client name, requested scope
        UI-->>Svc: approved with read scope
        Svc->>Svc: issue a token, store only its hash, register the client
    end
    Svc-->>Client: tool schemas

    Client->>Bridge: tools/call start_download
    Bridge->>Svc: forward
    Svc->>Audit: record the call
    alt The client lacks the scope
        Svc-->>Client: denied, naming the missing scope
    else The scope is granted
        Svc->>Acq: rank releases and validate against the account
        Acq-->>Svc: candidate plus safety report
        Svc->>Audit: outcome pending_approval
        Svc-->>Client: pending_approval with an approval id
        Svc->>UI: dialog naming the client, release, size and quality
        alt The user approves
            UI->>Acq: start with the approval id
            Acq-->>UI: queued
            Svc-->>Client: resolved on the next status call
        else The user denies
            Svc-->>Client: rejected with a reason and no retry hint
        end
    end

    Note over Svc,Audit: Every outcome writes an mcp_call row, including denials and rate limits
```

Invariants: a download cannot start without an approved request for the requesting client; every call is audited before it returns; results are size-capped and every externally sourced string is returned inside a labelled `untrusted` wrapper; and a revoked client fails its next call with a clear error rather than a silent no-op.

Tests: a poisonous title overview containing an instruction to download reaches the client as labelled data and cannot cause a download without a human click; a client with `read` only cannot call `start_download`; approving one request does not pre-approve the next; the audit log contains a row for all eight outcomes; and the HTTP transport rejects a mismatched protocol version and a non-loopback `Host`.

---

## F16 — Account switching, OS authentication, and content gating

```mermaid
sequenceDiagram
    autonumber
    participant User
    participant UI as account picker
    participant Acct as AccountService
    participant Auth as OsAuthenticator
    participant OS as OS credential
    participant DB as storage
    participant Store as repositories

    Note over UI: The picker shows local accounts and avatars.<br/>No network call of any kind, ever
    User->>UI: pick an account
    UI->>Acct: activate(account)
    alt The target scope is broader than the current one and the account is locked
        Acct->>Auth: verify device owner
        Auth->>OS: LAContext, or UserConsentVerifier, or polkit
        OS-->>Auth: granted or denied
        Acct->>DB: unlock attempt with its method and outcome
    else Narrower or equal scope, or the account is unlocked
        Note over Acct: An unauthenticated switch may only narrow what is visible,<br/>which is what makes guest safe by construction
    end
    Acct->>Store: bind the AccountScope for the session
    Store-->>UI: content, search, and counts filtered from here on

    User->>Acct: request a title
    Acct->>DB: insert content_request, state pending
    Acct->>UI: notify the adult accounts

    User->>UI: try again while the gate is backing off
    UI->>Acct: activate(adult)
    Acct-->>UI: refused by backoff, with a countdown and no hint about what lies behind the gate

    Note over Acct,DB: Guest is a single shared account, not one per visitor.<br/>When a guest session ends, its progress, watchlist, and requests are deleted

    Note over User,Store: An adult approves the pending request, which authorises the acquisition
```

Invariants: an unauthenticated switch may only narrow the scope, so escaping into guest can never widen what a child sees; the scope is bound once at activation and never re-derived in the UI; blocked content is indistinguishable from absent content; a child account can never create a download job regardless of the request path; failures back off and are audited; and where no OS mechanism exists the account still works but the parent is told the gate is an app PIN.

Tests: every indirect leak path (suggestions, similar titles, counts, continue-watching, notification bodies, the media server, a hand-written MCP call) under a restricted scope; an unrated title hidden by default; a burst of failed unlocks; and switching to an adult account from the child account while the OS dialog is cancelled.

---

## State machines

### SM1 — Download job

```mermaid
stateDiagram-v2
    [*] --> queued
    queued --> inspecting: room in the active set
    inspecting --> blocked: safety gate rejects
    inspecting --> metadata: gate passed
    metadata --> downloading: engine accepted, files selected
    metadata --> failed: no peers or no metadata within timeout
    downloading --> paused: user or bandwidth policy
    paused --> downloading: resume
    downloading --> seeding: payload complete
    downloading --> failed: unrecoverable engine error
    seeding --> completed: seeding policy satisfied
    seeding --> paused: user stops seeding early
    downloading --> removed: user removes
    paused --> removed: user removes
    blocked --> removed: user dismisses
    failed --> queued: retry with a different release
    completed --> [*]
    removed --> [*]
```

Terminal states: `completed`, `removed`, `failed` (after retry exhaustion). Every non-terminal state has a persisted `engine_state_json`, so a restart reattaches rather than restarting the transfer. Progress writes are coalesced to at most 1 Hz per job.

### SM2 — Media availability

```mermaid
stateDiagram-v2
    [*] --> discovered
    discovered --> watchlisted: added to watchlist
    discovered --> wanted: auto-acquire enabled
    watchlisted --> wanted: acquisition requested
    wanted --> acquiring: download started
    acquiring --> available: file imported and probed
    acquiring --> unavailable: all attempts failed
    available --> partial: one of several files removed
    partial --> available: replacement file added
    available --> missing: file absent at scan
    missing --> available: file reappears
    available --> removed: user deletes files
    unavailable --> wanted: retry
    removed --> [*]
```

This state is what the poster overlays, badges, and the availability filter read. It is derived from `media_item` plus the job set, and computed once per invalidation rather than per render.

### SM3 — Player session

```mermaid
stateDiagram-v2
    [*] --> preparing
    preparing --> buffering: mpv spawned, file loading
    preparing --> failed: spawn or load error
    buffering --> playing: first frame rendered
    buffering --> stalled: no data
    stalled --> buffering: piece priority raised, data arrives
    stalled --> failed: timeout beyond threshold
    playing --> paused: user pause
    paused --> playing: resume
    playing --> ended: end of stream
    playing --> failed: mpv exited unexpectedly
    ended --> [*]: finalize progress
    failed --> [*]: report exit code, offer retry at last position
```

`playing` is only entered on the first rendered frame, which is what makes the "time to first frame" metric meaningful for torrent streams. Every transition out of a live state writes progress, so a crash at any point loses at most 5 seconds.

### SM4 — Job scheduler

```mermaid
stateDiagram-v2
    [*] --> queued
    queued --> running: worker acquired, resource token available
    running --> succeeded: completed
    running --> deferred: rate limit or retryable error
    running --> failed: non-retryable or attempts exhausted
    running --> canceled: cancellation requested
    deferred --> queued: backoff elapsed, tokens available
    succeeded --> [*]
    failed --> [*]
    canceled --> [*]
```

Resource acquisition happens before `running`, and a token is released on every exit path including panic (a guard type, tested with an injected panic). Terminal rows are excluded from the dedupe index, so a new request for the same work after a failure is not silently swallowed.
